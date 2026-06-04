# Design for the cache 'get' / 'lookup' direction

## Overview

The get/lookup flow serves a cached block from either DRAM (memory-tier) or SSD into client-provided GPU memory. The dispatch-map entry determines the data source: if the block is DRAM-resident (warm path), the transfer is an async memcpy from pinned memory directly to GPU; if SSD-resident (cold path), the block is promoted back into the memory-tier via pipelined NVMe reads before being copied to GPU.

## Assumptions and Invariants

- **Cache block sizes are variable.** Each entry has its own size recorded in the dispatch-map. The client provides an IPC handle with a size field; the dispatcher uses the minimum of the stored size and the handle size for the copy.
- **Read references protect in-flight reads.** While a lookup is in-flight, the reader holds a read reference in the dispatch-map. Eviction skips entries with active references. Removes fail with ActiveReferences if readers are present.
- **Concurrent lookups are parallel.** Multiple clients can issue lookups for the same key simultaneously. Each takes an independent read reference and triggers its own DMA transfer.
- **Cache miss is client-handled.** On a miss, the dispatcher returns KeyNotFound immediately; there is no transparent fill-from-source.
- **No P2P DMA.** SSD→GPU transfers always pass through DRAM (memory-tier). The cold path promotes the entry back to DRAM, making subsequent lookups warm.

## Lookup Flow

### Warm Path (MemoryTier → GPU)

1. **Client submits lookup request via gRPC.** The client sends the key and an IPC handle (64-byte CUDA IPC memory handle + size) in a BatchLookupRequest. The server opens the IPC handle via `cudaIpcOpenMemHandle` (cached within the batch to avoid repeated open/close for shared handles).

2. **Dispatch-map lookup.** The dispatcher looks up the key in the dispatch-map, which atomically takes a read reference if the entry exists.

3. **Cache miss → immediate return.** If the key is not present, the dispatcher returns KeyNotFound.

4. **Size mismatch → reject.** If the entry reports MismatchSize, the read reference is released and an error is returned.

5. **MemoryTier hit → async DMA.** The dispatcher retrieves the memory-tier pointer from the dispatch-map result. Using the dedicated `warm_stream` (a pre-created CUDA stream), it issues `cudaMemcpyAsync` (H2D) from the pinned memory-tier slot directly to the client's GPU destination. This is zero-copy — no intermediate buffer is needed because the memory-tier pool is registered with CUDA.

6. **Release read reference, touch LRU.** The read reference is released and the memory-tier's LRU tracker is touched for this key (refreshing its eviction timestamp).

7. **Return stream handle.** The async stream handle is returned so the gRPC layer can synchronize before responding. The client's GPU memory is valid after stream completion.

### Cold Path (BlockDevice → MemoryTier → GPU)

1. **Steps 1–4 are identical** to the warm path.

2. **BlockDevice hit → promote.** The dispatch-map lookup returns a BlockDevice result with the SSD offset. The read reference is released (the entry will be re-registered during promotion).

3. **Evict if needed.** If the memory-tier pool is full, LRU entries with completed write-through are evicted to make space for the promoted entry.

4. **Allocate memory-tier slot.** A new slot is allocated in the memory-tier for the promoted entry.

5. **Pipelined SSD → DRAM read.** Using the PipelineRing (8 pre-allocated CUDA-pinned, SPDK-registered DMA buffers with 2 CUDA streams), the dispatcher issues chunked NVMe reads at MDTS granularity. Each completed chunk is available in the memory-tier slot immediately. This overlaps NVMe I/O with GPU DMA:
   - NVMe read completes into ring buffer → copy to memory-tier slot
   - Simultaneously: previous chunk streams H2D to GPU via async CUDA stream

6. **Re-register in dispatch-map.** The old BlockDevice entry is removed and a fresh MemoryTier entry is created with the new pointer. The ssd_offset is preserved (data remains on SSD for durability).

7. **GPU transfer complete.** After all chunks have been streamed to GPU, the transfer is done. The client's GPU memory holds the full block.

### Staging Path (Legacy / Prepare-Store)

If an entry is in the Staging state (from the `prepare_store`/`commit_store` direct-write API), lookup performs a synchronous `dma_copy_to_device` from the staging buffer to GPU and releases the read reference.

## Interaction with Put and Eviction

- **Concurrent populate for same key:** Rejected by the dispatch-map (AlreadyExists). No conflict with in-flight readers.
- **DRAM eviction during lookup:** Eviction skips entries with non-zero read references. A reader holding a reference prevents its entry from being evicted.
- **Remove during lookup:** The remove call fails with ActiveReferences if any reader holds a ref. The client can retry after the lookup completes.
- **Background write-through during lookup:** Write-through holds a read ref, coexisting with lookup read refs. Both access the same memory-tier slot safely (read-only from their perspective).

## Observability

- **Hit/miss ratio** — counter of cache hits (MemoryTier + BlockDevice) vs. misses.
- **Warm hit ratio** — fraction of hits served from memory-tier (no SSD I/O).
- **Lookup latency (warm)** — histogram of end-to-end latency for MemoryTier hits.
- **Lookup latency (cold)** — histogram of end-to-end latency for BlockDevice promotions.
- **DMA transfer latency** — histogram of GPU DMA times (H2D), broken down by warm vs. cold path.
- **Promotion rate** — counter of cold-path promotions per second.
- **Size mismatch rejections** — counter of lookups rejected due to size mismatch.
- **Lookup throughput** — counter of successful lookup completions per second.
- **PipelineRing utilization** — gauge of concurrent pipelined reads in flight.
