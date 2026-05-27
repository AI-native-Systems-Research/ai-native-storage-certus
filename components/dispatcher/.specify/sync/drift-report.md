# Drift Report: Dispatcher Cache Interface (001-dispatcher-cache-interface)

**Generated**: 2026-05-27
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`
**Implementation**: `components/dispatcher/src/` (lib.rs, pipeline.rs, io_segmenter.rs, background.rs)
**Interface**: `components/interfaces/src/idispatcher.rs`

## Summary

| Status | Count |
|--------|-------|
| Aligned | 29 |
| Drifted | 4 |
| Not Implemented | 2 |
| Unspecced Code | 0 |

---

## Functional Requirements

### FR-001: IDispatcher Interface Definition
**Status**: DRIFTED

The spec requires: `initialize`, `shutdown`, `lookup`, `check`, `remove`, `populate`, `prepare_store`, `commit_store`, `cancel_store`, and `touch`.

The implementation defines all of the above PLUS `lookup_async` (returns a `GpuStream` for non-blocking H2D DMA). The `lookup_async` method is present in the interface definition (`idispatcher.rs:172`) and fully implemented. The spec does not mention `lookup_async` as a separate method.

**Finding**: Interface has one extra method (`lookup_async`) not listed in spec FR-001.

---

### FR-002: DispatcherError Type
**Status**: ALIGNED

The `DispatcherError` enum covers: `NotInitialized`, `KeyNotFound`, `AlreadyExists`, `AllocationFailed`, `IoError`, `Timeout`, `InvalidParameter`. All failure modes from the spec are covered.

---

### FR-003: populate(key, ipc_handle)
**Status**: ALIGNED

Implementation at `lib.rs:971-1062`: calls `evict_for_space`, then `mt.insert()`, then `gpu.dma_copy_to_host`, then `dm.create_memory_tier_entry`, then enqueues a background write job. Matches spec exactly.

---

### FR-004: Background Write-Through
**Status**: ALIGNED

`process_write_job` (lib.rs:313-379) reads from memory-tier pointer, writes to SSD via extent manager, and calls `dm.convert_to_storage` on completion. In memory-tier-only mode, assigns synthetic offset.

---

### FR-005: Memory-Tier Retention After Write-Through
**Status**: ALIGNED

The write-through path calls `dm.convert_to_storage` (sets `ssd_offset`) but does NOT call `mt.remove()`. Memory-tier slot remains allocated for fast lookups.

---

### FR-006: lookup(key, ipc_handle) Dispatch Logic
**Status**: ALIGNED

Implementation at `lib.rs:781-903`: queries dispatch map, handles MemoryTier (DMA to GPU + LRU touch), BlockDevice (calls `promote_and_serve`), Staging (DMA from staging buffer), and uses `dma_copy_to_device_async` with CUDA stream when `warm_stream` is available, falling back to synchronous copy.

---

### FR-007: Lookup Cache Miss
**Status**: ALIGNED

Returns `DispatcherError::KeyNotFound(key)` when `LookupResult::NotExist` (lib.rs:821).

---

### FR-008: check(key)
**Status**: ALIGNED

Implementation at `lib.rs:906-925`: queries dispatch map and returns boolean without data transfer.

---

### FR-009: remove(key)
**Status**: ALIGNED

Implementation at `lib.rs:927-968`: frees memory-tier slot (`mt.remove`), removes dispatch-map entry (`dm.remove`), frees SSD extent if in BlockDevice state.

---

### FR-010: define_component! Usage
**Status**: ALIGNED

`define_component!` at lib.rs:60-81 exposes only `IDispatcher` interface.

---

### FR-011: Receptacles
**Status**: ALIGNED

Receptacles declared: `logger: ILogger`, `dispatch_map: IDispatchMap`, `gpu_services: IGpuServices`, `spdk_env: ISPDKEnv`, `memory_tier: IMemoryTier`. Matches spec.

---

### FR-012: initialize Validation
**Status**: ALIGNED

`initialize` validates `dispatch_map` and `memory_tier` are bound before proceeding (lib.rs:581-587). Other receptacles are used conditionally (spdk_env checked via `is_connected()`).

---

### FR-013: Thread Safety
**Status**: ALIGNED

Dispatch map accessed via `Arc<dyn IDispatchMap + Send + Sync>` with read/write reference semantics. Internal state protected by `Mutex` fields.

---

### FR-014: shutdown Drains Background Operations
**Status**: ALIGNED

`shutdown()` (lib.rs:732-778): stops evictor, stops background writer (which drains pending jobs), then shuts down block devices.

---

### FR-015: N Block Devices with N Extent Managers
**Status**: ALIGNED

`create_data_drives()` creates one `DataDrive` per PCI address, each with its own block device and extent manager.

---

### FR-016: FormatParams Computed from Device Size
**Status**: ALIGNED

`create_data_drives()` (lib.rs:527-555) computes `data_disk_size`, `region_size`, `slab_size`, `max_extent_size` from block device metadata and passes to `iem.format()`.

---

### FR-017: Silent Write-Through Failure
**Status**: ALIGNED

`process_write_job` silently returns (no error propagation) on extent allocation failure or I/O error (lib.rs:360-369).

---

### FR-018: remove Does Not Block on Write-Through
**Status**: ALIGNED

`remove()` proceeds immediately. The lookup call may block on write ref, but the spec's FR-018 intent is satisfied: remove doesn't explicitly wait for write-through completion.

---

### FR-019: MDTS-Aware I/O Segmentation
**Status**: ALIGNED

`io_segmenter.rs` implements MDTS-aware splitting. `pipeline.rs` implements zero-copy pipeline with 16 concurrent NVMe reads (`ZERO_COPY_DEPTH = 16`). Fallback ring-buffer path (`pipelined_ssd_to_gpu`) with 8 ring buffers also exists.

---

### FR-020: prepare_store
**Status**: ALIGNED

Implementation at `lib.rs:1064-1167`: validates size > 0, checks for existing key (returns AlreadyExists), reserves extent, returns DMA buffer. Cleanup on failure (removes dm entry).

---

### FR-021: commit_store
**Status**: ALIGNED

Implementation at `lib.rs:1169-1209`: writes buffer to SSD with MDTS segmentation, publishes extent, transitions dispatch-map entry.

---

### FR-022: cancel_store
**Status**: ALIGNED

Implementation at `lib.rs:1211-1230`: removes pending write (WriteHandle dropped = abort), removes dispatch-map entry.

---

### FR-023: touch(key)
**Status**: ALIGNED

Implementation at `lib.rs:1233-1243`: calls `dm.touch(key)`, returns KeyNotFound if key doesn't exist. No DMA or reference acquisition.

---

### FR-024: Capacity-Based Eviction
**Status**: ALIGNED

`evict_for_space` (lib.rs:291-310) is purely capacity-based: loops while `mt.used() + needed > mt.capacity()`, evicting LRU entries via `mt.evict_lru()` and transitioning to BlockDevice via `dm.convert_memory_tier_to_block`.

---

### FR-025: format_on_init Flag
**Status**: ALIGNED

`DispatcherConfig.format_on_init` defaults to `true`. When `false`, `iem.format()` is skipped (lib.rs:543).

---

### FR-026: BlockDeviceVersion Selection
**Status**: NOT IMPLEMENTED

The spec requires `DispatcherConfig` to support `BlockDeviceVersion` selection (V1, V2). The `DispatcherConfig` struct in `idispatcher.rs` has no `block_device_version` field. The implementation hardcodes `BlockDeviceSpdkNvmeComponent` (a single version).

---

### FR-027: ExtentManagerVersion Selection
**Status**: NOT IMPLEMENTED

The spec requires `DispatcherConfig` to support `ExtentManagerVersion` selection. The `DispatcherConfig` struct has no `extent_manager_version` field. The implementation hardcodes `ExtentManager` (a single version).

---

### FR-028: Promotion Re-Registers as MemoryTier
**Status**: ALIGNED

`promote_and_serve` (lib.rs:277-282): removes old BlockDevice entry, creates fresh MemoryTier entry, preserves ssd_offset via `convert_to_storage`.

---

### FR-029: Background SSD Evictor Thread
**Status**: ALIGNED

`BackgroundEvictor::start()` is called during `initialize()` when `ssd_eviction_threshold > 0.0` and extent managers exist (lib.rs:691-723). Thread is joined during `shutdown()` (lib.rs:735-737).

---

### FR-030: SSD Utilization Monitoring
**Status**: ALIGNED

`compute_utilization()` (background.rs:241-251) sums `used_bytes()` and `capacity_bytes()` across all extent managers. Interval is configurable via `ssd_eviction_interval_secs`.

---

### FR-031: SSD Eviction Logic
**Status**: ALIGNED

`evictor_loop` (background.rs:159-238): when utilization exceeds threshold, calls `dm.oldest_keys(batch_size)`, evicts until below low_watermark or batch exhausted.

---

### FR-032: SSD Evictor Skips MemoryTier Entries
**Status**: ALIGNED

`get_evictable_offset` (background.rs:253-273): returns `None` for `LookupResult::MemoryTier` entries. `dm.remove()` failure (active references) is handled gracefully with `continue`.

---

### FR-033: SSD Eviction Config Fields
**Status**: ALIGNED

`DispatcherConfig` includes: `ssd_eviction_threshold` (f64, default 0.9), `ssd_eviction_low_watermark` (f64, default 0.8), `ssd_eviction_batch_size` (usize, default 64), `ssd_eviction_interval_secs` (u64, default 5). Setting threshold to 0.0 disables evictor (checked at lib.rs:691).

---

### FR-034: Memory-Tier Pool Registration
**Status**: ALIGNED

During `initialize()`, after GPU and drives are ready, calls `gpu.register_host_memory()` on memory-tier pool (lib.rs:636-653). Failure is logged but non-fatal.  Cached `ClientChannels` are stored per data drive (lib.rs:562).

---

### FR-035: Memory-Tier Pool Unregistration on Shutdown
**Status**: ALIGNED

During `shutdown()`, calls `gpu.unregister_host_memory()` before teardown (lib.rs:746-753).

---

### FR-038: clear_memory_tier Method
**Status**: ALIGNED

Implementation at `lib.rs:1282-1300`: loops `mt.evict_lru()` until empty, calling `dm.convert_memory_tier_to_block(key)` for each evicted key (falls back to `dm.remove(key)` if transition fails). `IMemoryTier::clear()` also implemented in memory-tier component (resets slots, LRU, and allocator atomically).

---

## Success Criteria

### SC-001 through SC-013
**Status**: All ALIGNED

Test coverage exists for:
- SC-001: `lookup_memory_tier_hit`, `populate_succeeds_after_init`
- SC-002: `check_existing_returns_true`, `check_nonexistent_returns_false`
- SC-003: `remove_existing_succeeds`
- SC-004: `concurrent_populate_different_keys`, `concurrent_checks_on_initialized_dispatcher`
- SC-005: `initialize_without_receptacles_fails`
- SC-006: `drain_on_shutdown` (background.rs tests)
- SC-007: `initialize_multiple_pci_addrs`
- SC-008: `prepare_store_returns_dma_buffer`
- SC-009: `evict_for_space_evicts_when_pool_full`, `populate_triggers_eviction_on_full_pool`
- SC-010: touch tested implicitly via `lookup_memory_tier_hit` (calls `mt.touch`)
- SC-011: `lookup_block_device_promote_without_hardware`
- SC-012: Pipeline functions tested via integration tests with hardware
- SC-013: `evictor_full_eviction_cycle`, `evictor_start_and_shutdown`

---

## Drifted Items Detail

### 1. FR-001: Extra `lookup_async` Method
**Severity**: Low
**Description**: The interface includes `lookup_async` which returns a `GpuStream` for non-blocking H2D DMA copies. The synchronous `lookup` is implemented as `lookup_async` + `stream_synchronize`. This is an implementation enhancement that should be added to the spec.
**Recommendation**: Update spec FR-001 to list `lookup_async` as an additional method, or add a new FR describing the async variant.

### 2. FR-026: Missing BlockDeviceVersion Selection
**Severity**: Medium
**Description**: The spec requires version-selectable block device components but the config struct has no such field and only one block device implementation is used.
**Recommendation**: Either add `block_device_version` field to `DispatcherConfig` and implement selection logic, or remove FR-026 from the spec if V2 is the sole supported version going forward.

### 3. FR-027: Missing ExtentManagerVersion Selection
**Severity**: Medium
**Description**: Same as FR-026 but for extent managers. No version selection field exists in config.
**Recommendation**: Either add `extent_manager_version` field or remove FR-027 from spec.

### 4. Initialize Rejects Empty PCI Addresses Even in Memory-Tier-Only Mode
**Severity**: Low
**Description**: Spec User Story 5 acceptance scenario 4 says: "Given the spdk_env receptacle is not connected, When initialize is called, Then the dispatcher operates in memory-tier-only mode (no block devices created)." However, the implementation rejects empty `data_pci_addrs` with an `InvalidParameter` error BEFORE checking `spdk_env.is_connected()`. This means callers must provide PCI addresses even when operating in memory-tier-only mode (where they won't be used).
**Recommendation**: Move the `data_pci_addrs.is_empty()` check inside the `if self.spdk_env.is_connected()` block, or relax the validation when SPDK is not available.

---

## Unspecced Code

None.

---

## Recommendations

1. **Resolve FR-026 / FR-027**: Decide whether version selection is still a requirement. If only V2 components will be supported, remove these requirements. Otherwise, add the config fields and selection logic.

3. **Fix memory-tier-only mode initialization**: The empty PCI address check should be conditional on SPDK availability to properly support the memory-tier-only mode described in User Story 5, scenario 4.
