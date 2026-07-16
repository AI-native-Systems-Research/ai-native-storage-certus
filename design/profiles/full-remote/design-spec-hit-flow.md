# Design for the cache 'get' / 'lookup' direction (`full-remote` Profile)

## Overview

The `full-remote` lookup flow extends the `full` profile with a **remote path**: when a key is not found locally (neither DRAM nor SSD), the dispatcher can forward the miss to peer Certus nodes via the RemoteLookup component. Additionally, this node serves incoming remote lookups from peers via the RemoteLookupRdmaInitiator.

## Assumptions and Invariants

- **Cache block sizes are variable.** The client provides an IPC handle with a size field.
- **Read references protect in-flight reads.** Both local lookups and remote LookupRefs hold read references. Eviction skips entries with active references.
- **Remote lookups are best-effort.** A remote miss returns NotFound — there is no cascading or multi-hop forwarding.
- **No P2P DMA.** SSD→GPU and remote→GPU transfers always pass through DRAM.
- **RDMA for inter-node data movement.** Remote data arrives via RDMA Write into local memory.

## Lookup Flow — Local Paths

### Warm Path (MemoryTier → GPU)

Same as `full` profile:
1. Client submits lookup request via gRPC with key + IPC handle.
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

## Lookup Flow — Remote Path (unique to `full-remote`)

When the dispatch-map returns `NotExist` (key not found locally):

1. **Local miss detected.** The dispatcher's lookup returns KeyNotFound from the dispatch-map.

2. **Forward to RemoteLookup.** The dispatcher calls `remote_lookup.batch_lookup(entries)` for keys that missed locally.

3. **Peer resolution.** RemoteLookup queries peer Certus nodes via RDMA. Each peer's RemoteLookupRdmaInitiator resolves the key against its local dispatcher.

4. **RDMA data transfer.** If a peer has the entry, the peer's RemoteLookupRdmaInitiator acquires a LookupRef (read reference) and performs an RDMA Write of the data into the requesting node's memory.

5. **Local promotion (optional).** The received data may be inserted into the local memory-tier and dispatch-map so future lookups are warm/local.

6. **GPU transfer.** Data is copied from local memory to client GPU via the standard H2D DMA path.

7. **Peer releases reference.** After RDMA Write completes, the peer calls `release_lookup` to unpin the entry.

**If no peer has the entry:** RemoteLookup returns `NotFound` for that key, and the dispatcher returns KeyNotFound to the client.

## Incoming Remote Lookup (Peer → This Node)

When a peer node forwards a miss to this node:

1. **RemoteLookupRdmaInitiator receives request.** An RDMA request arrives with one or more cache keys.

2. **Resolve via local dispatcher.** `handle_lookup(key)` or `handle_batch_lookup(keys)` calls through to the dispatcher's dispatch-map.

3. **Acquire LookupRef.** On hit, a read reference is taken and a LookupRef (pointer + size + key) is returned. The memory-tier entry is pinned.

4. **RDMA Write to peer.** The data at the LookupRef pointer is sent to the requesting peer's memory via RDMA Write.

5. **Release reference.** `release_lookup(key)` releases the read reference, allowing the entry to be evicted if needed.

**If key not found locally:** `handle_lookup` returns `KeyNotFound`. The peer's RemoteLookup receives `NotFound`.

## Batch Lookup and Pre-Promotion

**`batch_lookup(entries)`** processes multiple keys concurrently:
- Local hits (warm/cold) are served in parallel.
- Local misses are batched and forwarded to RemoteLookup.
- Results are returned positionally.

**`promote_to_memory_tier(keys)`** pre-promotes cold entries to DRAM without GPU DMA. Useful for warming the local cache ahead of anticipated lookups.

## Interaction with Put and Eviction

- **Concurrent populate for same key:** Rejected by dispatch-map (AlreadyExists).
- **DRAM eviction during lookup:** Skips entries with active read references (both local and remote LookupRefs).
- **Remote LookupRef blocking eviction:** If RemoteLookupRdmaInitiator holds a LookupRef for an entry, that entry cannot be evicted from DRAM until `release_lookup` is called. A slow or failed peer can temporarily pin entries.
- **Remove during lookup:** Fails with ActiveReferences if any local or remote reader holds a ref.
- **Background write-through during lookup:** Coexists with both local and remote read refs.

## Observability

- **Hit/miss ratio** — counter of local hits vs. local misses.
- **Remote hit ratio** — fraction of local misses resolved by peer nodes.
- **Warm/cold/remote latency** — histogram breakdown by data path.
- **Remote lookup throughput** — requests forwarded to peers per second.
- **Incoming remote requests** — counter of requests served to peers.
- **LookupRef lifetime** — histogram of time between handle_lookup and release_lookup (RDMA Write duration).
- **Pinned entries** — gauge of entries held by outstanding LookupRefs (eviction pressure indicator).
- **PipelineRing utilization** — gauge of concurrent pipelined local SSD reads.
