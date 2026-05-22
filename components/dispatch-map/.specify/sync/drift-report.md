# Spec Drift Report

Generated: 2026-05-21
Project: dispatch-map v0

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 27 (FR: 21, SC: 6) |
| Aligned | 24 (89%) |
| Drifted | 3 (11%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 1 |

## Detailed Findings

### Spec: 001-dispatch-map - Dispatch Map

#### Aligned

- **FR-001**: CacheKey = u64 -> `pub type CacheKey = u64` in `idispatch_map.rs`
- **FR-002**: Per-entry metadata with location enum, size_blocks (u32), read_ref (u32), write_ref (u32), tsc (u64), protected by Mutex/Condvar. All fields present in `DispatchEntry`.
- **FR-003**: `create_staging` validates size != 0, checks for duplicate key, allocates DMA buffer via injected allocator, records entry with write_ref=1, returns Arc<DmaBuffer>.
- **FR-005**: `convert_to_storage` transitions Staging to BlockDevice; on MemoryTier sets `ssd_offset` field. Decrements read_ref by 1 as side effect.
- **FR-006**: `take_read` waits with DEFAULT_TIMEOUT (2000ms) until write_ref=0, then increments read_ref. Returns Timeout error on deadline expiry.
- **FR-007**: `take_write` waits with DEFAULT_TIMEOUT (2000ms) until read_ref=0 AND write_ref=0, then sets write_ref=1. Returns Timeout error on deadline expiry.
- **FR-008**: `release_read` decrements read_ref; returns RefCountUnderflow error if already 0.
- **FR-009**: `release_write` sets write_ref to 0; returns RefCountUnderflow error if already 0.
- **FR-010**: `downgrade_reference` atomically sets write_ref=0 and increments read_ref under same lock. Returns NoWriteReference error if write_ref is 0.
- **FR-011**: `remove` checks for active references (returns ActiveReferences error if any), then deletes entry. Returns KeyNotFound for missing keys.
- **FR-012**: `initialize()` calls `IExtentManager::for_each_extent` and populates entries as BlockDevice locations with zero ref counts.
- **FR-013**: All methods are thread-safe via Mutex+Condvar. Integration tests confirm multi-threaded concurrent access correctness.
- **FR-014**: ILogger usage throughout (info for init/recovery, debug for operations).
- **FR-015**: `define_component!` with IDispatchMap provided, ILogger and IExtentManager as receptacles.
- **FR-016**: `touch(key)` updates TSC timestamp, returns KeyNotFound if key missing, does not modify ref counts.
- **FR-017**: `oldest_keys(n)` returns up to n keys sorted by ascending TSC. Thread-safe (acquires lock).
- **FR-019**: `set_dma_alloc(alloc)` method injects DMA allocator. Stored in `Mutex<Option<DmaAllocFn>>`.
- **FR-020**: `initialize()` is an explicit public API call, not invoked during construction. Rebuilds from extent manager.
- **FR-021**: `convert_to_storage` on MemoryTier entry sets `ssd_offset` field rather than transitioning to BlockDevice.
- **SC-001**: Recovery test (`recovery_populated`) confirms 100% of extent-manager extents appear in map after initialization.
- **SC-002**: Integration tests (`multiple_readers_concurrent`, `concurrent_readers_and_writer_on_different_keys`) confirm no corruption or deadlocks.
- **SC-003**: Downgrade is atomic (single mutex acquisition, write_ref=0 + read_ref+=1 in same critical section). Test `downgrade_unblocks_pending_readers` confirms no window.
- **SC-005**: Lookup does not block when no writer is active (wait_for returns immediately when write_ref=0). Benchmark `lookup_no_contention` confirms.
- **SC-006**: Tests confirm no leaks or underflows (`release_read_underflow`, `release_write_underflow`, `lookup_acquires_read_ref`).

#### Drifted

- **FR-004**: Spec says lookup returns `BlockDevice(offset, size)`. Implementation's `LookupResult::BlockDevice` only contains `offset: u64` — no `size` field. The `size_blocks` stored in `DispatchEntry` is not exposed in the lookup result. Additionally, `MismatchSize` variant exists in the enum but spec says "for future use but is not currently triggered" — this matches, but the missing size in BlockDevice result is a gap.
  - Location: `components/interfaces/src/idispatch_map.rs` (LookupResult::BlockDevice variant)
  - Severity: minor (callers can track size independently; the entry stores it internally)

- **FR-018**: Spec says MemoryTier fields are `pointer: u64, size: usize, ssd_offset: Option<u64>`. Implementation uses `pointer: *mut u8, size: u32, ssd_offset: Option<u64>`. The pointer type (`*mut u8` vs `u64`) and size type (`u32` vs `usize`) differ from the spec. The interface method `create_memory_tier_entry` also takes `pointer: *mut u8, size: u32` rather than `pointer: u64, size: usize`.
  - Location: `components/dispatch-map/src/entry.rs` (Location::MemoryTier), `components/interfaces/src/idispatch_map.rs`
  - Severity: minor (using raw pointer is more idiomatic Rust than u64 cast; u32 for size aligns with the block-based size convention used elsewhere)

- **SC-004**: Spec says "The DispatchEntry struct size varies by Location variant (the Staging variant includes an Arc<DmaBuffer>)". The benchmark asserts size <= 56 bytes. The actual struct contains: Location enum (Staging has Arc = 16 bytes, BlockDevice has u64 = 8 bytes, MemoryTier has pointer+u32+Option<u64> = ~24 bytes + discriminant), size_blocks (u32), read_ref (u32), write_ref (u32), tsc (u64). This is larger than compact for the non-Staging variants but the spec's revised language merely says "varies by variant" which is factually correct. The benchmark's 56-byte bound is an implementation detail not in the spec.
  - Location: `components/dispatch-map/src/entry.rs`, `benches/dispatch_map_benchmark.rs`
  - Severity: informational (spec language is now permissive; implementation is reasonable)

#### Not Implemented

(none)

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `entry_size()` public helper function for benchmarks | src/lib.rs | ~4 | Not needed in spec (test/bench utility) |

## Recommendations

1. **FR-004 BlockDevice size in LookupResult**: Consider adding `size_blocks: u32` to `LookupResult::BlockDevice` if callers need to know the extent size from a lookup. Alternatively, document in the spec that size is tracked internally but not exposed in the lookup result.

2. **FR-018 pointer/size types**: Update the spec to match the implementation's more idiomatic types (`*mut u8` instead of `u64`, `u32` instead of `usize`), or update implementation to match the spec. The `*mut u8` approach is safer in Rust (no pointer-to-integer roundtrip needed). Recommend updating the spec.

3. **SC-004 entry size**: The spec's current language is permissive ("varies by variant"). No action needed unless a hard size constraint is desired.
