# Spec Drift Report
Generated: 2026-07-10
Project: dispatch-map

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 29 |
| Aligned | 27 (93%) |
| Drifted | 1 (3%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 1 |

## Sync Actions Applied (2026-07-10)

The following drift was resolved by updating the spec to match the implementation:

- **FR-002**: Removed `Staging` variant from Location enum; now specifies only `BlockDevice` and `MemoryTier`.
- **FR-003**: Replaced `create_staging(key, size)` with `create_memory_tier_entry(key, pointer, size)`.
- **FR-004**: Removed `Staging(DmaBuffer)` from lookup return variants; now returns `NotExist`, `BlockDevice`, or `MemoryTier`.
- **FR-005**: Updated to describe MemoryTier `ssd_offset` setting (removed staging buffer transition language).
- **FR-018**: Simplified to cover only `convert_memory_tier_to_block`; `create_memory_tier_entry` now under FR-003.
- **FR-019**: Marked as removed (DMA allocator injection no longer needed).
- **FR-021**: Marked as merged into FR-005 (redundant).
- **FR-024**: Added for `recover_extent(key, offset, size_blocks)`.
- **SC-004**: Updated to reference BlockDevice/MemoryTier size variation instead of Staging.
- User Stories 1-3, 6-8, Edge Cases, Key Entities, Clarifications, Assumptions: Updated to remove all Staging/DMA buffer references.

## Detailed Findings
### Spec: 001-dispatch-map - Dispatch Map Component
#### Aligned
- FR-001: CacheKey type as u64 → `interfaces::CacheKey` (re-exported from interfaces crate at src/lib.rs:25)
- FR-002: Location enum with BlockDevice and MemoryTier variants → `src/entry.rs:7-17`
- FR-003: create_memory_tier_entry(key, pointer, size) creates MemoryTier entry with write_ref=1 → `src/lib.rs:362-405`
- FR-004: lookup returns NotExist, BlockDevice, or MemoryTier; increments read_ref; blocks on active write with 2000ms timeout → `src/lib.rs:113-155`
- FR-005: convert_to_storage on MemoryTier sets ssd_offset, conditional read_ref decrement, error on BlockDevice → `src/lib.rs:157-188`
- FR-006: take_read waits for write_ref=0 with 2000ms timeout, increments read_ref → `src/lib.rs:191-213`
- FR-007: take_write waits for read_ref=0 and write_ref=0 with 2000ms timeout, sets write_ref=1 → `src/lib.rs:216-238`
- FR-008: release_read decrements read_ref, returns RefCountUnderflow if already 0 → `src/lib.rs:241-258`
- FR-009: release_write sets write_ref=0, returns RefCountUnderflow if already 0 → `src/lib.rs:261-278`
- FR-010: downgrade_reference atomically transitions write to read in single critical section, error if no write ref → `src/lib.rs:281-303`
- FR-011: remove deletes entry, returns ActiveReferences error if read_ref>0 or write_ref>0 → `src/lib.rs:305-328`
- FR-012: initialize requires IEvictionPolicy connected (error if unbound), recovers from IExtentManager via for_each_extent, returns Ok(()) when no IExtentManager bound → `src/lib.rs:67-111`
- FR-013: All methods thread-safe via Mutex<Inner> and Condvar blocking semantics → `src/state.rs` (entire module)
- FR-014: ILogger receptacle used for info, debug, and error logging throughout the component → used in initialize, lookup, convert_to_storage, take_read, take_write, release_read, release_write, downgrade_reference, remove, create_memory_tier_entry, convert_memory_tier_to_block
- FR-015: define_component! with IDispatchMap provider, ILogger/IExtentManager/IEvictionPolicy receptacles → `src/lib.rs:32-45`
- FR-016: touch(key) delegates to IEvictionPolicy ep.touch(handle), no ref count changes, returns KeyNotFound if missing → `src/lib.rs:330-342`
- FR-017: oldest_keys(n) delegates to IEvictionPolicy::peek_oldest(pool_id, n), thread-safe → `src/lib.rs:353-359`
- FR-018: MemoryTier variant has pointer/size/ssd_offset; convert_memory_tier_to_block(key) reads offset from ssd_offset field → `src/lib.rs:407-443, src/entry.rs:11-16`
- FR-020: initialize() is explicit public API, not called during construction → `src/lib.rs:67` (must be called after binding receptacles)
- FR-022: is_evictable returns true iff key exists, MemoryTier state, ssd_offset: Some(_), read_ref==0, write_ref==0; false for all other cases → `src/lib.rs:445-458`
- FR-023: entry_size(key) returns size_blocks * 4096 without acquiring references, KeyNotFound on missing key → `src/lib.rs:344-351`
- FR-024: recover_extent(key, offset, size_blocks) inserts BlockDevice entry, returns AlreadyExists if key present, tracked by eviction policy → `src/lib.rs:460-483`
- SC-001: All committed extents recoverable via initialize() with for_each_extent → `src/lib.rs:90-103`
- SC-002: Concurrent readers experience no data corruption or deadlocks — enforced by Mutex/Condvar design in `src/state.rs`
- SC-003: Write-to-read downgrade is atomic in single critical section → `src/lib.rs:281-303`
- SC-004: Per-entry metadata compact; size varies between BlockDevice (offset: u64) and MemoryTier (pointer + size + ssd_offset) → `src/entry.rs:27-35`
- SC-005: Lookup completes without blocking when no writer is active — condvar satisfied immediately → `src/lib.rs:114-119`
- SC-006: All ref count operations maintain consistent counts — no leaks or underflows → verified across all methods

#### Drifted
- **FR-018 (minor)**: `create_memory_tier_entry` stores `size_blocks: size.div_ceil(4096)` which interprets `size` as bytes and divides by 4096 to get blocks. However FR-023 says `entry_size` returns `size_blocks * 4096`. This means if `size` is not block-aligned (e.g., 5000 bytes), size_blocks becomes 2, and entry_size returns 8192 (rounded up). The spec notes this behavior in FR-023 ("for memory-tier entries where the original size was not block-aligned, the returned value is rounded up to the nearest block boundary") but the MemoryTier's raw `size` field in the Location variant still stores the original byte count passed to create_memory_tier_entry.
  - Location: src/lib.rs:387
  - Severity: **informational** — documented rounding behavior, not a bug

## Unspecced Code
- `entry_size()` module-level free function: Returns `std::mem::size_of::<DispatchEntry>()` — the struct size in bytes. Defined at `src/lib.rs:16-18`. Intended for benchmarks/assertions. Distinct from the FR-023 `entry_size(key)` instance method which returns the data size for a specific key. Left unspecced intentionally as a diagnostic utility.

## Recommendations

1. **FR-018 rounding note**: The minor informational drift about size_blocks rounding is already documented in FR-023. No action needed unless the size field semantics change.

2. **Module-level entry_size() utility**: This is a benchmark/diagnostic utility returning struct size. Left unspecced intentionally.
