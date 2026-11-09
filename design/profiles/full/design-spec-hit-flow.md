# Design for the cache 'get' / 'lookup' direction

## Overview

The get/lookup flow serves a cached block from either DRAM (memory-tier) or SSD into client-provided GPU memory. The dispatch-map entry determines the data source: if the block is DRAM-resident (warm path), the transfer is an async memcpy from pinned memory directly to GPU; if SSD-resident (cold path), the block is promoted back into the memory-tier via pipelined NVMe reads before being copied to GPU.

## Assumptions and Invariants

- **Cache block sizes are variable.** Each entry has its own size recorded in the dispatch-map. The client provides an IPC handle with a size field; the dispatcher uses the minimum of the stored size and the handle size for the copy.
- **Read references protect in-flight reads.** While a lookup is in-flight, the reader holds a read reference in the dispatch-map. Eviction skips entries with active references. Removes fail with ActiveReferences if readers are present.
- **Concurrent lookups are parallel.** Multiple clients can issue lookups for the same key simultaneously. Each takes an independent read reference and triggers its own DMA transfer.
- **Cache miss is client-handled.** On a miss, the dispatcher returns KeyNotFound immediately; there is no transparent fill-from-source.
- **No P2P DMA.** SSD→GPU transfers always pass through DRAM (memory-tier). The cold path promotes the entry back to DRAM, making subsequent lookups warm.
- **Every key is attributed.** `batch_lookup` returns a `LookupOutcome { served_by, result }` per key. `served_by` is the route (`Dram`, `Ssd`, `Remote`, `Miss`, `SizeMismatch`, `Error`), and `served_by.is_hit()` holds exactly when `result.is_ok()`.
- **Multi-region blocks.** An entry may carry N per-layer GPU regions (vLLM 0.23+). The one DRAM slot is scattered back to them in order (region L ← slot + Σ preceding sizes).

## Lookup Flow

### Warm Path (MemoryTier → GPU)

1. **Client submits lookup request via shmq.** The client writes the key and its IPC handles (one 64-byte CUDA IPC memory handle + size per layer region) into the `/dev/shm` mailbox as a Lookup op (HandleBatch framing). The server opens each IPC handle via `cudaIpcOpenMemHandle` through a mapping cache (shared handles are opened once; expired mappings are closed by the reaper). Entries whose handles fail to open are reported not-served and counted as errors.

2. **Dispatch-map lookup.** The dispatcher looks up the key in the dispatch-map, which atomically takes a read reference if the entry exists. A lookup that times out waiting on a transient write reference is retried a few times rather than treated as a miss — misclassifying a live key as absent would forward it to remote lookup.

3. **Cache miss → immediate return.** If the key is not present, the dispatcher returns KeyNotFound.

4. **Size mismatch → reject.** If the entry reports MismatchSize, the read reference is released and an error is returned.

5. **MemoryTier hit → async DMA.** The dispatcher retrieves the memory-tier pointer from the dispatch-map result (and, with `integrity-check`, verifies the slot's CRC-32 first). Using the per-device warm stream (the stream bound to the destination GPU, so tensor-parallel ranks each use a device-correct stream), it issues `cudaMemcpyAsync` (H2D) from the pinned memory-tier slot to each GPU region. This is zero-copy — the memory-tier pool is registered with CUDA.

6. **Deferred sync, then release.** In `batch_lookup` the warm copies are only queued; the read pins are kept in `warm_pins`. The memory-tier LRU is batch-touched, the cold scatter-gather (below) runs while the warm H2D drains, and **then** the warm stream is synchronized once and the pins released. Releasing at submission would let the evictor reclaim a slot under an in-flight DMA.

7. **Reply.** The key is attributed `ServedBy::Dram`; the shmq response byte is 1. The client's GPU memory is valid when the response arrives. (Single-key `lookup_async` instead returns the stream handle for the caller to synchronize.)

### Cold Path (BlockDevice → MemoryTier → GPU)

1. **Steps 1–4 are identical** to the warm path. The dispatch-map lookup returns a `BlockDevice { offset }` result and the reader holds a read pin for the whole SSD→DRAM→GPU pipeline.

2. **Group by drive.** Cold entries are grouped by target drive, `splitmix64(key) % num_drives` — the same placement function the write path used.

3. **Allocate memory-tier slots.** For each entry, `evict_and_insert` allocates a slot for the promoted block, running inline eviction first if the pool is full (LRU entries with completed write-through are demoted to BlockDevice via `try_evict_to_block`). An entry that cannot get a slot (`AllocationFailed`) is deferred to the staging post-pass; one that hits `AlreadyExists` is deferred to the concurrent-promotion recovery below.

4. **Scatter-gather SSD → DRAM → GPU read.** One NVMe channel is checked out from each active drive's ChannelPool and a `DriveWork` built per drive. A single `scatter_gather_multi_drive_zero_copy` loop on the request's worker thread then fans `ReadAsync`s across all drives at once (total queue depth 128, split by segment count), with each segment (MDTS-sized) DMA'd by NVMe **directly into the memory-tier slot** — zero-copy, since the pool is SPDK-registered. Completions are reaped continuously; when an object's last segment lands its fused H2D is issued immediately on the device's pipe stream pair, so NVMe I/O and GPU DMA overlap. One final dual stream sync covers the batch.

5. **Promote in place.** For each successful job the dispatcher calls `promote_block_to_memory_tier(key, ptr, size)`, which flips the dispatch-map entry from BlockDevice to MemoryTier **in place**, retaining the `ssd_offset` (data remains on SSD for durability). The transition works even on pinned entries and does not disturb the held read pin. Multi-region entries are then scattered to their N GPU regions from the slot.

6. **Release pin, reply.** The read pin is held until the promotion completes (so a concurrent remove or SSD eviction cannot free the fresh slot mid-read), then released. The key is attributed `ServedBy::Ssd` (response byte 2) even though it is now DRAM-resident — the route, not the residency. Subsequent lookups take the warm path.

Single-key `lookup` / `lookup_async` take the same steps through `promote_and_serve`, which uses `pipelined_multi_object_zero_copy` on one drive's channel instead of the multi-drive loop.

### Tier-Saturation Fallback (serve_cold_staged)

If the memory tier cannot allocate a slot even after inline eviction (e.g. every resident entry is pinned or not yet written through), the dispatcher falls back to `serve_cold_staged` in a post-pass after the channel leases are returned: it checks out one StagingPool buffer (64 × 16 MiB CUDA-pinned, SPDK-registered by default; blocks until one is free), streams the block SSD → staging → GPU, and returns the buffer — **without** promoting it. The entry stays `BlockDevice`, so the next cold lookup repeats the staged serve rather than benefiting from a warm entry. This keeps lookups making progress under memory-tier pressure instead of failing.

### Concurrent Promotion Race

If two lookups for the same cold key race, only one `mt.insert` can win. The loser gets `AlreadyExists` and, via `serve_concurrently_promoted`, waits for the winner's promotion to land and serves the block warm (DRAM → GPU) rather than issuing a duplicate promotion. Such a key is attributed `ServedBy::Dram`, since that is the route it took.

### Local Miss → Remote Lookup (inert in this profile)

After the local passes, keys that were not found are forwarded to the bound `IRemoteLookup`. In the `full` profile `RemoteLookupComponent` is wired but never initialized (no `init_hook`), so it returns `NotFound` for every key immediately; the key is attributed `ServedBy::Miss` and the client recomputes it. Peer serving is enabled by the `full-remote` profile.

## Batch Lookup and Pre-Promotion

**`batch_lookup(entries)`** accepts multiple `(key, Vec<IpcHandle>)` pairs. Order within the call: classify (warm copies queued) → cold slot prep → multi-drive scatter-gather → staging post-pass → deferred warm sync and pin release → remote forward → reply. Returns one `LookupOutcome` per entry in the same order as the input.

**`promote_to_memory_tier(keys)`** pre-promotes cold entries to DRAM without performing any GPU DMA (via `pipelined_multi_ssd_to_dram_only`). The shmq `Touch`, `TouchCheck` and `Pin` ops carry a `promote` flag that triggers it. This is useful for warming the cache ahead of anticipated lookups — subsequent lookups will hit the warm path.

## Interaction with Put and Eviction

- **Concurrent populate for same key:** Rejected by the dispatch-map (AlreadyExists). No conflict with in-flight readers.
- **DRAM eviction during lookup:** Eviction skips entries with non-zero read references. A reader holding a reference prevents its entry from being evicted.
- **Remove during lookup:** The remove call fails with ActiveReferences if any reader holds a ref. The client can retry after the lookup completes.
- **Background write-through during lookup:** Write-through holds a read ref, coexisting with lookup read refs. Both access the same memory-tier slot safely (read-only from their perspective).

## Observability

- **Hit/miss ratio** — `lookup_hits_dram + lookup_hits_ssd + remote_lookup_hits` vs. misses; these three partition every served key by route and agree with the per-key `served_by`.
- **Warm hit ratio** — `lookup_hits_dram` as a fraction of hits (no SSD I/O).
- **Lookup latency (warm)** — histogram of end-to-end latency for MemoryTier hits.
- **Lookup latency (cold)** — histogram of end-to-end latency for BlockDevice promotions.
- **DMA transfer latency** — histogram of GPU DMA times (H2D), broken down by warm vs. cold path.
- **Promotion rate** — counter of cold-path promotions per second.
- **Size mismatch rejections** — counter of lookups rejected due to size mismatch.
- **Lookup throughput** — counter of successful lookup completions per second.
- **Cold-pipeline timings** — `PipelineMetrics` records SSD read, GPU DMA and stream-sync time per cold batch (Prometheus via `--metrics-port`); `pipeline-telemetry` adds per-batch logs.
