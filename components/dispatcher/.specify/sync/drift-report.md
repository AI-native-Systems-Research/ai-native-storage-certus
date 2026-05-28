# Drift Report: Dispatcher Cache Interface (001-dispatcher-cache-interface)

**Generated**: 2026-05-28
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`
**Implementation**: `components/dispatcher/src/` (lib.rs, pipeline.rs, io_segmenter.rs, background.rs)
**Interface**: `components/interfaces/src/idispatcher.rs`

## Summary

| Status | Count |
|--------|-------|
| Aligned | 29 |
| Drifted | 5 |
| Not Implemented | 2 |
| Unspecced Code | 1 |

---

## Functional Requirements

### FR-001: IDispatcher Interface Definition
**Status**: DRIFTED

The spec requires: `initialize`, `shutdown`, `lookup`, `lookup_async`, `check`, `remove`, `populate`, `prepare_store`, `commit_store`, `cancel_store`, and `touch`.

The implementation defines all of the above PLUS `batch_lookup` method. The `batch_lookup` method accepts `&[(CacheKey, IpcHandle)]` and returns `Vec<Result<(), DispatcherError>>`, promoting cold entries in parallel across drives with multi-queue threads.

**Finding**: Interface has two extra methods (`lookup_async`, `batch_lookup`) not listed in spec FR-001.

---

### FR-002: DispatcherError Type
**Status**: ALIGNED

The `DispatcherError` enum covers: `NotInitialized`, `KeyNotFound`, `AlreadyExists`, `AllocationFailed`, `IoError`, `Timeout`, `InvalidParameter`. All failure modes from the spec are covered.

---

### FR-003: populate(key, ipc_handle)
**Status**: ALIGNED

Implementation at `lib.rs`: calls `evict_for_space`, then `mt.insert()`, then `gpu.dma_copy_to_host`, then `dm.create_memory_tier_entry`, then enqueues a background write job. Matches spec exactly.

---

### FR-004: Background Write-Through
**Status**: ALIGNED

`process_write_job` reads from memory-tier pointer via `peek()`, writes to SSD via extent manager, and calls `dm.convert_to_storage` on completion.

---

### FR-005: Memory-Tier Retention After Write-Through
**Status**: ALIGNED

The write-through path calls `dm.convert_to_storage` (sets `ssd_offset`) but does NOT call `mt.remove()`. Memory-tier slot remains allocated for fast lookups.

---

### FR-006: lookup(key, ipc_handle) Dispatch Logic
**Status**: ALIGNED

Queries dispatch map, handles MemoryTier (DMA to GPU + LRU touch), BlockDevice (calls `promote_and_serve`), Staging (DMA from staging buffer), and uses `dma_copy_to_device_async` with CUDA stream when `warm_stream` is available.

---

### FR-007 through FR-018
**Status**: All ALIGNED

---

### FR-019: MDTS-Aware I/O Segmentation & Pipelined Reader
**Status**: DRIFTED

Spec says: "Phase 1 issues up to 16 concurrent NVMe reads directly into unique offsets of the memory-tier slot."

Code now accepts a `max_queue_depth` parameter to `pipelined_ssd_to_gpu_zero_copy` instead of using a hardcoded constant of 16. The single-entry `promote_and_serve` path passes 16 (unchanged behavior). The `batch_lookup` path passes `16 / num_queues` (where `num_queues` is the number of concurrent threads sharing the same drive's NVMe queue) to prevent submission queue overflow.

**Finding**: Pipeline depth is now parameterized rather than fixed at 16.

---

### FR-020 through FR-025
**Status**: All ALIGNED

---

### FR-026: BlockDeviceVersion Selection
**Status**: REMOVED (marked ~~REMOVED~~ in spec)

The spec itself marks this as superseded. No config field exists, implementation hardcodes a single block device. Spec and code agree.

---

### FR-027: ExtentManagerVersion Selection
**Status**: REMOVED (marked ~~REMOVED~~ in spec)

Same as FR-026 — marked removed in spec. Code agrees.

---

### FR-028 through FR-038
**Status**: All ALIGNED

---

## Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `batch_lookup` with parallel per-drive multi-queue cold promotion | `components/dispatcher/src/lib.rs:865-1190` | ~325 | New User Story + FR amendment |

### Description

`batch_lookup` classifies entries by dispatch-map state, serves MemoryTier/Staging hits inline, groups BlockDevice (cold) entries by target drive, then spawns multiple threads per drive (MAX_QUEUES_PER_DRIVE=2) using `std::thread::scope`. Each thread gets its own NVMe client channels and CUDA streams, calls the zero-copy pipeline with reduced queue depth (16/num_queues=8), and results are merged back in order.

---

## Drifted Items Detail

### 1. FR-001: Extra `lookup_async` and `batch_lookup` Methods
**Severity**: Moderate
**Description**: The interface includes `lookup_async` (returns GpuStream for non-blocking H2D DMA) and `batch_lookup` (parallel cold promotion). Neither is listed in FR-001's method enumeration.
**Recommendation**: Update FR-001 to include both methods in the interface definition.

### 2. FR-019: Parameterized Pipeline Queue Depth
**Severity**: Minor
**Description**: The zero-copy pipeline now takes `max_queue_depth` as a parameter instead of using the hardcoded constant `ZERO_COPY_DEPTH = 16`. The single-entry path still uses depth 16; the batch path uses `16 / num_queues` to share NVMe queue capacity across concurrent threads.
**Recommendation**: Update FR-019 to note the parameterized depth and explain the multi-queue sharing strategy.

### 3. User Story 9: Sequential vs Parallel Promotion
**Severity**: Moderate
**Description**: User Story 9 describes a single-entry sequential `promote_and_serve` path. The actual implementation additionally has `batch_lookup` which promotes multiple cold entries concurrently with per-drive parallelism and multi-queue threads. The sequential path still exists but is no longer the primary cold promotion mechanism for batch operations.
**Recommendation**: Add a new User Story (Story 11) describing batch parallel cold promotion, or extend Story 9 to cover both sequential and parallel paths.

---

## Recommendations

1. **Add `batch_lookup` to FR-001** and create a dedicated FR describing its parallel promotion semantics.
2. **Update FR-019** to reflect the parameterized `max_queue_depth` and multi-queue sharing.
3. **Add User Story 11** for batch parallel cold promotion (per-drive thread parallelism, multi-queue, reduced queue depth).
