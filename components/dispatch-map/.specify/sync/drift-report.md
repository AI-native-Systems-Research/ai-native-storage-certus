# Spec Drift Report
Generated: 2026-06-18
Project: dispatch-map

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 23 |
| Aligned | 21 (91%) |
| Drifted | 2 (9%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 2 |

## Detailed Findings
### Spec: 001-dispatch-map - Dispatch Map Component
#### Aligned
- FR-001: CacheKey type as u64 → `interfaces::CacheKey` (re-exported in src/lib.rs:27)
- FR-002: Per-entry metadata (location, size_blocks, read_ref, write_ref, tsc) with Mutex/Condvar → `src/entry.rs:48` (DispatchEntry) and `src/state.rs:21` (DispatchMapState with Mutex/Condvar)
- FR-003: create_staging with DMA buffer, write_ref=1, error on size=0 and alloc failure → `src/lib.rs:102-141`
- FR-004: lookup returns NotExist/Staging/BlockDevice/MemoryTier, increments read_ref, blocks on write_ref with 2000ms timeout, refreshes TSC → `src/lib.rs:143-183`
- FR-005: convert_to_storage transitions Staging to BlockDevice, sets ssd_offset on MemoryTier, conditional read_ref decrement, error on already BlockDevice → `src/lib.rs:185-219`
- FR-006: take_read waits for write_ref=0 with 2000ms timeout, increments read_ref → `src/lib.rs:222-245`
- FR-007: take_write waits for read_ref=0 and write_ref=0 with 2000ms timeout → `src/lib.rs:247-269`
- FR-008: release_read decrements read_ref, error on underflow → `src/lib.rs:272-289`
- FR-009: release_write decrements write_ref, error on underflow → `src/lib.rs:292-309`
- FR-010: downgrade_reference atomically transitions write to read, error if no write ref → `src/lib.rs:312-334`
- FR-011: remove deletes entry, error on active references → `src/lib.rs:336-354`
- FR-012: initialize recovers from IExtentManager via for_each_extent, returns Ok(()) when unbound → `src/lib.rs:60-99`
- FR-013: All methods thread-safe via Mutex<Inner> and Condvar blocking → `src/state.rs` (all state behind Mutex)
- FR-014: ILogger used for info, debug, and error logging → used in set_dma_alloc, initialize, create_staging, lookup, convert_to_storage, take_read, take_write, release_read, release_write, downgrade_reference, remove, create_memory_tier_entry, convert_memory_tier_to_block
- FR-015: define_component! with IDispatchMap provider, ILogger and IExtentManager receptacles → `src/lib.rs:33-45`
- FR-016: touch updates TSC, returns KeyNotFound, no ref count changes → `src/lib.rs:356-363`
- FR-017: oldest_keys returns up to n keys sorted by ascending TSC → `src/lib.rs:375-384`
- FR-019: set_dma_alloc stores injected allocator → `src/lib.rs:48-54`
- FR-020: initialize() is explicit public API, not called during construction → `src/lib.rs:60-99` (must be called explicitly after binding)
- FR-021: convert_to_storage on MemoryTier sets ssd_offset, does not transition to BlockDevice → `src/lib.rs:196-198`
- FR-022: is_evictable checks MemoryTier + ssd_offset: Some + read_ref==0 + write_ref==0, returns false otherwise → `src/lib.rs:465-478`
- FR-023: entry_size returns size_blocks * 4096, KeyNotFound on missing key → `src/lib.rs:366-373`

#### Drifted
- FR-018: Spec says MemoryTier fields are `pointer: *mut u8, size: u32, ssd_offset: Option<u64>` and two methods: `create_memory_tier_entry(key, pointer, size)` and `convert_memory_tier_to_block(key, offset)`. Implementation's `convert_memory_tier_to_block` takes only `(key)` — it reads the offset from the `ssd_offset` field already set by `convert_to_storage` rather than accepting an `offset` parameter. The spec says the method signature includes an `offset` parameter but the implementation does not.
  - Location: src/lib.rs:424 (`fn convert_memory_tier_to_block(&self, key: CacheKey)`)
  - Severity: minor (the implementation correctly reads the already-stored ssd_offset; the spec's "offset" parameter in FR-018 is inconsistent with the actual design where convert_to_storage sets ssd_offset first)

- SC-004: Spec says "The DispatchEntry struct size varies by Location variant (the Staging variant includes an Arc<DmaBuffer>)". This is factually correct but informational — the benchmark asserts size <= 56 bytes which is not specified.
  - Location: src/entry.rs, benches/dispatch_map_benchmark.rs
  - Severity: informational

#### Not Implemented
(none)

## Unspecced Code
- `recover_extent(key, offset, size_blocks)`: A standalone method that inserts a BlockDevice entry directly without going through create_staging or initialize(). Present in the IDispatchMap interface and implemented at `src/lib.rs:480-499`. Not covered by any FR-* requirement. Used for incremental recovery or external extent injection.
- `entry_size()` free function (module-level): Returns `std::mem::size_of::<DispatchEntry>()` — the struct size in bytes for benchmarks/assertions. Defined at `src/lib.rs:16-18`. Distinct from the FR-023 `entry_size(key)` instance method.

## Recommendations

1. **FR-018 convert_memory_tier_to_block signature**: Update spec FR-018 to clarify that `convert_memory_tier_to_block(key)` does not accept an `offset` parameter — it uses the `ssd_offset` already set by a prior `convert_to_storage` call. Alternatively, consider whether the spec's intent was a different method signature.

2. **Unspecced recover_extent**: Add a new requirement (e.g., FR-024) to cover `recover_extent(key, offset, size_blocks)` which directly inserts a BlockDevice entry. This is used by the dispatcher for incremental recovery paths that bypass the full `initialize()` walk.

3. **Module-level entry_size()**: This is a test/benchmark utility and may not need formal spec coverage. Consider documenting it as a diagnostic API if it should remain public.
