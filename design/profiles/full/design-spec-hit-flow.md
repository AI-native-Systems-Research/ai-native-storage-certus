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

1. **Client submits lookup request via shmq.** The client writes the key and an IPC handle (64-byte CUDA IPC memory handle + size) into the `/dev/shm` mailbox as a Lookup op (HandleBatch framing). The server opens the IPC handle via `cudaIpcOpenMemHandle` (cached within the batch to avoid repeated open/close for shared handles).

2. **Dispatch-map lookup.** The dispatcher looks up the key in the dispatch-map, which atomically takes a read reference if the entry exists.

3. **Cache miss → immediate return.** If the key is not present, the dispatcher returns KeyNotFound.

4. **Size mismatch → reject.** If the entry reports MismatchSize, the read reference is released and an error is returned.

5. **MemoryTier hit → async DMA.** The dispatcher retrieves the memory-tier pointer from the dispatch-map result. Using the dedicated `warm_stream` (a pre-created CUDA stream), it issues `cudaMemcpyAsync` (H2D) from the pinned memory-tier slot directly to the client's GPU destination. This is zero-copy — no intermediate buffer is needed because the memory-tier pool is registered with CUDA.

6. **Release read reference, touch LRU.** The read reference is released and the memory-tier's LRU tracker is touched for this key (refreshing its eviction timestamp).

7. **Return stream handle.** The async stream handle is returned so the shmq serve layer can synchronize before responding. The client's GPU memory is valid after stream completion.

### Cold Path (BlockDevice → MemoryTier → GPU)

1. **Steps 1–4 are identical** to the warm path. The dispatch-map lookup returns a `BlockDevice { offset }` result and the reader holds a read pin for the whole SSD→DRAM→GPU pipeline.

2. **Schedule on the cold-read pool.** The lookup is handed to the `ColdReadPool`, which selects the per-drive worker for `key % num_drives`. That worker owns a pre-connected NVMe channel and a CUDA stream, so no per-request setup is needed.

3. **Allocate memory-tier slot.** A slot is allocated in the memory-tier for the promoted entry, running inline eviction first if the pool is full (LRU entries with completed write-through are demoted to BlockDevice via `try_evict_to_block`).

4. **Pipelined SSD → DRAM → GPU read.** Using the PipelineRing's StagingPool (pre-allocated CUDA-pinned, SPDK-registered DMA buffers with per-device pipe CUDA streams), the worker issues chunked NVMe reads at MDTS granularity. Each completed chunk copies into the memory-tier slot and streams H2D to GPU, overlapping NVMe I/O with GPU DMA:
   - NVMe read completes into a staging buffer → copy to memory-tier slot
   - Simultaneously: previous chunk streams H2D to GPU via the async CUDA stream

5. **Promote in place.** Once the block is resident in DRAM, the dispatcher calls `promote_block_to_memory_tier(key, ptr, size)`, which flips the dispatch-map entry from BlockDevice to MemoryTier **in place**, retaining the `ssd_offset` (data remains on SSD for durability). The transition works even on pinned entries and does not disturb the held read pin.

6. **Release pin, GPU transfer complete.** After all chunks have streamed to GPU, the read pin is released and the transfer is done. The client's GPU memory holds the full block; subsequent lookups take the warm path.

### Tier-Saturation Fallback (serve_cold_staged)

If the memory tier cannot allocate a slot even after inline eviction (e.g. every resident entry is pinned or actively referenced), the dispatcher falls back to `serve_cold_staged`: it streams the block SSD → staging → GPU through the PipelineRing **without** promoting it. The entry stays `BlockDevice`, so the next cold lookup repeats the staged serve rather than benefiting from a warm entry. This keeps lookups making progress under memory-tier pressure instead of failing.

### Concurrent Promotion Race

If two lookups for the same cold key race, only one promotion can win the in-place `promote_block_to_memory_tier`. The loser observes the entry already `MemoryTier` (or an `AlreadyExists`/state-changed result), and serves the block warm (DRAM → GPU) rather than issuing a duplicate promotion.

## Batch Lookup and Pre-Promotion

**`batch_lookup(entries)`** accepts multiple (key, IpcHandle) pairs and processes them concurrently. For entries on SSD (cold path), promotions run in parallel to exploit multi-drive bandwidth. Returns one Result per entry in the same order as the input.

**`promote_to_memory_tier(keys)`** pre-promotes cold entries to DRAM without performing any GPU DMA. This is useful for warming the cache ahead of anticipated lookups — subsequent lookups will hit the warm path.

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
