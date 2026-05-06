# Spec Drift Report

Generated: 2026-05-05
Project: dispatcher v0

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 26 |
| Aligned | 22 (85%) |
| Drifted | 2 (8%) |
| Not Implemented | 2 (8%) |
| Unspecced Code | 5 |

## Detailed Findings

### Spec: 001-dispatcher-cache-interface - Dispatcher Cache Interface

#### Aligned

- FR-001: IDispatcher interface with all required methods -> `interfaces/src/idispatcher.rs`
- FR-002: DispatcherError with all failure modes -> `interfaces/src/idispatcher.rs`
- FR-003: populate registers in dispatch map, allocates staging, DMA from GPU, downgrades ref, enqueues background write
- FR-004: Async SSD write via BackgroundWriter processing WriteJobs
- FR-005: Staging freed after write (release_read + convert_to_storage)
- FR-006: lookup handles both Staging and BlockDevice paths with DMA
- FR-007: Cache miss returns KeyNotFound for NotExist
- FR-008: check returns bool, no DMA
- FR-009: remove frees staging buffer and/or SSD extent
- FR-010: define_component! exposing IDispatcher
- FR-011: Receptacles: logger, dispatch_map, gpu_services, spdk_env
- FR-013: Thread safety via Mutex/Atomic, multi-thread tests
- FR-014: Shutdown completes in-flight ops, drains background writer
- FR-015: N drives supported, one BD + one EM per PCI address
- FR-018: Remove blocks during active write via condvar (take_write)
- FR-019: MDTS segmentation via io_segmenter module
- SC-001 through SC-007: All aligned

#### Drifted

- FR-012: Spec says "validate all receptacles are connected at initialize() time" but only dispatch_map is validated. gpu_services is checked lazily at first use.
  - Location: `src/lib.rs` (initialize method)
  - Severity: minor

- FR-016: Spec says "pass a unique PCI-derived identifier to each extent manager" but implementation passes data_disk_size without a PCI-derived unique ID.
  - Location: `src/lib.rs` (initialize method, drive setup loop)
  - Severity: minor

#### Not Implemented

- FR-017: Background write failure should clean up dispatch map entry and release read reference. Current code returns without cleanup on failure, leaking the entry.
  - Location: `src/background_writer.rs` (process_write_job error path)
  - Severity: moderate

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| prepare_store/commit_store/cancel_store (two-phase store API) | src/lib.rs | ~80 | Add to 001 spec |
| Eviction mechanism (run_eviction_cycle, watermark) | src/lib.rs | ~60 | Add to 001 spec |
| BlockDeviceVersion/ExtentManagerVersion enums | src/lib.rs | ~20 | Add to 001 spec |
| format_on_init config flag | src/lib.rs | ~10 | Add to 001 spec |
| PendingWrite struct | src/background_writer.rs | ~15 | Internal detail |

## Recommendations

1. **Fix FR-017 (moderate)**: Implement cleanup in background write error path — release_read and remove dispatch map entry on failure.
2. **Backfill two-phase store API**: The prepare_store/commit_store/cancel_store methods represent a significant feature not covered by any spec.
3. **Backfill eviction mechanism**: The LRU eviction with watermarks is production functionality that should be specified.
4. **Decide on FR-012/FR-016**: Minor drift — determine if lazy validation and size-based ID are intentional design choices (backfill) or gaps (align).
