# Design for the cache 'put' / 'populate' direction

## Overview

> Profile `full-optimized`: the put path is identical to `full`. The only difference is how the eviction policy orders the entry it inserts — see *Eviction Ordering (TinyLFU admission)* below.

The put flow moves a GPU tensor (cache block) from client GPU memory into a DRAM memory-tier pool, then asynchronously persists it to SSD via write-through. The entry is immediately available for lookups from DRAM; the SSD copy provides durability. This is a **cache** — the source of truth lives elsewhere, so data loss on crash is acceptable. On restart, the dispatch-map is rebuilt by iterating over finalized extents in the extent manager.

## Assumptions and Invariants

- **Cache block sizes are variable.** Each entry has its own size (recorded in the dispatch-map entry and the extent metadata). The memory-tier uses a first-fit free-list allocator with 4 KiB alignment to handle variable sizes.
- **Single dispatcher process.** One certus-server process handles all client requests. No sharding or multi-instance coordination.
- **No ordering guarantees across keys.** Puts to different keys are fully independent and may reach SSD in any order. Puts to the same key are rejected (AlreadyExists) — the client must remove before re-populating.
- **Cache semantics.** Memory-tier data is volatile. A crash between steps 3–8 loses the block; this is acceptable because the data is recoverable from the original source.
- **Memory-tier pool sizing.** The pool must be large enough to absorb the working set. When full, eviction frees slots — entries that have completed write-through are eligible for eviction (dispatch-map transitions from MemoryTier to BlockDevice).
- **A full cache is never fatal to a store.** A failed store kills the vLLM engine (`assert transfer_result.success`), so a tier that stays full backpressures and then refuses or drops the store rather than erroring.

## Put Flow

1. **Client submits request via shmq.** The client writes the key and an IPC handle (64-byte CUDA IPC memory handle + size) into the certus-server's `/dev/shm` mailbox as a Populate op (HandleBatch framing). The server opens the IPC handle via `cudaIpcOpenMemHandle` to obtain a device pointer in its own CUDA context.

2. **Memory-tier eviction (if needed).** If the memory-tier pool lacks space for the new entry, the dispatcher evicts entries (chosen by the bound LRU + TinyLFU eviction policy) whose write-through has completed (ssd_offset is set). Evicted entries transition from MemoryTier to BlockDevice in the dispatch-map — their data remains on SSD — and an `EvictionEvent { Demoted }` is emitted. Pinned or not-yet-persisted candidates are skipped (never removed) and the scan widens.

   **Backpressure.** If nothing is evictable, `reserve_memory` retries every 20 ms until its deadline — `--store-backpressure-ms` (default 5 s) from the start of the call, or a deadline shared across the whole Reserve batch when called from shmq — giving the background writer time to land write-throughs. When the budget elapses, `populate` **drops** the store (returns Ok, counted in `store_drops_on_full`; a later load misses and recomputes); a shmq Reserve replies 0 for that key and the client skips caching it.

3. **Memory-tier slot allocation.** The dispatcher allocates a slot from the memory-tier's mmap'd DRAM pool (first-fit, 4 KiB aligned). The pool is pre-registered with CUDA via `cudaHostRegister` for zero-copy DMA.

4. **GPU → DRAM DMA.** The dispatcher issues `cudaMemcpyAsync` (D2H) from the client's GPU memory into the memory-tier slot on the source device's dedicated **store** stream (separate from the warm H2D stream so PCIe runs full-duplex), then synchronizes that stream. Any failure from here until step 5 releases the reserved slot, so it cannot leak.

5. **Dispatch-map registration.** The dispatcher atomically registers the entry in the dispatch-map as a MemoryTier entry (key → pointer + size), acquiring a write reference. The entry is now visible to `check` and `lookup` requests.

6. **Client receives acknowledgement.** The shmq response is written back to the mailbox channel. From the client's perspective the put is complete. The CUDA IPC handle is closed.

7. **Write reference downgrade.** The write reference is downgraded to a read reference, allowing concurrent lookups while the background writer holds a ref.

8. **Background write-through (async).** The WriteJob targets drive `splitmix64(key) % num_drives`. The ParallelBackgroundWriter picks it up:
   - Retrieves the memory-tier pointer for the key.
   - Wraps the memory-tier slot as a zero-copy DMA buffer (no data copy).
   - Allocates a contiguous extent via the target drive's extent manager.
   - Writes to SSD using MDTS-aware segmented I/O (splits into max-transfer-size chunks).
   - On success: publishes the extent (marks finalized) and calls `convert_to_storage` to record the ssd_offset in the dispatch-map.
   - Releases the read reference.

9. **Entry is now durable.** The entry exists in both DRAM (memory-tier) and SSD. Lookups are served from DRAM (warm path). If later evicted from DRAM, lookups promote from SSD (cold path).

## Split-Populate API (reserve_memory / copy_gpu_to_memory_async / copy_gpu_to_memory_completed)

An alternative to the single-call `populate` is the split-populate path, which separates reservation from DMA:

1. **`reserve_memory(key, size, session_id, deadline)`** — Reserves a DRAM slot in the memory-tier (runs eviction and backpressure as in step 2). Returns a raw pointer to the allocated slot. Does NOT register in the dispatch-map. `session_id` is an opaque per-request identifier (0 = unset) for observability only. The shmq `Reserve` op computes **one** deadline for the whole batch, so K keys backpressure for at most one budget in total rather than K budgets; keys reached after the deadline get a single attempt and fail fast. A key already resident is refused (`store_already_resident`).
2. **`copy_gpu_to_memory_async(key, regions, stream)`** — Issues one `cudaMemcpyAsync` D2H per region into the previously reserved slot; region L lands at `slot + Σ sizes[0..L]`, so a block split into N per-layer GPU allocations (vLLM 0.23+) is stored as one colocated slot. A region set larger than the slot is rejected. With a null stream (the shmq `CopyToStore` path) the copy runs on the device's store stream and is synchronized before returning.
3. **`copy_gpu_to_memory_completed(key, size)`** — Called after the copy completes; finalizes the entry: registers in the dispatch-map (computing and attaching a CRC-32 when built with `integrity-check`), downgrades the write ref, and enqueues background write-through (steps 5–9). On `AlreadyExists` the slot is freed only if the existing entry provably points elsewhere.
4. **`release_memory(key)`** — Cancellation path (shmq `AbortStore`): frees the reserved slot without populating. Idempotent. Reservations never committed or aborted are reclaimed by the `shmq-reaper` after `--reserve-timeout-secs`.

This is the path the vLLM connector uses (Reserve → CopyToStore → CommitStore). It lets the shmq layer overlap reservation and the D2H copy with other batch work. **`batch_populate(entries)`** applies the single-call path across multiple `(key, IpcHandle)` pairs at once.

## Eviction Ordering (TinyLFU admission)

When step 3 inserts the slot, memory-tier calls `IEvictionPolicy::track`; when step 5 registers the entry, dispatch-map does the same in its own pool. In `eviction-policy-optimized`, `track` increments the key's Count-Min Sketch count and compares it with the count of the pool's current LRU head:

- **Not more popular than the head** → the new entry is placed at the **LRU front**, i.e. it is the next eviction candidate. A one-off block stored once does not displace blocks that keep being re-stored.
- **More popular** → placed at the MRU end, as plain LRU would.

The count tracks how often the key has been *stored* (after a `remove`, a re-store increments it again); lookups and `touch` move the entry to the MRU end but do not change the count. The sketch is halved every `10 × max_len` tracks so stale popularity decays.

Placement at the front does not make an entry evictable early: until its write-through lands (step 8) it has no `ssd_offset`, and `evict_one_clean` skips it. Under sustained store bursts fresh, low-frequency entries therefore cluster unpersisted at the front of the eviction window (the `oldest_sampled` / `oldest_persisted` gauges show this). `evict_for_space` handles it by retrying a scan that frees nothing once at full depth (`EVICT_SCAN_MAX` = 1024 candidates), which reaches the persisted entries behind that front.

## Duplicate Key Handling

A put for an existing key returns `AlreadyExists`. The client must explicitly `remove` the key before re-populating. This avoids the complexity of in-place replacement with in-flight readers.

## Interaction with Lookup During Write-Through

A lookup arriving while background write-through is in progress finds the entry in the dispatch-map as MemoryTier and serves directly from DRAM — no coordination needed. The read/write reference system prevents the entry from being removed while a reader is active.

## Eviction

- **DRAM eviction (inline):** on a failed slot allocation the dispatcher evicts synchronously, delegated to the bound eviction policy. Only entries with a completed write-through (ssd_offset set) and no active references are eligible. The dispatch-map entry transitions from MemoryTier to BlockDevice via `try_evict_to_block`; subsequent lookups use the cold path.
- **DRAM demotion (background MemoryTierEvictor):** when enabled with `--memory-tier-eviction-threshold` > 0 (off by default), a dedicated background thread proactively demotes cold DRAM entries to SSD-only above the threshold, down to a 70% low-water mark, using the same `try_evict_to_block` transition (data already durable on SSD). This keeps headroom for incoming puts without stalling them on inline eviction.
- **SSD eviction (BackgroundEvictor):** When SSD usage exceeds a configurable threshold (90%), the evictor scans for oldest keys and, for each, removes the dispatch-map entry first (refused if pinned), then frees the DRAM slot and the extent on the same splitmix64-selected drive, and emits `EvictionEvent { Removed }`. Entries evicted from SSD are fully removed from the system — a subsequent lookup returns KeyNotFound.

## Crash Recovery

On restart (without `--format`), the in-memory dispatch-map and memory-tier are empty. Each drive's GPT is read back to locate its metadata and data partitions, and the dispatch-map is rebuilt by iterating over finalized extents in each data drive's extent manager (`recover_extent`). Each finalized extent provides key and offset metadata. Non-finalized extents (from incomplete writes) are reclaimed as free space. DRAM-only entries (memory-tier slots not yet written through) are lost — acceptable under cache semantics.

## Observability

- **Memory-tier utilization** — gauge of used vs. total pool bytes. High utilization signals eviction pressure.
- **Eviction rate** — counter of entries evicted from memory-tier.
- **GPU DMA latency** — histogram of D2H transfer times (step 4).
- **Write-through latency** — histogram of end-to-end background write time (steps 8).
- **Write-through queue depth** — gauge of pending WriteJobs in the BackgroundWriter channel.
- **Put throughput** — counter of successful puts per second (acknowledgements sent, step 6).
- **Dispatch-map size** — gauge of entries, broken down by location (MemoryTier vs. BlockDevice).
- **AlreadyExists rejections** — counter of puts rejected due to duplicate keys.
- **AllocationFailed errors** — counter of puts failed due to pool exhaustion after eviction.
- **Store backpressure / drops** — `store_backpressure_events` (retries taken on a full tier) and `store_drops_on_full` (stores dropped after the budget), from `tier_event_stats()`.
- **Already-resident refusals** — `store_already_resident`: reserves refused because the key was already in the tier (not a capacity problem).
- **Why eviction failed** — `evictions_blocked_by_pin`, `evictions_blocked_unpersisted`, `eviction_scans_exhausted`, and the `oldest_sampled` / `oldest_persisted` gauges over the eviction window.
