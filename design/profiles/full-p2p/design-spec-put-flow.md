# Design for the cache 'put' / 'populate' direction (`full-p2p`)

## Overview

The put flow moves a GPU tensor (cache block) from client GPU memory into a DRAM memory-tier pool, then asynchronously persists it to SSD via write-through. The entry is immediately available for lookups from DRAM; the SSD copy provides durability. This is a **cache** — the source of truth lives elsewhere, so data loss on crash is acceptable. On restart, the dispatch-map is rebuilt by iterating over finalized extents in the extent manager.

**The put path in `full-p2p` is identical to the `full` profile.** The P2P
BAR1 machinery only changes the cold *read* path (see `design-spec-hit-flow.md`);
writes still land in DRAM and are written through to SSD in the background.

## Assumptions and Invariants

- **Cache block sizes are variable.** Each entry has its own size (recorded in the dispatch-map entry and the extent metadata). The memory-tier uses a first-fit free-list allocator with 4 KiB alignment to handle variable sizes.
- **Single dispatcher process.** One certus-server process handles all client requests. No sharding or multi-instance coordination.
- **No ordering guarantees across keys.** Puts to different keys are fully independent and may reach SSD in any order. Puts to the same key are rejected (AlreadyExists) — the client must remove before re-populating.
- **Cache semantics.** Memory-tier data is volatile. A crash before write-through completes loses the block; this is acceptable because the data is recoverable from the original source.
- **Memory-tier pool sizing.** The pool must be large enough to absorb the working set. When full, eviction frees slots — entries that have completed write-through are eligible for eviction (dispatch-map transitions from MemoryTier to BlockDevice).

## Put Flow

1. **Client submits request via shmq.** The client writes the key and an IPC handle (64-byte CUDA IPC memory handle + size) into the certus-server's `/dev/shm` mailbox as a Populate op (HandleBatch framing). The server opens the IPC handle via `cudaIpcOpenMemHandle` to obtain a device pointer in its own CUDA context.

2. **Memory-tier eviction (if needed).** If the memory-tier pool lacks space for the new entry, the dispatcher evicts entries (chosen by the bound eviction policy) whose write-through has completed (`ssd_offset` is set). Evicted entries transition from MemoryTier to BlockDevice in the dispatch-map — their data remains on SSD. If nothing is evictable (all entries are still writing through), the put fails with AllocationFailed.

3. **Memory-tier slot allocation (`reserve_memory`).** The dispatcher allocates a slot from the memory-tier's mmap'd DRAM pool (first-fit, 4 KiB aligned). The pool is pre-registered with CUDA via `cudaHostRegister` for zero-copy DMA. The slot is not yet visible in the dispatch-map.

4. **GPU → DRAM DMA (`copy_gpu_to_memory_async`).** The dispatcher issues an async D2H copy from the client's GPU region(s) into the reserved memory-tier slot, on the per-drive `store` CUDA stream, then synchronizes. The memory-tier pool is CUDA-pinned so the copy is zero-copy on the host side.

5. **Dispatch-map registration (`copy_gpu_to_memory_completed`).** The dispatcher atomically registers the entry in the dispatch-map as a MemoryTier entry (key → pointer + size), acquiring a write reference. The entry is now visible to `check` and `lookup` requests.

6. **Client receives acknowledgement.** The shmq response is written back to the mailbox channel. From the client's perspective the put is complete. The CUDA IPC handle is closed.

7. **Write reference downgrade.** The write reference is downgraded to a read reference, allowing concurrent lookups while the background writer holds a ref.

8. **Background write-through (async).** The ParallelBackgroundWriter picks up a WriteJob:
   - Retrieves the memory-tier pointer for the key.
   - Wraps the memory-tier slot as a zero-copy DMA buffer (no data copy).
   - Allocates a contiguous extent via the target drive's extent manager (`reserve_extent` → WriteHandle).
   - Writes to SSD using MDTS-aware segmented I/O (splits into max-transfer-size chunks).
   - On success: publishes the extent (marks finalized) and calls `convert_to_storage` to record the `ssd_offset` in the dispatch-map. **The entry stays MemoryTier** — recording the offset only makes the entry durable and evictable; it does not demote it.
   - Releases the read reference.

9. **Entry is now durable.** The entry exists in both DRAM (memory-tier) and SSD. Lookups are served from DRAM (warm path). If later evicted from DRAM, lookups are served over the P2P cold path (SSD → GPU BAR1 ring → VRAM).

## Split-Populate API (reserve_memory / copy_gpu_to_memory_async / copy_gpu_to_memory_completed)

The single-call `populate` is implemented on top of a split path that separates reservation from DMA:

1. **`reserve_memory(key, size, session_id)`** — Reserves a DRAM slot in the memory-tier (runs eviction if needed). Returns a raw pointer to the allocated slot. Does NOT register in the dispatch-map. `session_id` is an opaque per-request identifier (0 = unset) supplied by the client for observability only; it has no allocation semantics.
2. **`copy_gpu_to_memory_async(key, regions, stream)`** — Issues an async D2H copy of the client GPU region(s) into the previously reserved slot on the given stream.
3. **`copy_gpu_to_memory_completed(key, size)`** — Finalizes the entry: registers it in the dispatch-map (as MemoryTier) and enqueues background write-through (equivalent to steps 5–9 above).
4. **`release_memory(key)`** — Cancellation path: frees the reserved slot without populating.

This API enables the shmq serve layer to overlap reservation with other batch work, and supports external DMA engines that populate the slot outside the dispatcher's control. `batch_populate` applies the same sequence across a batch of keys.

## Duplicate Key Handling

A put for an existing key returns `AlreadyExists`. The client must explicitly `remove` the key before re-populating. This avoids the complexity of in-place replacement with in-flight readers.

## Interaction with Lookup During Write-Through

A lookup arriving while background write-through is in progress finds the entry in the dispatch-map as MemoryTier and serves directly from DRAM — no coordination needed. The read/write reference system prevents the entry from being removed while a reader is active.

## Eviction

- **DRAM eviction:** delegated to the bound eviction policy. Only entries with a completed write-through (`ssd_offset` set) and no active references are eligible. The dispatch-map entry transitions from MemoryTier to BlockDevice (via `try_evict_to_block` / `convert_memory_tier_to_block`); subsequent lookups use the P2P cold path.
- **Background DRAM demotion (MemoryTierEvictor):** independent of inline populate-time eviction, a background worker demotes evictable MemoryTier entries to BlockDevice when memory-tier utilization crosses a threshold, freeing DRAM without I/O (the data is already on SSD).
- **SSD eviction (BackgroundEvictor):** When SSD usage exceeds a configurable threshold, the evictor scans for oldest keys, removes their extents, and frees space. Entries evicted from SSD are fully removed from the system — a subsequent lookup returns KeyNotFound.

## Crash Recovery

On restart, the in-memory dispatch-map and memory-tier are empty. The dispatch-map is rebuilt by iterating over finalized extents in each data drive's extent manager. Each finalized extent provides key and offset metadata (recovered as BlockDevice entries). Non-finalized extents (from incomplete writes) are reclaimed as free space. DRAM-only entries (memory-tier slots not yet written through) are lost — acceptable under cache semantics.

## Observability

- **Memory-tier utilization** — gauge of used vs. total pool bytes. High utilization signals eviction pressure.
- **Eviction rate** — counter of entries evicted from memory-tier (inline + MemoryTierEvictor).
- **GPU DMA latency** — histogram of D2H transfer times (step 4).
- **Write-through latency** — histogram of end-to-end background write time (step 8).
- **Write-through queue depth** — gauge of pending WriteJobs in the BackgroundWriter channel.
- **Put throughput** — counter of successful puts per second (acknowledgements sent, step 6).
- **Dispatch-map size** — gauge of entries, broken down by location (MemoryTier vs. BlockDevice).
- **AlreadyExists rejections** — counter of puts rejected due to duplicate keys.
- **AllocationFailed errors** — counter of puts failed due to pool exhaustion after eviction.
