# Spec Drift Report

Generated: 2026-05-20
Project: dispatcher-v1
Spec: 001-dispatcher-cache-interface (Memory-Tier Architecture)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 35 |
| Aligned | 34 (97%) |
| Drifted | 1 (3%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 0 |

## Detailed Findings

### Spec: 001-dispatcher-cache-interface - Dispatcher Cache Interface (Memory-Tier Architecture)

#### Aligned

- FR-001: IDispatcher interface with all required methods -> `interfaces/src/idispatcher.rs:168-233`
- FR-002: DispatcherError covering all failure modes -> `interfaces/src/idispatcher.rs:134-165`
- FR-003: populate with memory-tier insert + DMA + create_memory_tier_entry + evict_for_space + background write -> `src/lib.rs` (populate method)
- FR-004: Async write-through via BackgroundWriter -> `src/background.rs:33-95`, `src/lib.rs:620-650`
- FR-005: Memory-tier slot NOT freed on write-through completion -> confirmed in bg_writer closure
- FR-006: lookup with MemoryTier/BlockDevice/Staging paths -> `src/lib.rs` (lookup method)
- FR-007: lookup returns KeyNotFound for missing keys -> LookupResult::NotExist branch
- FR-008: check returns presence without data transfer -> `src/lib.rs` (check method)
- FR-009: remove frees memory-tier + extent + dispatch-map -> `src/lib.rs` (remove method)
- FR-010: define_component! with IDispatcher -> `src/lib.rs:52-100`
- FR-011: receptacles for ILogger, IDispatchMap, IGpuServices, ISPDKEnv, IMemoryTier -> `src/lib.rs:52-70`
- FR-012: initialize validates dispatch_map and memory_tier bound -> `src/lib.rs:500-520`
- FR-013: Thread safety via dispatch-map locking semantics -> IDispatchMap take_read/take_write/release_*
- FR-014: shutdown drains background writer before returning -> `src/lib.rs` (shutdown method)
- FR-015: N data block devices + N extent managers -> `src/lib.rs:550-620`
- FR-016: FormatParams passed to extent managers -> `src/lib.rs:580-600`
- FR-017: Background write-through silently drops on failure (entry remains in MemoryTier) -> confirmed in bg_writer closure
- FR-018: remove does NOT block on write-through -> confirmed (no condvar wait)
- FR-019: MDTS-aware segmentation + zero-copy pipeline (primary) / ring-buffer fallback -> `src/io_segmenter.rs`, `src/pipeline.rs`
- FR-020: prepare_store with eviction + extent + dispatch-map + DMA buffer -> `src/lib.rs` (prepare_store)
- FR-021: commit_store with segmented write + publish + convert -> `src/lib.rs` (commit_store)
- FR-022: cancel_store drops WriteHandle + removes dispatch-map entry -> `src/lib.rs` (cancel_store)
- FR-023: touch updates timestamp only, no DMA -> `src/lib.rs` (touch method)
- FR-024: Capacity-based memory-tier eviction via evict_for_space + evict_lru -> `src/lib.rs`
- FR-025: format_on_init flag in DispatcherConfig -> `interfaces/src/idispatcher.rs:64`
- FR-026: BlockDeviceVersion selection (V1, V2) -> `interfaces/src/idispatcher.rs:13-19`
- FR-027: ExtentManagerVersion selection -> `interfaces/src/idispatcher.rs:22-27`
- FR-028: Pipelined promotion re-inserts into memory-tier -> `src/lib.rs` (promote_and_serve)

#### Drifted

- FR-019: **RESOLVED** (spec updated 2026-05-20). Spec now describes zero-copy pipeline as primary path with ring-buffer fallback.

- FR-024: Spec says "Count-based TSC eviction (from v0) is NOT used in v1" but `DispatcherConfig` still includes `max_cache_entries` (default: 10000) and `eviction_threshold` (default: 0.8) fields. These are vestigial from v0; the v1 memory-tier eviction is purely capacity-based via `IMemoryTier::used()/capacity()`. The fields are not referenced in the eviction logic.
  - Location: `interfaces/src/idispatcher.rs:58-61`
  - Severity: minor

#### Not Implemented

(none)

### Unspecced Code

(none — all features specced as of 2026-05-20 via FR-006 update, FR-019 update, FR-034, FR-035)

## Inter-Spec Conflicts

None.

## Recommendations

1. **Update FR-019 for zero-copy pipeline** (priority: high): The spec describes a ring-buffer-based pipelined reader. The implementation now uses a zero-copy path that reads NVMe directly into the memory-tier slot and streams to GPU from the same memory. Update the spec to describe the zero-copy algorithm as the primary path, with the ring-buffer path as fallback for unregistered memory.

2. **Add FR-034/035/036 for pool registration and async DMA** (priority: medium): The memory-tier pool CUDA+SPDK registration at init and the async warm-path DMA are significant performance features (~4x warm-path improvement, ~3.5x cold-path improvement) that should be specified.

3. **Remove or document legacy config fields** (priority: low): `max_cache_entries` and `eviction_threshold` are unused in v1. Either remove them (breaking change for callers using `..Default::default()`) or mark them as deprecated in the doc comment.
