# Drift Report: extent-manager

**Spec**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`
**Generated**: 2026-05-21
**Status**: 2 Drifted | 28 Aligned | 0 Not Implemented

## Summary Table

| Requirement | Status | Notes |
|-------------|--------|-------|
| FR-001 | Drifted | Component named `ExtentManager`, not `ExtentManagerV2` |
| FR-002 | Aligned | All FormatParams validated |
| FR-003 | Aligned | Superblock written at LBA 0 with CRC32 |
| FR-004 | Aligned | initialize reads superblock, validates magic/CRC, recovers |
| FR-005 | Aligned | reserve_extent returns WriteHandle with offset |
| FR-006 | Aligned | FREE_KEY publish silently frees slot |
| FR-007 | Aligned | abort/drop releases slot |
| FR-008 | Aligned | remove_extent sets FREE_KEY, deferred free |
| FR-009 | Aligned | get_extents and for_each_extent filter FREE_KEY |
| FR-010 | Aligned | Slab has dense Vec<u64> keys parallel to bitmap |
| FR-011 | Aligned | FREE_KEY = u64::MAX used as sentinel |
| FR-012 | Aligned | BTreeMap<u64, Slab> keyed by start_offset |
| FR-013 | Aligned | checkpoint serializes all regions with CRC32 |
| FR-014 | Aligned | checkpoint skips if no dirty regions |
| FR-015 | Aligned | Coalescing via CheckpointCoalesce state machine |
| FR-016 | Aligned | Background thread with 300s default interval |
| FR-017 | Aligned | Recovery tries active, falls back to inactive |
| FR-018 | Aligned | Recovery rebuilds bitmap from key vectors |
| FR-019 | Aligned | BuddyAllocator per region |
| FR-020 | Aligned | Bitmap allocator with rover |
| FR-021 | Aligned | SizeClassManager indexes slabs by element_size |
| FR-022 | Aligned | key & (region_count - 1) sharding |
| FR-023 | Aligned | parking_lot::RwLock per region |
| FR-024 | Aligned | Component is Send + Sync via Arc + RwLock |
| FR-025 | Aligned | pending_frees defers slot reuse until checkpoint |
| FR-026 | Aligned | get_instance_id implemented |
| FR-027 | Aligned | set_checkpoint_interval implemented |
| FR-028 | Aligned | set_metadata_ns_id implemented |
| FR-029 | Aligned | set_dma_alloc implemented |
| FR-030 | Aligned | volatile_write_cache feature gate present |
| SC-001 | Aligned | Integration tests cover full lifecycle |
| SC-002 | Aligned | Checkpoint/recovery tests verify round-trip |
| SC-003 | Aligned | corrupt_active_falls_back_to_previous test |
| SC-004 | Aligned | Concurrent tests with 8 threads |
| SC-005 | Drifted | No explicit large-scale test for 100M extents |
| SC-006 | Aligned | Coalescing logic limits concurrent I/O |

## Detailed Findings

### FR-001 (Drifted)

**Spec**: "The component MUST be named ExtentManagerV2"
**Implementation**: The component is defined as `ExtentManager` (not `ExtentManagerV2`) via `define_component! { pub ExtentManager { ... } }` in `src/lib.rs:66`.

The spec explicitly requires the name `ExtentManagerV2`, but the codebase uses `ExtentManager`. This appears to be a deliberate simplification during implementation (the crate is already named `extent-manager` and there is no V1 in the workspace). The version field is set to `"0.3.0"`.

**Impact**: Low. The naming difference does not affect functionality. If spec compliance is required, either the spec or the code should be updated to match.

### SC-005 (Drifted)

**Spec**: "The component supports approximately 100 million extents on a 10 TB data device with 128 KiB extent size"
**Implementation**: No test exercises this scale. The existing benchmarks and tests use small disk sizes (64-256 MiB). The architecture (BTreeMap, buddy allocator, bitmap) should support this scale, but it is not validated.

**Impact**: Medium. This is a scalability criterion that requires dedicated benchmarking or stress-testing infrastructure.

### Additional Code Without Spec Coverage

The following features exist in the implementation but are not mentioned in the spec:

1. **`used_bytes()` and `capacity_bytes()`** - IExtentManager interface methods that report allocation statistics. These are utility methods not in the spec's requirement list.

2. **`FaultConfig` / fault injection in MockBlockDevice** - Test infrastructure for simulating I/O failures. Not a spec concern but supports robustness testing.

3. **Superblock `version` field is 5** - The spec documents version 4 in the superblock layout (and magic "CERTUSV4"), but `FORMAT_VERSION` in `superblock.rs` is set to 5. The magic remains `0x4345_5254_5553_5634` ("CERTUSV4") as spec'd. This version field drift could cause confusion if external tools rely on the spec's version number.

4. **`WriteHandle` is defined in `interfaces` crate** - The spec describes WriteHandle as a component entity, but it is actually defined in the shared interfaces crate (`iextent_manager.rs`). This is an architectural choice for sharing types across the interface boundary and is functionally correct.

## Recommendations

1. **Resolve naming drift (FR-001)**: Either rename the component to `ExtentManagerV2` or update the spec to reflect the actual name `ExtentManager`. Given there is no V1, updating the spec is the simpler path.

2. **Add scale test (SC-005)**: Create a benchmark or integration test that exercises the component with a large simulated data device (e.g., 10 TB emulated via block counting without actual I/O) to validate memory overhead and lookup performance at scale.

3. **Reconcile FORMAT_VERSION**: The spec says version 4 but code uses version 5. Update the spec's on-disk format reference to reflect the actual version number, or document the version history.

4. **Document utility methods**: Add `used_bytes()` and `capacity_bytes()` to the spec as informational/monitoring requirements, since they are part of the public interface.
