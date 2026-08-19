# Design for the cache 'get' / 'lookup' direction (`full-remote` Profile)

## Overview

The `full-remote` lookup flow extends the `full` profile with a **remote path**: when a key is not found locally (neither DRAM nor SSD), the dispatcher forwards the miss to the RemoteLookup orchestrator, which reserves a landing slot in its own RDMA-registered memory and broadcasts a key query to peers. A peer that holds the value RDMA-**writes** it into the reserved slot. Symmetrically, when a peer queries a key this node holds, this node's RemoteLookupRdmaInitiator pushes the value out into the peer's memory.

## Assumptions and Invariants

- **Cache block sizes are variable.** The client provides an IPC handle with a size field; remote queries carry the *expected value length in bytes*.
- **Read references protect in-flight local reads.** Local lookups hold dispatch-map read references; eviction skips entries with active references. Remote transfer is one-sided RDMA WRITE and does not pin the data-holder's source entry.
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

Same as `full` profile:
1. BlockDevice hit → retrieve SSD offset.
2. Evict if needed to make space.
3. Allocate memory-tier slot.
4. Pipelined SSD→DRAM read via PipelineRing.
5. Re-register in dispatch-map as MemoryTier.
6. GPU transfer complete.

## Lookup Flow — Remote Path (this node is the requester)

When the dispatch-map returns `NotExist` (key not found locally):

1. **Local miss detected.** The dispatcher's lookup returns KeyNotFound from the dispatch-map.

2. **Forward to RemoteLookup.** The dispatcher calls `remote_lookup.batch_lookup(&[(key, expected_size)])` for keys that missed locally, where `expected_size` is the value length in bytes.

3. **Reserve a landing slot.** RemoteLookup reserves a slot in its own Responder-registered memory pool to receive the value.

4. **Query peers.** RemoteLookup SHOUTs a `KeyQuery` for the key over Zyre. A peer that holds it WHISPERs back.

5. **Advertise the slot.** RemoteLookup WHISPERs an `RdmaRequest` carrying its `endpoint`, pool `rkey`, and slot descriptors to the holding peer.

6. **RDMA data transfer.** The holding peer (data-holder) connects to this node's Responder and RDMA-**writes** the value directly into the reserved slot. This node's Responder accepts the write; it never reads peer memory.

7. **Local promotion (optional).** The received data may be inserted into the local memory-tier and dispatch-map so future lookups are warm/local.

8. **GPU transfer.** Data is copied from local memory to client GPU via the standard H2D DMA path.

**If no peer has the entry:** no peer replies to the `KeyQuery`; RemoteLookup returns `NotFound` for that key, and the dispatcher returns KeyNotFound to the client.

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
- **Serving a peer does not pin the source entry:** the data-holder copies the value out via one-sided RDMA WRITE, so a slow or failed peer does not pin the holder's memory-tier entry. On the *requester* side, reserved landing slots are reclaimed via the Responder control channel (`Disconnect` → `DisconnectAck`) on peer EXIT.
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
