# Design for the cache 'get' / 'lookup' direction (`full-p2p`)

## Overview

The get/lookup flow serves a cached block from either DRAM (memory-tier) or SSD into client-provided GPU memory. The dispatch-map entry determines the data source: if the block is DRAM-resident (**warm path**), the transfer is an async memcpy from pinned memory directly to GPU; if SSD-resident (**cold path**), the `full-p2p` profile serves it over a **GPUDirect Storage** pipeline — the NVMe controller DMAs the block into a GPU BAR1 staging ring and a device-to-device copy delivers it into the client's VRAM, with no host DRAM bounce.

This cold path is the defining difference between `full-p2p` and `full`. In the
standard `full` profile the cold path promotes the block back into DRAM
(NVMe→DRAM, then DRAM→GPU). In `full-p2p` the entry stays on SSD and DRAM
promotion happens asynchronously afterward.

## Assumptions and Invariants

- **Cache block sizes are variable.** Each entry has its own size recorded in the dispatch-map. The client provides an IPC handle with a size field; the dispatcher uses the minimum of the stored size and the handle size for the copy.
- **Read references protect in-flight reads.** While a lookup is in-flight, the reader holds a read reference in the dispatch-map. Eviction skips entries with active references. Removes fail with ActiveReferences if readers are present. The P2P cold path holds a read pin on the `BlockDevice` entry for the whole SSD→BAR1→GPU pipeline.
- **Concurrent lookups are parallel.** Multiple clients can issue lookups for the same key simultaneously. Each takes an independent read reference and triggers its own DMA transfer.
- **Cache miss is client-handled.** On a miss, the dispatcher returns KeyNotFound immediately; there is no transparent fill-from-source.
- **P2P cold path is single-region.** The P2P pipeline serves a block into a single contiguous GPU destination region. A multi-region lookup is rejected with `InvalidParameter` (or, where a DRAM fallback is available, served through the DRAM-bounce path).
- **GPUDirect required.** The BAR1 ring depends on GPU BAR1 memory being GDRCopy-mapped and SPDK-registered. If the ring is unavailable, the dispatcher falls back to the DRAM-bounce cold path.

## Lookup Flow

### Warm Path (MemoryTier → GPU)

Identical to the `full` profile.

1. **Client submits lookup request via shmq.** The client writes the key and an IPC handle (64-byte CUDA IPC memory handle + size) into the `/dev/shm` mailbox as a Lookup op (HandleBatch framing). The server opens the IPC handle via `cudaIpcOpenMemHandle` (cached within the batch to avoid repeated open/close for shared handles).

2. **Dispatch-map lookup.** The dispatcher looks up the key in the dispatch-map, which atomically takes a read reference if the entry exists.

3. **Cache miss → immediate return.** If the key is not present, the dispatcher returns KeyNotFound.

4. **Size mismatch → reject.** If the entry reports MismatchSize, the read reference is released and an error is returned.

5. **MemoryTier hit → async DMA.** The dispatcher retrieves the memory-tier pointer from the dispatch-map result. Using the dedicated `warm_stream` (a pre-created CUDA stream), it issues `cudaMemcpyAsync` (H2D) from the pinned memory-tier slot directly to the client's GPU destination. This is zero-copy — no intermediate buffer is needed because the memory-tier pool is registered with CUDA.

6. **Release read reference, touch LRU.** The read reference is released and the memory-tier's LRU tracker is touched for this key (refreshing its eviction timestamp).

7. **Return stream handle.** The async stream handle is returned so the shmq serve layer can synchronize before responding. The client's GPU memory is valid after stream completion.

### Cold Path — P2P (BlockDevice → GPU BAR1 ring → GPU VRAM)

1. **Steps 1–4 are identical** to the warm path. The dispatch-map lookup returns a `BlockDevice { offset }` result and the reader holds a read pin.

2. **Schedule on the cold-read pool.** The lookup is handed to the `P2pColdReadPool`, which selects the per-drive worker for `key % num_drives`. That worker owns a pre-connected NVMe channel and a CUDA stream, so no per-request setup is needed.

3. **NVMe → GPU BAR1 ring.** The worker issues chunked NVMe reads (MDTS granularity) that DMA **directly into free slots of the GPU BAR1 staging ring** (`P2P_RING_SLOTS = 64`). The ring is GPU device memory exposed on the PCIe BAR, GDRCopy-mapped on the host and registered with SPDK so the NVMe controller can target it as a DMA destination.

4. **BAR1 ring → client VRAM (device-to-device).** As each ring slot fills, the worker issues a device-to-device copy from the BAR1 slot into the client's destination GPU region on its CUDA stream. Chunks pipeline: while one slot streams D2D to the client, the next NVMe read fills another slot. NVMe I/O and GPU D2D overlap, and the data never touches host DRAM — a single PCIe hop from SSD to GPU.

5. **Entry stays BlockDevice.** The P2P serve does **not** re-register the entry as MemoryTier. The read pin is released when the pipeline completes.

6. **Queue DRAM backfill.** The worker enqueues a `DramBackfillJob` for the key. After `backfill_delay_ms`, the `DramBackfillWorker` reads the block SSD→DRAM, inserts a memory-tier slot, and calls `promote_block_to_memory_tier(key, ptr, size)` to flip the entry to MemoryTier in place (the `ssd_offset` is retained). Subsequent lookups then take the warm path.

7. **GPU transfer complete.** After all chunks have streamed to the client, the transfer is done and the stream handle is returned.

### Cold Path — DRAM fallback

When the P2P ring is unavailable, or the lookup requires a DRAM-bounce (e.g. a
promotion request), the dispatcher uses the retained `PipelineRing`: chunked NVMe
reads land in CUDA-pinned, SPDK-registered host buffers, are copied into a
memory-tier slot, and streamed H2D to the client; the entry is then promoted to
MemoryTier via `promote_block_to_memory_tier`. This mirrors the standard `full`
cold path.

## Concurrent Promotion Race

If two lookups for the same cold key race, or a lookup races the
`DramBackfillWorker`, only one promotion can win the memory-tier `insert`. The
loser observes `AlreadyExists`, waits briefly for the winner to publish the
`MemoryTier` entry, and then serves the block warm (DRAM → GPU) rather than
issuing a duplicate promotion.

## Batch Lookup and Pre-Promotion

**`batch_lookup(entries)`** accepts multiple `(CacheKey, Vec<IpcHandle>)` pairs and processes them concurrently. Cold (BlockDevice) entries are served in parallel across per-drive cold-pool workers to exploit multi-drive bandwidth. Each cold entry is served over the P2P path when it is single-region; a multi-region entry is rejected with `InvalidParameter`. Returns one Result per entry in the same order as the input.

**`promote_to_memory_tier(keys)`** pre-promotes cold entries to DRAM (via the DRAM-bounce path) without performing any GPU DMA — useful for warming the cache ahead of anticipated lookups so subsequent lookups hit the warm path.

## Interaction with Put and Eviction

- **Concurrent populate for same key:** rejected by the dispatch-map (AlreadyExists). No conflict with in-flight readers.
- **DRAM eviction during lookup:** eviction skips entries with non-zero read references. A reader (warm or P2P cold) holding a reference prevents its entry from being evicted or demoted.
- **Remove during lookup:** the remove call fails with ActiveReferences if any reader holds a ref. The client can retry after the lookup completes.
- **Background write-through during lookup:** write-through holds a read ref, coexisting with lookup read refs. Both access the same memory-tier slot safely (read-only from their perspective).
- **Backfill vs. eviction:** the `DramBackfillWorker`'s promotion competes with the `MemoryTierEvictor`'s demotion; both operate through the dispatch-map's reference counting, so a pinned or actively-read entry is never demoted out from under a reader.

## Observability

- **Hit/miss ratio** — counter of cache hits (MemoryTier + BlockDevice) vs. misses.
- **Warm hit ratio** — fraction of hits served from memory-tier (no SSD I/O).
- **Lookup latency (warm)** — histogram of end-to-end latency for MemoryTier hits.
- **Lookup latency (cold/P2P)** — histogram of end-to-end latency for BlockDevice P2P serves.
- **DMA transfer latency** — histogram of GPU transfer times, broken down by warm (H2D) vs. cold (D2D) path.
- **P2P serve rate** — counter of P2P cold serves per second.
- **DRAM backfill rate** — counter of `promote_block_to_memory_tier` promotions completed by the backfill worker.
- **Multi-region rejections** — counter of cold lookups rejected because they were not single-region.
- **Size mismatch rejections** — counter of lookups rejected due to size mismatch.
- **Lookup throughput** — counter of successful lookup completions per second.
- **BAR1 ring utilization** — gauge of concurrent slots in flight in the P2P staging ring.
