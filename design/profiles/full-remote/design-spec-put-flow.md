# Design for the cache 'put' / 'populate' direction (`full-remote` Profile)

## Overview

The put flow in the `full-remote` profile is identical to the `full` profile. Populate operations are **local-only** — data is not propagated to peer nodes on write. Remote nodes discover entries only when a peer issues a remote lookup.

The put flow moves a GPU tensor (cache block) from client GPU memory into a DRAM memory-tier pool, then asynchronously persists it to SSD via write-through. The entry is immediately available for local lookups from DRAM, and for peers that later query its key — when a peer queries, this node (the data-holder) RDMA-writes the value out via its RemoteLookupRdmaInitiator.

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

5. **Dispatch-map registration.** The entry is atomically registered as a MemoryTier entry, acquiring a write reference. The entry is now visible to local lookups AND resolvable when a peer queries its key (served by the RemoteLookupRdmaInitiator push path).

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
- Peers that query the key: on receiving a `KeyQuery`, this node resolves the value in its memory-tier and RDMA-writes it out via the RemoteLookupRdmaInitiator

Any entry resolvable in the local memory-tier (whether MemoryTier or promoted from BlockDevice state) can be served to a querying peer. Populate itself does **not** notify peers — discovery is pull-driven by peer queries.

## Duplicate Key Handling

Same as `full`: AlreadyExists returned. Client must `remove` before re-populating.

## Eviction

- **DRAM eviction:** LRU-based. Serving a peer uses one-sided RDMA WRITE out of the source entry and does not pin it, so remote traffic does not block eviction.
- **SSD eviction:** Threshold-based background evictor. Entries removed from both DRAM and SSD can no longer be served to querying peers.

## Crash Recovery

Same as `full` profile. On restart, dispatch-map rebuilt from finalized extents. Querying peers get no reply (NotFound) for keys that were DRAM-only (not yet written through). Cluster membership re-establishes automatically via Zyre discovery once the node rejoins the mesh.
