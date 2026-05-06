# Spec Drift Report

Generated: 2026-05-05
Project: dispatch-map v0

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 21 |
| Aligned | 17 (81%) |
| Drifted | 4 (19%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 4 |

## Detailed Findings

### Spec: 001-dispatch-map - Dispatch Map

#### Aligned

- FR-001: CacheKey = u64 -> `pub type CacheKey = u64`
- FR-003: create_staging validates size != 0, checks duplicate, allocates buffer, sets write_ref=1
- FR-005: convert_to_storage one-way transition from Staging to BlockDevice
- FR-008: release_read decrements, error on underflow
- FR-009: release_write sets to 0, error on underflow
- FR-010: downgrade_reference atomic write_ref=0, read_ref+=1 under same lock
- FR-011: remove errors if active references, removes entry
- FR-012: Recovery via for_each_extent — initialize() walks extents, rebuilds map
- FR-013: Thread-safe via Mutex + Condvar
- FR-014: ILogger usage throughout
- FR-015: define_component! with IDispatchMap provided, ILogger + IExtentManager receptacles
- SC-001: Extents recoverable after restart
- SC-002: Concurrent readers supported
- SC-003: Downgrade atomic
- SC-005: Lookup no-block when no writer active
- SC-006: Consistent ref counts

#### Drifted

- FR-002: Spec requires per-entry metadata with `extent_manager_id` and `block_device_id` fields. Implementation is missing these fields but has an unspecced `tsc` field for LRU ordering.
  - Location: src/lib.rs (DispatchEntry struct)
  - Severity: moderate

- FR-004: Spec requires `lookup(key, timeout)` with caller-supplied timeout parameter. Implementation uses hardcoded DEFAULT_TIMEOUT (2000ms) with no caller-supplied timeout.
  - Location: src/lib.rs (lookup method)
  - Severity: moderate

- FR-006: Spec requires `take_read(key, timeout)` with caller-supplied timeout. Same hardcoded timeout issue.
  - Location: src/lib.rs (take_read method)
  - Severity: moderate

- SC-004: Spec requires entry size <= 32 bytes. Actual entry is ~36+ bytes due to `tsc` field (8 bytes) and enum padding.
  - Location: src/lib.rs (DispatchEntry struct)
  - Severity: minor

#### Not Implemented

(none)

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| oldest_keys method (LRU key retrieval for eviction) | src/lib.rs | ~20 | Add to 001 spec |
| tsc field in DispatchEntry (timestamp for LRU) | src/lib.rs | ~5 | Add to 001 spec |
| set_dma_alloc as separate interface method | src/lib.rs | ~10 | Add to 001 spec |
| DEFAULT_TIMEOUT = 2000ms (spec assumed 100ms) | src/lib.rs | ~3 | Update 001 spec assumptions |

## Recommendations

1. **Decide on timeout API (FR-004/006)**: Either add timeout parameter to the interface methods or update spec to document fixed internal timeout. This affects the IDispatchMap interface contract.
2. **Decide on entry metadata (FR-002)**: The `tsc` field is needed for LRU eviction but `extent_manager_id`/`block_device_id` may not be needed if drive selection is deterministic from key.
3. **Backfill LRU/eviction support**: The `oldest_keys` and `tsc` features enable eviction and should be specified.
4. **Backfill set_dma_alloc**: This is a required initialization step that should be in the spec.
