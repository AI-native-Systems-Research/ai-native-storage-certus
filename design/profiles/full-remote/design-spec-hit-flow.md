# Design for the cache 'get' / 'lookup' direction (`full-remote` Profile)

## Overview

The `full-remote` lookup flow extends the `full` profile with a **remote path**: when a key is not found locally (neither DRAM nor SSD), the dispatcher forwards the miss to the RemoteLookup orchestrator, which reserves a landing slot in its own RDMA-registered memory and broadcasts a key query to peers. A peer that holds the value RDMA-**writes** it into the reserved slot. Symmetrically, when a peer queries a key this node holds, this node's RemoteLookupRdmaInitiator pushes the value out into the peer's memory.

## Assumptions and Invariants

- **Cache block sizes are variable.** The client provides an IPC handle with a size field; remote queries carry the *expected value length in bytes*.
- **Read references protect in-flight local reads.** Local lookups hold dispatch-map read references; eviction skips entries with active references. Remote transfer is one-sided RDMA WRITE, so it takes no *cross-node* reference on the requester — but the data-holder **does** hold a local read pin on each source entry until the write completes (see the remote-serve step below).
- **Remote lookups are best-effort.** A remote miss returns NotFound — there is no cascading or multi-hop forwarding.
- **No P2P DMA.** SSD→GPU and remote→GPU transfers always pass through DRAM.
- **RDMA for inter-node data movement.** The requester offers a landing region (via its Responder); the data-holder RDMA-writes into it (via its Initiator).

## Lookup Flow — Local Paths

### Warm Path (MemoryTier → GPU)

Same as `full` profile:
1. Client submits lookup request via shmq (`/dev/shm` mailbox) with key + IPC handle.
2. Dispatch-map lookup atomically takes read reference.
3. Cache miss → proceeds to cold path or remote path.
4. MemoryTier hit → `cudaMemcpyAsync` (H2D) via `warm_stream`.
5. Release read reference, touch LRU.
6. Return stream handle.

### Cold Path (BlockDevice → MemoryTier → GPU)

Same as `full` profile (base `dispatcher` crate):
1. BlockDevice hit → retrieve SSD offset; the read pin is held for the whole SSD→DRAM→GPU pipeline.
2. Evict if needed to make space (`try_evict_to_block`).
3. Allocate memory-tier slot.
4. Pipelined SSD→DRAM read via the `ColdReadPool` worker over the PipelineRing StagingPool, overlapped with H2D to GPU.
5. Promote in place via `promote_block_to_memory_tier(key, ptr, size)` (ssd_offset retained), then release the read pin.
6. GPU transfer complete; if the memory tier is saturated, `serve_cold_staged` streams SSD→staging→GPU without promoting (entry stays BlockDevice).

## Lookup Flow — Remote Path (this node is the requester)

When the dispatch-map returns `NotExist` (key not found locally):

1. **Local miss detected.** The dispatcher's lookup returns KeyNotFound from the dispatch-map.

2. **Forward to RemoteLookup.** The dispatcher calls `remote_lookup.batch_lookup(&[(key, expected_size)])` for keys that missed locally, where `expected_size` is the value length in bytes.

3. **Reserve a landing slot.** RemoteLookup reserves a slot inside its own memory-tier pool via `memory_tier.insert(key, size)`. The Responder registers the *whole* pool once as a single `REMOTE_WRITE` MR and hands out one pool-wide `rkey` (`local_region()`), so the slot is already RDMA-writable the moment it is allocated. Concurrent lookups for the same key are coalesced single-flight — later callers ride the first as followers rather than reserving duplicate slots.

4. **Phase 1 — memory probe.** RemoteLookup SHOUTs a `KeyQuery` over Zyre. Each peer WHISPERs a `KeyResponse` classifying every queried key as `Memory`, `Disk`, or `None`. Once a **quorum** of the expected peers replies (`quorum_pct`, default 80%) *or* `phase1_timeout` (default 20 ms) elapses, the round advances. A `Memory`-holder is preferred: RemoteLookup WHISPERs it an `RdmaRequest` carrying its `endpoint`, pool `rkey`, and slot descriptors.

5. **Phase 2 — disk re-scan.** If no memory-holder served the key, RemoteLookup re-scans the responses it already cached (no new SHOUT) and targets a `Disk`-holder. That peer promotes the key disk→memory before writing. The whole operation is bounded by `op_deadline` (default 50 ms) across up to `max_retry_rounds` rounds.

6. **RDMA data transfer.** The chosen data-holder connects to this node's Responder and RDMA-**writes** the value directly into the reserved slot. This node's Responder accepts the write; it never reads peer memory. The holder WHISPERs an `RdmaStatus` when the write lands.

7. **Publish on success.** On `RdmaStatus::Success` the value is already sitting in the reserved slot, so RemoteLookup simply publishes it: `dispatch_map.create_memory_tier_entry(key, ptr, len)` then `release_write(key)`. The key is now an ordinary memory-tier (warm) hit for future lookups. A slot whose op never published while still exposed to a live peer is **orphaned** rather than freed, and reclaimed on a late `RdmaStatus`, peer `EXIT`, or op timeout.

8. **GPU transfer.** Data is copied from the now-local memory-tier slot to the client GPU via the standard H2D DMA path.

**If no peer has the entry:** no peer classifies the key as `Memory` or `Disk`; RemoteLookup returns `NotFound` for that key, and the dispatcher returns KeyNotFound to the client.

## Incoming Remote Lookup (this node is the data-holder)

When a peer forwards a miss whose key this node holds:

1. **Receive the query.** The RemoteLookup actor receives the peer's `KeyQuery` over Zyre (`handle_key_query`) and, if the key is present in the memory-tier, the subsequent `RdmaRequest` (`handle_rdma_request`) carrying the peer's endpoint, `rkey`, and slot descriptors.

2. **Dispatch a serve command.** The actor sends `InitiatorCmd::Serve` to its off-loop initiator worker.

3. **Resolve and submit.** The worker pins each requested value in the local memory-tier (a dispatch-map read reference) and hands the batch to the RemoteLookupRdmaInitiator via `push_async`, which queues it and returns. A thread dedicated to that peer connects (or reuses a warmed connection) and RDMA-**writes** the values into the peer's advertised slots.

4. **Completion.** When every write in the batch has landed, the initiator invokes the batch's completion callback on that connection thread. The callback releases the read references and hands the per-key statuses to the actor as `PushComplete`.

   The transfer is one-sided, but the source entry **is** held under a read reference for the whole RDMA window, and the callback is what releases it. That is not incidental: the NIC reads the source buffer asynchronously after submission has returned, so releasing the reference any earlier would let the memory tier evict the value out from under an in-flight write.

**If key not found locally:** the `KeyQuery` is simply not answered; the requesting peer eventually observes `NotFound`.

## Batch Lookup

**`batch_lookup(&[(key, size)])`** processes multiple keys:
- Local hits (warm/cold) are served by the dispatcher.
- Local misses are batched and forwarded to RemoteLookup, which reserves slots and queries peers.
- Results are returned positionally as `Vec<Result<(), RemoteLookupError>>`.

## Interaction with Put and Eviction

- **Concurrent populate for same key:** Rejected by dispatch-map (AlreadyExists).
- **DRAM eviction during lookup:** Skips entries with active local read references.
- **Serving a peer pins the source entry locally, not across nodes:** the data-holder holds a dispatch-map read pin on each source value until its one-sided RDMA WRITE completes (the pin is owned by the completion callback, because the NIC keeps reading the buffer after submission returns) — so an in-flight push does keep the holder's memory-tier entry unevictable for the transfer window. What it does *not* take is a cross-node reference: the requester holds nothing on the holder. On the *requester* side, reserved landing slots are reclaimed via the Responder control channel (`Disconnect` → `DisconnectAck`) on peer EXIT.
- **Remove during lookup:** Fails with ActiveReferences if any local reader holds a ref.
- **Background write-through during lookup:** Coexists with local read refs.

## Observability

- **Hit/miss ratio** — counter of local hits vs. local misses.
- **Remote hit ratio** — fraction of local misses resolved by peer nodes.
- **Warm/cold/remote latency** — histogram breakdown by data path.
- **Remote lookup throughput** — requests forwarded to peers per second.
- **Incoming remote requests** — counter of `KeyQuery`/`RdmaRequest` served to peers (outbound pushes).
- **Push duration** — histogram of time from `InitiatorCmd::Serve` to `PushComplete` (RDMA Write duration).
- **Outstanding landing slots** — gauge of reserved-but-unfilled Responder slots (in-flight remote misses).
- **PipelineRing utilization** — gauge of concurrent pipelined local SSD reads.
