# Design for the cache 'put' / 'populate' direction (`full-remote` Profile)

## Overview

The put flow in the `full-remote` profile is identical to the `full` profile. Populate operations are **local-only** — data is not propagated to peer nodes on write. Remote nodes discover entries only when a peer issues a remote lookup.

The put flow moves a GPU tensor (cache block) from client GPU memory into a DRAM memory-tier pool, then asynchronously persists it to SSD via write-through. The entry is immediately available for lookups from DRAM (both local and from remote peers via RemoteRequestHandler).

## Assumptions and Invariants

- **Cache block sizes are variable.** Each entry has its own size (recorded in the dispatch-map entry and the extent metadata). The memory-tier uses a first-fit free-list allocator with 4 KiB alignment.
- **Single dispatcher process per node.** One certus-server process handles all client requests on this node. Remote cooperation is between nodes, not within a node.
- **No ordering guarantees across keys.** Puts to different keys are fully independent.
- **Cache semantics.** Memory-tier data is volatile. A crash loses in-flight data.
- **No remote propagation on put.** A populate on this node does not notify peers. Peers discover entries via remote lookup.

## Put Flow

1. **Client submits request via gRPC.** The client sends the key and an IPC handle (64-byte CUDA IPC memory handle + size) in a BatchPopulateRequest. The server opens the IPC handle via `cudaIpcOpenMemHandle`.

2. **Memory-tier eviction (if needed).** If the memory-tier pool lacks space, the dispatcher evicts LRU entries whose write-through has completed. Evicted entries transition from MemoryTier to BlockDevice in the dispatch-map.

3. **Memory-tier slot allocation.** The dispatcher allocates a slot from the memory-tier's mmap'd DRAM pool (first-fit, 4 KiB aligned). The pool is pre-registered with CUDA via `cudaHostRegister`.

4. **GPU → DRAM DMA.** `cudaMemcpy` (D2H) from the client's GPU memory into the memory-tier slot.

5. **Dispatch-map registration.** The entry is atomically registered as a MemoryTier entry, acquiring a write reference. The entry is now visible to local lookups AND incoming remote requests via RemoteRequestHandler.

6. **Client receives acknowledgement.** The gRPC response is returned.

7. **Write reference downgrade.** Downgraded to read reference for the background writer.

8. **Background write-through (async).** The ParallelBackgroundWriter persists to SSD via the target drive's extent manager. On success, `convert_to_storage` records the ssd_offset.

9. **Entry is now durable.** Exists in both DRAM and SSD.

## Split-Populate API (reserve_memory / populate_memory / memory_populated)

Same as `full` profile:
1. `reserve_memory(key, size)` — Reserve DRAM slot
2. `populate_memory(key, ipc_handle)` — DMA into reserved slot
3. `memory_populated(key, size)` — Finalize: register + enqueue write-through
4. `release_memory(key)` — Cancel without populating

## Remote Visibility

Once step 5 completes (dispatch-map registration), the entry is visible to:
- Local lookups via gRPC
- Remote lookups from peer nodes via RemoteRequestHandler

The RemoteRequestHandler resolves keys through the dispatcher, so any entry in the dispatch-map (MemoryTier or BlockDevice state) is accessible to peers.

## Duplicate Key Handling

Same as `full`: AlreadyExists returned. Client must `remove` before re-populating.

## Eviction

- **DRAM eviction:** LRU-based. Entries pinned by remote LookupRefs (from RemoteRequestHandler) are not evictable until `release_lookup` is called.
- **SSD eviction:** Threshold-based background evictor. Entries removed from SSD are no longer visible to remote peers.

## Crash Recovery

Same as `full` profile. On restart, dispatch-map rebuilt from finalized extents. Remote peers will get KeyNotFound for entries that were DRAM-only (not yet written through). Cluster membership must be re-established via `join_cluster`.
