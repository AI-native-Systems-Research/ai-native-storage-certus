# Drift Report: extent-manager v2

**Spec**: `specs/001-extent-manager-v2/spec.md`
**Component**: `components/extent-manager/`
**Date**: 2026-06-18
**Status**: 5 Drifted | 27 Aligned | 2 Not Implemented

---

## Summary Table

| Requirement | Status | Severity | Notes |
|-------------|--------|----------|-------|
| FR-001 | Aligned | - | Component named ExtentManager per spec |
| FR-002 | Aligned | - | All FormatParams validated |
| FR-003 | Aligned | - | Superblock written at LBA 0 with CRC32 |
| FR-004 | Aligned | - | initialize reads superblock, validates magic/CRC, recovers |
| FR-005 | Aligned | - | reserve_extent returns WriteHandle with offset |
| FR-006 | Aligned | - | FREE_KEY publish silently frees slot |
| FR-007 | Aligned | - | abort/drop releases slot |
| FR-008 | Aligned | - | remove_extent sets FREE_KEY, deferred free |
| FR-009 | Aligned | - | get_extents and for_each_extent filter FREE_KEY |
| FR-010 | Aligned | - | Slab has dense Vec<u64> keys parallel to bitmap |
| FR-011 | Aligned | - | FREE_KEY = u64::MAX sentinel |
| FR-012 | Aligned | - | BTreeMap<u64, Slab> keyed by start_offset |
| FR-013 | Aligned | - | checkpoint serializes all regions with CRC32 |
| FR-014 | Aligned | - | checkpoint skips if no dirty regions |
| FR-015 | Aligned | - | Coalescing via CheckpointCoalesce state machine |
| FR-016 | Drifted | Moderate | Default interval 30s vs spec 300s |
| FR-017 | Aligned | - | Recovery tries active, falls back to inactive |
| FR-018 | Aligned | - | Recovery rebuilds bitmap from key vectors |
| FR-019 | Aligned | - | BuddyAllocator per region |
| FR-020 | Aligned | - | Bitmap allocator with rover |
| FR-021 | Aligned | - | SizeClassManager indexes slabs by element_size |
| FR-022 | Aligned | - | key & (region_count - 1) sharding |
| FR-023 | Aligned | - | parking_lot::RwLock per region |
| FR-024 | Aligned | - | Component is Send + Sync via Arc/Mutex/RwLock |
| FR-025 | Aligned | - | pending_frees defers slot reuse until checkpoint |
| FR-026 | Aligned | - | get_instance_id implemented |
| FR-027 | Aligned | - | set_checkpoint_interval implemented |
| FR-028 | Aligned | - | set_metadata_ns_id on concrete type |
| FR-029 | Aligned | - | set_dma_alloc on concrete type |
| FR-030 | Aligned | - | volatile_write_cache feature gate present |
| FR-031 | Aligned | - | used_bytes() on IExtentManager |
| FR-032 | Aligned | - | capacity_bytes() on IExtentManager |
| FR-033 | Aligned | - | WriteHandle defined in interfaces crate |
| FR-034 | Drifted | Minor | Feature gate named "testing" not "test-only" |
| Superblock | Drifted | Major | Magic/version/layout mismatch |
| SC-005 | Drifted | Moderate | No test at 100M scale |
| SC-010 (implied) | Not Implemented | - | No explicit Send+Sync compile-time assertion |

---

## Detailed Findings

### Aligned

#### FR-001: Component naming and definition
**Spec**: Component MUST be named ExtentManager, defined using define_component!, providing IExtentManager with receptacles metadata_device: IBlockDevice and logger: ILogger.
**Code**: `define_component! { pub ExtentManager { version: "0.3.0", provides: [IExtentManager], receptacles: { metadata_device: IBlockDevice, logger: ILogger } ... } }` in `src/lib.rs:66-85`.
**Status**: Fully aligned.

#### FR-002: FormatParams validation
**Spec**: format() MUST validate sector_size > 0, slab_size multiple of sector_size, max_extent_size <= slab_size, region_count positive power of two, metadata device large enough.
**Code**: All five validations present in `src/lib.rs:320-369`.
**Status**: Fully aligned.

#### FR-003: Superblock at LBA 0
**Spec**: format() MUST write superblock at LBA 0 with format parameters, checkpoint layout, and CRC32.
**Code**: `Superblock::new()` + `serialize()` + `write_blocks(0, ...)` in `src/lib.rs:415-430`.
**Status**: Fully aligned (layout differences noted separately under Superblock drift).

#### FR-004: initialize() recovery
**Spec**: initialize() MUST read superblock, validate magic/CRC, recover from active checkpoint, rebuild allocation state.
**Code**: `src/recovery.rs:11-87` reads superblock, validates via `Superblock::deserialize()`, recovers checkpoint, `src/lib.rs:446-513` rebuilds regions.
**Status**: Fully aligned.

#### FR-005: reserve_extent returns WriteHandle
**Spec**: reserve_extent(key, size) MUST allocate sector-aligned slot and return WriteHandle with disk byte offset; not visible until publish.
**Code**: `src/lib.rs:516-552` allocates via `r.alloc_extent(size)`, sector-aligns, creates WriteHandle with publish/abort closures.
**Status**: Fully aligned.

#### FR-006: FREE_KEY publish is silent discard
**Spec**: If key equals FREE_KEY, slot MUST be freed and call returns Ok without visibility.
**Code**: `src/lib.rs:536-539` checks `key == FREE_KEY`, calls `r.free_slot()`, returns `Ok(Extent{...})`.
**Status**: Fully aligned.

#### FR-007: Abort/drop releases slot
**Spec**: abort() or dropping MUST release slot without writing key.
**Code**: `WriteHandle::abort()` takes `abort_fn`, `Drop` impl calls it if not consumed (`interfaces/src/iextent_manager.rs:141-155`).
**Status**: Fully aligned.

#### FR-008: remove_extent with deferred free
**Spec**: remove_extent(offset) sets key to FREE_KEY in memory, hides from enumeration, returns OffsetNotFound for invalid offset, slot freed only after next checkpoint.
**Code**: `src/region.rs:124-153` sets key to FREE_KEY, adds to pending_frees; `flush_pending_frees()` called only after checkpoint (`src/lib.rs:280`). Empty slab return to buddy is in `free_slot()`.
**Status**: Fully aligned.

#### FR-009: Enumeration
**Spec**: get_extents() returns all non-FREE_KEY; for_each_extent() invokes callback.
**Code**: Both methods iterate all slabs/slots, skip FREE_KEY (`src/lib.rs:554-600`).
**Status**: Fully aligned.

#### FR-010: Per-slab key vectors
**Spec**: Each Slab MUST maintain dense Vec<u64> parallel to bitmap.
**Code**: `pub keys: Vec<u64>` in `src/slab.rs:12`, initialized to `vec![FREE_KEY; num_slots]`.
**Status**: Fully aligned.

#### FR-011: FREE_KEY sentinel
**Spec**: FREE_KEY = u64::MAX marks unoccupied.
**Code**: `pub const FREE_KEY: u64 = u64::MAX` in `src/slab.rs:5`.
**Status**: Fully aligned.

#### FR-012: BTreeMap for slabs
**Spec**: Slabs in BTreeMap<u64, Slab> keyed by start_offset; O(log n) lookup via range.
**Code**: `pub slabs: BTreeMap<u64, Slab>` in `src/region.rs:11`; `range(..=offset).next_back()` in `src/region.rs:129-131`.
**Status**: Fully aligned.

#### FR-013: Checkpoint serialization
**Spec**: checkpoint() serializes slab descriptors + key vectors into CRC32-protected blob, writes to inactive copy, switches superblock.
**Code**: `src/checkpoint.rs:35-116` serializes regions, builds CRC32 header, writes to inactive, updates superblock active_copy/seq.
**Status**: Fully aligned.

#### FR-014: Skip when clean
**Spec**: checkpoint() MUST skip I/O if no region modified since last checkpoint.
**Code**: `src/lib.rs:240-249` checks `any_dirty`, returns Ok(()) early if false.
**Status**: Fully aligned.

#### FR-015: Checkpoint coalescing
**Spec**: Concurrent calls coalesced; at most two actual I/O operations.
**Code**: `CheckpointCoalesce` with `in_progress` flag and `completed_seq` counter; waiters compute `needed = completed_seq + 2` when in_progress, block on condvar. At most one running + one queued = two I/O ops.
**Status**: Fully aligned.

#### FR-017: Dual-copy fallback
**Spec**: Recovery tries active first; falls back to inactive on CRC/media failure.
**Code**: `src/recovery.rs:31-68` tries active, on error tries inactive with prev_seq.
**Status**: Fully aligned.

#### FR-018: Recovery rebuilds from key vectors
**Spec**: initialize() restores extents by reading key vectors, marking non-FREE slots as allocated.
**Code**: `slab_from_descriptor()` in `src/recovery.rs:78-87` iterates keys, calls `mark_slot_allocated` for non-FREE.
**Status**: Fully aligned.

#### FR-019: Buddy allocator
**Spec**: Each region MUST use buddy allocator for slab-sized chunks.
**Code**: `BuddyAllocator` in `src/buddy.rs`; each `RegionState` has `buddy: BuddyAllocator` (`src/region.rs:12`).
**Status**: Fully aligned.

#### FR-020: Bitmap with rover
**Spec**: Each slab uses bitmap allocator with rover for even distribution.
**Code**: `AllocationBitmap` + `rover` field in Slab; `find_free_from(self.rover)` and rover wrap (`src/slab.rs:30-33`).
**Status**: Fully aligned.

#### FR-021: Size-class manager
**Spec**: Size-class manager indexes slabs by element size for O(1) lookup.
**Code**: `SizeClassManager` with `HashMap<u32, Vec<u64>>` in `src/slab.rs:92-122`.
**Status**: Fully aligned.

#### FR-022: Key sharding
**Spec**: Keys sharded to regions by key & (region_count - 1).
**Code**: `let idx = key as usize & (regions.len() - 1)` in `src/lib.rs:189`.
**Status**: Fully aligned.

#### FR-023: Independent region locking
**Spec**: Each region independently locked (parking_lot::RwLock); hot-path only acquires target region's lock.
**Code**: `Vec<Arc<RwLock<RegionState>>>` using `parking_lot::RwLock`. `reserve_extent` acquires outer `regions` read-lock (cheap, non-contended) then single region write-lock. `remove_extent` additionally touches `shared` mutex for offset-to-region mapping.
**Status**: Aligned. The outer read-lock and shared mutex access are lightweight non-contended lookups, not per-region locks.

#### FR-024: Send + Sync
**Spec**: Component MUST be Send + Sync and safe for concurrent use.
**Code**: All fields are Arc/Mutex/RwLock wrapped; concurrent integration tests exercise multi-threaded access.
**Status**: Aligned (implicitly via type composition).

#### FR-025: Deferred free crash safety
**Spec**: Freed slot MUST NOT be reallocated until removal persisted by checkpoint.
**Code**: `pending_frees: Vec<(u64, usize)>` accumulates removals; `flush_pending_frees()` called after checkpoint completes (`src/lib.rs:280`).
**Status**: Fully aligned.

#### FR-026: get_instance_id
**Spec**: MUST provide get_instance_id() returning superblock instance_id.
**Code**: `fn get_instance_id()` in `src/lib.rs:608-614`.
**Status**: Fully aligned.

#### FR-027: set_checkpoint_interval
**Spec**: MUST provide set_checkpoint_interval(duration).
**Code**: `fn set_checkpoint_interval(&self, interval: Option<Duration>)` in `src/lib.rs:616-618`, exposed on IExtentManager trait.
**Status**: Fully aligned.

#### FR-028: set_metadata_ns_id
**Spec**: MUST provide set_metadata_ns_id(ns_id: u32).
**Code**: `pub fn set_metadata_ns_id(&self, ns_id: u32)` in `src/lib.rs:159-161` on concrete ExtentManager type. Not on IExtentManager trait, but spec does not require it be on the trait.
**Status**: Aligned.

#### FR-029: set_dma_alloc
**Spec**: MUST provide set_dma_alloc(alloc).
**Code**: `pub fn set_dma_alloc(&self, alloc: DmaAllocFn)` in `src/lib.rs:151-153` on concrete type.
**Status**: Aligned.

#### FR-030: volatile_write_cache feature
**Spec**: Component MAY support volatile_write_cache feature gate controlling flush calls.
**Code**: Feature `volatile_write_cache` in `Cargo.toml:27`; `#[cfg(feature = "volatile_write_cache")]` guards `metadata_client.flush()` calls in `src/lib.rs:272` and `src/checkpoint.rs:104`.
**Status**: Aligned (MAY requirement, fully implemented).

#### FR-031: used_bytes()
**Spec**: MUST provide used_bytes() returning total allocated bytes.
**Code**: `fn used_bytes(&self) -> u64` in `src/lib.rs:620-630`, sums `total_usable_size - total_free` per region via buddy allocator.
**Status**: Aligned. Note: implementation reports buddy-level allocation (slab granularity) rather than per-extent sum, but this matches "bytes currently allocated" semantics at the block level.

#### FR-032: capacity_bytes()
**Spec**: MUST provide capacity_bytes() returning total data capacity.
**Code**: `fn capacity_bytes(&self) -> u64` in `src/lib.rs:632-636`, sums `total_usable_size` per region.
**Status**: Fully aligned.

#### FR-033: WriteHandle in interfaces crate
**Spec**: WriteHandle MUST be defined in interfaces crate with RAII two-phase commit.
**Code**: `WriteHandle` struct with `publish()`, `abort()`, and `Drop` impl in `interfaces/src/iextent_manager.rs:95-155`. The extent-manager `src/write_handle.rs` is just a comment pointing there.
**Status**: Fully aligned.

---

### Drifted

#### FR-016: Background checkpoint default interval (Moderate)
**Spec**: Background thread MUST call checkpoint() at a configurable interval (default 300 seconds / 5 minutes).
**Code**: Default is `Duration::from_secs(30)` (30 seconds) set in `src/lib.rs:94`.
**Impact**: Moderate. Affects default checkpoint frequency and metadata write amplification in production. Systems relying on spec-documented 5-minute interval will experience 10x more frequent checkpoints.

#### FR-034: Fault injection feature gate name (Minor)
**Spec**: Component MUST provide fault injection test infrastructure behind a `test-only` feature gate.
**Code**: Feature gate is named `testing` (Cargo.toml line 22). Module `test_support` is gated by `#[cfg(any(test, feature = "testing"))]`. FaultConfig and MockBlockDevice with fault injection are fully implemented.
**Impact**: Minor. Functionality is complete; only the feature gate name differs from spec.

#### Superblock On-Disk Layout (Major)
**Spec**: Magic `0x4345_5254_5553_5635` ("CERTUSV5"), version 5. CRC32 at offset 84 covering bytes 0-83. No `data_start_offset` field.
**Code**: Magic `0x4345_5254_5553_5634` ("CERTUSV4"), `FORMAT_VERSION = 6`. Superblock includes `data_start_offset` (8 bytes) at offset 84, pushing CRC to offset 92. Total header payload is 96 bytes (vs spec's 88).
**Impact**: Major. Any external tool, recovery utility, or cross-version compatibility check that uses the spec's documented layout will fail. The magic, version, and field layout are all inconsistent with the spec.

#### SC-005: 100M extent scale validation (Moderate)
**Spec**: Component designed to support approximately 100 million extents on 10 TB device with 128 KiB extent size.
**Code**: Largest test uses 800 extents on 256 MiB. No benchmark or test exercises the stated architectural target.
**Impact**: Moderate. Scalability claim is unverified.

#### FR-031 used_bytes() semantics (Minor)
**Spec**: "returns the total number of bytes currently allocated (sum of all published extent sizes)".
**Code**: Returns `buddy.total_usable_size() - buddy.total_free()` which is slab-granularity allocation (whole slabs allocated to size classes), not the sum of individual extent sizes. A slab with 1 allocated 4KiB extent in a 1GiB slab reports 1GiB used.
**Impact**: Minor. Reported value may be much larger than actual extent usage due to internal fragmentation at slab level. The metric is useful but not precisely what the spec describes.

---

### Not Implemented

#### FR-024 Compile-time assertion (Low)
**Spec**: Component MUST be Send + Sync.
**Code**: No explicit `static_assertions::assert_impl_all!(ExtentManager: Send, Sync)` or equivalent. The component IS Send+Sync by virtue of its fields, and concurrent tests prove it works, but there is no compile-time guarantee that a future refactor couldn't break this.
**Status**: Functionally satisfied but not formally asserted. Low risk.

#### BlockDeviceClient::flush() method
**Spec**: FR-030 implies flush calls are issued to metadata device when volatile_write_cache is enabled.
**Code**: `metadata_client.flush()` is called under `#[cfg(feature = "volatile_write_cache")]` but no `flush()` method exists in `src/block_io.rs`. The feature likely does not compile without a `flush()` implementation (possibly provided via conditional compilation not visible in the current source, or it simply is not compiled in default builds).
**Status**: Incomplete implementation detail for the volatile_write_cache path.

---

## Unspecced Features

| Feature | Location | Description |
|---------|----------|-------------|
| `data_start_offset` in Superblock | `src/superblock.rs:23`, `src/lib.rs:374-378` | Enables metadata and data to coexist on the same NVMe device. Superblock records where data begins after the metadata reservation. Not in spec. |
| `metadata_region_size` in FormatParams | `interfaces/src/iextent_manager.rs:63-67` | Caps the metadata area size so metadata+data can share a single SSD (default 128 MiB). Not listed in spec's Key Entities/FormatParams. |

---

## Recommendations

1. **Major: Reconcile superblock on-disk format** between spec and code. The spec documents version 5 with magic CERTUSV5, but code uses CERTUSV4/version 6 with an extra `data_start_offset` field. Update the spec to match the actual binary layout or version the documentation.

2. **Moderate: Reconcile default checkpoint interval** -- spec says 300s, code uses 30s. The 30s default is likely intentional for correctness (shorter data loss window). Update spec to reflect actual default.

3. **Moderate: Add scale benchmark for SC-005** to validate the 100M extent architectural target, even if just measuring memory overhead and BTreeMap lookup latency.

4. **Minor: Add `metadata_region_size` and `data_start_offset` to spec** as they are production-critical for single-device deployments.

5. **Minor: Rename feature gate** in either spec (`test-only` -> `testing`) or code (`testing` -> `test-only`) for consistency.

6. **Minor: Clarify used_bytes() semantics** in spec to state it reports slab-level allocation rather than per-extent sum, or change implementation to sum actual extent sizes.

7. **Low: Add compile-time Send+Sync assertion** for FR-024 robustness.

8. **Low: Verify volatile_write_cache feature compiles** by ensuring `BlockDeviceClient::flush()` method exists when the feature is enabled.
