# Spec Drift Report

Generated: 2026-05-05
Project: Extent Manager V2
Spec: 001-extent-manager-v2

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 25 |
| Aligned | 22 (88%) |
| Drifted | 3 (12%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 0 |

## Detailed Findings

### Spec: 001-extent-manager-v2 - Extent Manager V2

#### Aligned

- FR-001: Component named ExtentManagerV2, uses define_component!, provides IExtentManager, has metadata_device receptacle
- FR-002: format() validates all FormatParams: sector_size > 0, slab_size multiple of sector_size, max_extent_size <= slab_size, region_count is power of two
- FR-003: format() writes superblock at LBA 0 with format parameters, layout, and CRC32 checksum
- FR-004: initialize() reads superblock, validates magic/CRC, recovers from active checkpoint, rebuilds allocation state
- FR-005: reserve_extent(key, size) allocates sector-aligned slot, returns WriteHandle; not visible until publish()
- FR-006: WriteHandle::publish() writes key into slab key vector; FREE_KEY causes immediate free
- FR-007: WriteHandle::abort() releases slot without writing key; Drop calls abort_fn
- FR-008: remove_extent sets key to FREE_KEY, deferred free via pending_frees until post-checkpoint
- FR-009: get_extents() and for_each_extent() return/iterate only non-FREE_KEY entries
- FR-010: Each Slab has dense Vec<u64> keys parallel to bitmap slots; no separate HashMap
- FR-011: FREE_KEY = u64::MAX sentinel for unoccupied slots
- FR-012: Slabs in BTreeMap<u64, Slab> keyed by start_offset; range query for O(log n) lookup
- FR-013: checkpoint() serializes slab descriptors + key vectors, CRC32 protected, writes to inactive region, switches superblock
- FR-014: checkpoint() returns early if no region is dirty
- FR-015: Concurrent checkpoint() calls coalesced via CheckpointCoalesce state machine
- FR-017: Recovery attempts active copy first, falls back to inactive on CRC failure
- FR-018: After checkpoint + reboot, initialize() restores extents from slab key vectors
- FR-019: Each region uses BuddyAllocator for slab-sized chunk allocation
- FR-020: Each slab uses AllocationBitmap with rover for even distribution
- FR-021: SizeClassManager indexes slabs by element size
- FR-022: Keys sharded to regions by key & (region_count - 1)
- FR-023: Regions independently locked via parking_lot::RwLock
- FR-025: Deferred free via pending_frees; slot not released until post-checkpoint

#### Drifted

- FR-016: Spec says default checkpoint interval is 5000 ms. Code uses 300 seconds (5 minutes) via `Duration::from_secs(300)`.
  - Location: src/lib.rs:94
  - Severity: moderate (spec likely has typo — 5s is too aggressive for production)

- Superblock Magic: Spec defines magic as 0x4345_5254_5553_5635 ("CERTUSV5"). Code uses 0x4345_5254_5553_5634 ("CERTUSV4").
  - Location: src/superblock.rs:5
  - Severity: minor (internal consistency maintained, but spec/code disagree)

- FR-024: Spec requires component MUST be Send + Sync. Component is Send + Sync via auto-derivation but has no explicit compile-time assertion.
  - Location: src/lib.rs
  - Severity: minor (functionally correct, lacks guard against future breakage)

#### Not Implemented

(none)

### Success Criteria

- SC-001 through SC-006: All aligned

## Recommendations

1. **FR-016**: Update spec to 300 seconds (5 minutes) — a 5-second checkpoint interval would generate excessive I/O.
2. **Superblock magic**: Either update code to V5 or spec to V4. Since spec represents the intended design, updating code is preferred.
3. **FR-024**: Add compile-time Send + Sync assertion to guard against future field additions.
