# Sync Apply Report: Dispatcher Component

**Date**: 2026-07-14  
**Operator**: speckit-sync-apply  
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`  
**Prior runs**: 2026-05-21 (5 changes), 2026-05-29 (7 changes), 2026-06-12 (2 changes)

## Summary

| Metric | Count |
|--------|-------|
| New drift items identified | 2 |
| Backfills applied to spec | 2 |
| Spec sections modified | FR-033, User Story 12 (new), FR-046–FR-050 (new), Key Entities |

---

## Backfills Applied (2026-07-14)

### BACKFILL-015: FR-033 updated to include memory-tier eviction config fields

**Drift item**: FR-033 drift (2026-07-14)  
**File**: `spec.md` FR-033  

**Before**: FR-033 listed only SSD eviction config fields: `ssd_eviction_threshold`, `ssd_eviction_low_watermark`, `ssd_eviction_batch_size`, `ssd_eviction_interval_secs`, and `max_eviction_attempts`.

**After**: FR-033 now additionally specifies: `memory_tier_eviction_threshold` (f64, default 0.0 — disabled), `memory_tier_eviction_low_watermark` (f64, default 0.70), `memory_tier_eviction_batch_size` (usize, default 64), `memory_tier_eviction_interval_secs` (u64, default 2).

---

### BACKFILL-016: User Story 12 + FR-046 through FR-050 added for Background Memory-Tier Demotion

**Drift item**: Unspecced MemoryTierEvictor (2026-07-14)  
**File**: `spec.md` User Story 12, FR-046–FR-050, Key Entities  

**Before**: No specification for the background memory-tier evictor. Feature existed only in code (`src/background.rs:409-555`).

**After**:
- **User Story 12** (Priority P3): Background Memory-Tier Demotion — proactive LRU demotion from DRAM to SSD when utilization exceeds threshold.
- **FR-046**: Start evictor thread on `initialize()` when threshold > 0.0; join on `shutdown()`.
- **FR-047**: Periodic utilization check at configurable interval (default 2s).
- **FR-048**: Demote via `oldest_keys(batch_size)`, stop at low watermark or batch exhaustion.
- **FR-049**: Check `is_evictable` before demotion; path is `mt.remove` + `convert_memory_tier_to_block`; remove entry on transition failure.
- **FR-050**: Emit `EvictionEvent` (Demoted or Removed) via eviction channel (FR-042).
- **Key Entities**: Added "Background Memory-Tier Evictor" definition.

---

## Backfills Applied (2026-06-12)

### BACKFILL-013: FR-024 updated for shard-targeted eviction

**Drift item**: DRIFT-A (2026-06-12)  
**File**: `spec.md` FR-024  

**Before**: Described blind LRU via `IMemoryTier::evict_lru()` on most iterations.

**After**: FR-024 now describes shard-targeted eviction:
- `evict_for_space` signature includes `target_key: CacheKey` parameter
- Primary path calls `IMemoryTier::evict_lru_for_key(target_key)` — evicts from the same shard as the target key
- Rationale: memory-tier uses 16 shards (key % 16); untargeted eviction freed space in wrong shards causing "pool full after eviction" failures under pool-overflow workloads despite global free space being available

---

### BACKFILL-014: `evict_lru_for_key` added to IMemoryTier interface listing

**Drift item**: DRIFT-B (2026-06-12)  
**File**: `spec.md` Prior Clarity (Session 2026-05-08)  

**Before**: IMemoryTier listed as providing `insert(), get(), peek(), remove(), evict_lru(), oldest_keys(), touch(), capacity(), used()`.

**After**: Now also lists `evict_lru_for_key()` with description: "evicts the LRU entry from the same shard as `key`, ensuring the freed space is allocatable by a subsequent `insert(key, ...)`." Pool description updated to note 16-shard architecture. Eviction Q&A and write-through Q&A updated to reference `evict_lru_for_key(target_key)` instead of `evict_lru()`.

---

## Previous Backfills (2026-05-29)

| Metric | Count |
|--------|-------|
| Drift items resolved | 7 |
| Backfills applied | BACKFILL-006 through BACKFILL-012 |
| Spec sections modified | FR-019, FR-024, FR-025, FR-037, FR-039, User Story 7, User Story 9, Assumptions |

---

## Backfills Applied

### BACKFILL-006: FR-024 rewritten to reflect actual eviction algorithm

**Drift item**: DRIFT-A  
**File**: `spec.md` FR-024  

**Before**: Described a strict two-phase per-iteration algorithm: Phase 1 scans `oldest_keys(128)` + `is_evictable` every iteration; Phase 2 blind `evict_lru` only when Phase 1 yields nothing.

**After**: FR-024 now describes the actual algorithm:
- `MAX_SCAN=4` (not 128) — small scan window to minimize lock hold time under concurrency
- Clean eviction probe (`oldest_keys(4)` + `is_evictable`) fires only on every 8th attempt
- Blind LRU (`evict_lru`) is the primary path on all other iterations — O(1), preferred under concurrent load
- Loop is guarded by `MAX_ATTEMPTS=512`; returns `AllocationFailed` if space cannot be freed
- Rationale: under high concurrency (batch cold promotions), the two-phase-every-iteration model caused severe lock contention on the memory-tier; the sparse probe + blind-LRU-primary model trades data-safety preference for throughput

**Also updated**: User Story 7 acceptance scenarios revised to match the actual algorithm. The "two-phase approach" framing in scenarios 1 and 3 replaced with the correct "sparse-probe plus blind-LRU-primary" description. Edge Case for `AllocationFailed` from the eviction loop added.

---

### BACKFILL-007: FR-019 and related clarification updated for sliding-window pipeline

**Drift item**: DRIFT-B  
**File**: `spec.md` FR-019, Clarification 2026-05-08 (pipelined reader)  

**Before**: FR-019 and clarification described a strict two-phase model: Phase 1 completes all NVMe reads before Phase 2 begins GPU H2D copies. Correctness argument stated that two-phase design tolerates out-of-order NVMe completions.

**After**: 
- FR-019 now describes `pipelined_ssd_to_gpu_zero_copy` as a sliding-window pipeline: NVMe reads and GPU copies are interleaved per completion, not in two distinct global phases.
- The implementation uses a FIFO inflight queue (`VecDeque`) and submits the next NVMe read immediately after each completion, overlapping SSD I/O with GPU DMA.
- Correctness relies on a single NVMe queue pair completing requests in FIFO order (not on phase isolation).
- Added note: a periodic stream sync every 16 GPU commands bounds the CUDA command queue depth.
- The `pipelined_ssd_to_gpu` fallback (ring-buffer path) still uses batch-submit-then-wait semantics and is unchanged.
- User Story 9 acceptance scenario 1 updated to remove the two-phase framing and describe the sliding window.

---

### BACKFILL-008: FR-037 extended to cover `batch_lookup` warm stream usage

**Drift item**: DRIFT-C  
**File**: `spec.md` FR-037  

**Before**: FR-037 stated the `warm_stream` is used by `lookup_async`.

**After**: FR-037 now notes that `batch_lookup` also uses `warm_stream` for the MemoryTier fast path (same `memcpy_h2d_async` + inline `stream_synchronize`), avoiding DmaBuffer wrapping overhead. The stream is shared via an `AtomicU64` load — no mutex acquired.

---

### BACKFILL-009: FR-025 extended with dispatch-map recovery behavior

**Drift item**: DRIFT-D  
**File**: `spec.md` FR-025  

**Before**: FR-025 only said "When false, extent managers are not reformatted on initialization, preserving on-disk data from previous sessions."

**After**: FR-025 now additionally specifies: "After recovering all extent managers, the dispatcher iterates each extent manager's allocated extents via `for_each_extent` and calls `dm.recover_extent(key, offset, size)` to rebuild the dispatch map index from persisted SSD metadata. The number of recovered extents and elapsed time are logged." This makes the restart persistence behavior observable and testable.

---

### BACKFILL-010: Shutdown sequence updated to include extent manager checkpoint

**Drift item**: DRIFT-E  
**File**: `spec.md` User Story 5 scenario 3, FR-014  

**Before**: Scenario 3 said "the background writer drains all pending write-through jobs, block devices are shut down in reverse order, and resources are released."

**After**: Scenario 3 now includes "extent managers are checkpointed (persisting metadata to SSD) before block device shutdown." FR-014 extended: "Before shutting down block devices, the dispatcher MUST checkpoint all extent managers to persist their metadata."

---

### BACKFILL-011: Edge case added for AllocationFailed from eviction loop exhaustion

**Drift item**: DRIFT-F  
**File**: `spec.md` Edge Cases section  

**Before**: No edge case described for eviction loop exhaustion.

**After**: New edge case added: "When `evict_for_space` cannot free enough memory-tier space within MAX_ATTEMPTS (512) iterations — which can occur when all entries have active read/write references and cannot be evicted — the function returns `AllocationFailed`. This error propagates to the caller as if `mt.insert()` had failed."

---

### BACKFILL-012: `poller_base_cpu` documented in DispatcherConfig

**Drift item**: DRIFT-G  
**File**: `spec.md` FR-011, Key Entities (DispatcherConfig description)  

**Before**: No mention of CPU affinity for NVMe poller threads.

**After**: FR-011 now lists `poller_base_cpu: Option<usize>` as a DispatcherConfig field. When set, each data drive's NVMe poller actor is pinned to CPU core `poller_base_cpu + drive_index`. This is important for NUMA-aware deployments where NVMe PCIe lanes and DRAM are on the same NUMA node as the poller thread. Defaults to `None` (OS scheduler decides).

---

## Items Not Backfilled (implementation details, not operator-facing)

The following were noted in the drift report but not backfilled to the spec because they are internal implementation details that do not affect the observable interface contract:

- Periodic stream sync every 16 GPU commands in `pipelined_ssd_to_gpu_zero_copy` — internal throughput optimization.
- `MAX_SCAN=4` and `MAX_ATTEMPTS=512` exact constants — captured via BACKFILL-006 semantics, exact values intentionally not specified (tunable).

---

## Cumulative Drift Status After This Run

| Drift Item | Status |
|-----------|--------|
| FR-033 memory-tier config fields | Resolved — FR-033 updated (BACKFILL-015) |
| Unspecced MemoryTierEvictor | Resolved — US12 + FR-046–050 added (BACKFILL-016) |
| DRIFT-A: evict_for_space algorithm | Resolved — FR-024 and US7 rewritten |
| DRIFT-B: sliding window pipeline | Resolved — FR-019 and US9 rewritten |
| DRIFT-C: batch_lookup warm_stream | Resolved — FR-037 extended |
| DRIFT-D: dispatch-map recovery | Resolved — FR-025 extended |
| DRIFT-E: shutdown checkpoint | Resolved — FR-014, US5-S3 updated |
| DRIFT-F: AllocationFailed from eviction | Resolved — Edge Cases updated |
| DRIFT-G: poller_base_cpu config | Resolved — FR-011 extended |
| Prior BACKFILL-001 through -005 | Confirmed still resolved |
