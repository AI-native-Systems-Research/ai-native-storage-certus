# Drift Report: Dispatcher Cache Interface (001-dispatcher-cache-interface)

**Generated**: 2026-05-29  
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`  
**Implementation**: `components/dispatcher/src/` (lib.rs, pipeline.rs, io_segmenter.rs, background.rs)  
**Interface**: `components/interfaces/src/idispatcher.rs`  
**Prior run**: 2026-05-28 (this run reflects incremental analysis)

## Summary

| Status | Count |
|--------|-------|
| Aligned | 31 |
| Drifted (behavioral) | 3 |
| Drifted (omission from spec) | 4 |
| Unspecced features | 1 |

All spec requirements are implemented. No missing features. Several behavioral and documentation gaps exist between spec and code.

---

## Critical / Behavioral Drift

### DRIFT-A: `evict_for_space` algorithm diverges significantly from spec description

**Spec refs**: FR-024, User Story 7, Clarification 2026-05-22, Edge Cases  
**Severity**: Significant — the spec's stated algorithm is materially different from the code

**Spec says** (FR-024 and clarifications):
- Phase 1: query `oldest_keys(MAX_SCAN=128)` for LRU candidates, check each via `is_evictable(key)`, pick the first evictable entry, evict it via `mt.remove()` + `convert_memory_tier_to_block`.
- Phase 2 (fallback): only if no evictable candidate found, call `evict_lru()` blindly.
- This two-phase logic repeats until `used + needed <= capacity`.

**Implementation does** (lib.rs lines 296-348):
- Uses `MAX_SCAN=4` (not 128).
- The clean eviction probe (`oldest_keys` + `is_evictable`) only fires on every **8th iteration** (`attempts % 8 == 0`), not on every pass.
- The primary path in all other iterations immediately falls through to blind LRU.
- Adds a `MAX_ATTEMPTS=512` guard: if the loop does not free enough space in 512 iterations, returns `Err(AllocationFailed(...))`. This error case is not described in the spec.

**Code comment rationale** (lines 305-307):
> Under high concurrency (many threads promoting cold entries simultaneously), scanning many candidates per attempt causes severe MT lock contention because oldest_keys(N) holds the lock while scanning N entries. Use a tiny scan window and prefer blind LRU as the primary fast path.

**Required spec update**: FR-024 must be rewritten. The two-phase-per-iteration model is wrong. The actual algorithm prefers speed/concurrency over data-safety. The 512-attempt limit and `AllocationFailed` return from eviction must also be documented.

---

### DRIFT-B: `pipelined_ssd_to_gpu_zero_copy` uses a sliding window, not two sequential phases

**Spec refs**: FR-019, User Story 9 (acceptance scenario 1), Clarification 2026-05-08, SC-012  
**Severity**: Moderate — the spec's correctness argument relies on a model that is not how the code works

**Spec says**: Phase 1 issues up to 16 concurrent NVMe reads into **unique** offsets of the memory-tier slot, completing **all reads before Phase 2**. Phase 2 issues GPU H2D copies in segment order. The two-phase design is explicitly justified as tolerating out-of-order NVMe completions since each chunk targets a unique address.

**Implementation does** (pipeline.rs lines 244-401):
- Uses a sliding window (VecDeque `inflight`). As each NVMe read completes, the implementation **immediately** issues the GPU H2D copy for that segment AND submits the next NVMe read — in the same loop iteration. There are no two distinct phases.
- The implementation assumes FIFO NVMe queue completion order (`inflight.pop_front()`) to associate each completion with the oldest in-flight segment. Out-of-order completions would yield incorrect results (the spec's stated concern).
- Adds a periodic stream sync every 16 GPU commands to bound command queue depth — not mentioned in spec.

**Correctness note**: The sliding window implementation is faster (overlaps SSD I/O with GPU DMA) but relies on FIFO queue semantics rather than the two-phase isolation the spec describes. The spec's safety argument for out-of-order NVMe is not preserved by the implementation.

**Required spec update**: FR-019 and the clarification for `pipelined_ssd_to_gpu_zero_copy` must be rewritten to describe the sliding window algorithm and document the FIFO queue assumption.

---

### DRIFT-C: `batch_lookup` MemoryTier path uses `warm_stream` with inline sync (beyond FR-037 scope)

**Spec refs**: FR-037, FR-039  
**Severity**: Minor

**Spec says**: FR-037 states `warm_stream` is used by `lookup_async`. FR-039 says `batch_lookup` serves MemoryTier hits inline (same as single-entry lookup).

**Implementation does**: `batch_lookup` loads `warm_stream` (AtomicU64) and calls `memcpy_h2d_async` followed by `stream_synchronize` inline for each MemoryTier hit (lib.rs lines 954-975). This is consistent with the intent but the spec only attributes `warm_stream` to `lookup_async`.

**Required spec update**: FR-037 should note that `batch_lookup` also uses `warm_stream` for MemoryTier hits to avoid the DmaBuffer wrapping overhead.

---

## Omission Drift (features in code not documented in spec)

### DRIFT-D: Dispatch-map recovery on `format_on_init=false` is unspecified

**Spec ref**: FR-025  
**Severity**: Moderate — this is significant operator-facing behavior

**Spec says** (FR-025): "When false, extent managers are not reformatted on initialization, preserving on-disk data from previous sessions." No further recovery behavior is described.

**Implementation does** (lib.rs lines 659-683): When `format_on_init=false`, after recovering extent managers, the code iterates all extents via `iem.for_each_extent()` and calls `dm.recover_extent(key, offset, size)` to rebuild the dispatch map from on-disk extent metadata. Timing is logged. This restores the full cache index from persisted SSD data.

**Required spec update**: FR-025 must document that the dispatch map is reconstructed from extent metadata during non-format initialization. This is a key operator-observable behavior for restart persistence scenarios.

---

### DRIFT-E: `shutdown` checkpoints extent managers (unspecified)

**Spec refs**: User Story 5 scenario 3, FR-014  
**Severity**: Minor

**Spec says**: Shutdown "drains all pending write-through jobs, block devices are shut down in reverse order, resources released."

**Implementation does** (lib.rs lines 821-833): After draining the background writer and evictor, calls `iem.checkpoint()` on each extent manager before block device teardown. This persists extent manager metadata to SSD at shutdown.

**Required spec update**: User Story 5 scenario 3 and FR-014 should mention extent manager checkpointing as part of the shutdown sequence.

---

### DRIFT-F: `evict_for_space` returns `AllocationFailed` from the eviction loop itself

**Spec refs**: FR-024, Edge Cases  
**Severity**: Minor

**Spec says** (Edge Cases): "When memory-tier insertion fails during populate (pool full after eviction attempt), populate returns an `AllocationFailed` error." The spec frames `AllocationFailed` as arising from `mt.insert()` failure, not from the eviction loop.

**Implementation does**: `evict_for_space` itself returns `Err(DispatcherError::AllocationFailed(...))` after `MAX_ATTEMPTS=512` without freeing enough space (lib.rs lines 311-314). Both code paths surface as `AllocationFailed` to the caller, but the trigger condition (eviction loop exhaustion vs. allocation failure) differs. This is related to DRIFT-A.

---

### DRIFT-G: `poller_base_cpu` config field is unspecified

**Spec refs**: FR-011, User Story 5  
**Severity**: Minor

**Spec says**: DispatcherConfig and initialization are described without mention of CPU affinity for NVMe pollers.

**Implementation does** (lib.rs lines 501-503, 612-616): `DispatcherConfig.poller_base_cpu: Option<usize>` pins each NVMe block device actor to `base + i` CPU core via `admin.set_actor_cpu()`. This is a performance-critical configuration for NUMA-aware NVMe polling.

**Required spec update**: FR-011 or a new FR should document `poller_base_cpu` and its effect on NVMe poller CPU affinity.

---

## Previously Resolved Drift (from 2026-05-21/28 run)

The following items were identified in prior runs and are confirmed resolved in the spec:

| Item | Resolution |
|------|-----------|
| FR-026 (BlockDeviceVersion) | Marked REMOVED in spec |
| FR-027 (ExtentManagerVersion) | Marked REMOVED in spec |
| FR-036 (lookup_async) | Added to spec |
| FR-037 (warm_stream) | Added to spec |
| FR-038 (clear_memory_tier) | Added to spec |
| FR-039 (batch_lookup) | Added to spec, User Story 11 added |
| US5-S4 (data_pci_addrs always required) | Clarified in spec |

---

## Items Correctly Implemented and Correctly Specified

All of the following are aligned between spec and code:

- FR-003 (populate path, evict+insert+DMA+register+enqueue)
- FR-004 (write-through uses peek(), not get())
- FR-005 (memory-tier slot not freed after write-through)
- FR-006 (lookup dispatch: MemoryTier/BlockDevice/Staging)
- FR-007 (KeyNotFound for missing entries)
- FR-008 (check — no data transfer)
- FR-009 (remove — frees MT slot, dispatch entry, SSD extent)
- FR-010 (define_component! macro usage)
- FR-011 (receptacles: logger, dispatch_map, gpu_services, spdk_env, memory_tier)
- FR-012 (validate dispatch_map and memory_tier at initialize)
- FR-013 (RwLock on dispatch map operations)
- FR-014 (shutdown drains background operations)
- FR-015 (N block devices, N extent managers)
- FR-016 (FormatParams computed from disk geometry)
- FR-017 (write-through failure silently drops job)
- FR-018 (remove does not block on write-through)
- FR-020 (prepare_store: evict, reserve extent, return DmaBuffer)
- FR-021 (commit_store: MDTS I/O, publish, convert_to_storage)
- FR-022 (cancel_store: drop PendingWrite → WriteHandle abort, remove DM entry)
- FR-023 (touch delegates to dm.touch)
- FR-028 (promote re-registers as MemoryTier with original ssd_offset)
- FR-029 (SSD evictor started only when threshold > 0.0 and drives exist)
- FR-030 (evictor interval configurable via ssd_eviction_interval_secs)
- FR-031 (evictor stops at low_watermark)
- FR-032 (evictor skips MemoryTier entries; remove failure is graceful)
- FR-033 (all evictor config fields present in DispatcherConfig)
- FR-034 (register_host_memory at init, non-fatal on failure)
- FR-035 (unregister_host_memory in shutdown before teardown)
- FR-036 (lookup delegates to lookup_async + stream_synchronize)
- FR-037 (warm_stream pre-allocated, used for lock-free MemoryTier H2D)
- FR-038 (clear_memory_tier loops evict_lru, converts/removes each entry)
- FR-039 (batch_lookup: classify, inline hot, parallel cold per drive, merge results)
