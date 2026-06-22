# Spec Drift Report
Generated: 2026-06-18
Project: dispatcher

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 39 |
| Aligned | 36 (92%) |
| Drifted | 3 (8%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 2 |

Note: FR-026 and FR-027 are marked REMOVED in the spec and excluded from analysis.

## Detailed Findings
### Spec: 001-dispatcher-cache-interface - Dispatcher Cache Interface (Memory-Tier Architecture)

#### Aligned
- FR-001: IDispatcher interface defined in shared interfaces crate with all required methods (initialize, shutdown, lookup, lookup_async, batch_lookup, check, remove, populate, prepare_store, commit_store, cancel_store, touch, promote_to_memory_tier, clear_memory_tier, flush_to_ssd) → `components/interfaces/src/idispatcher.rs:153-506`
- FR-002: DispatcherError enum covers all failure modes (NotInitialized, KeyNotFound, AlreadyExists, AllocationFailed, IoError, Timeout, InvalidParameter) → `components/interfaces/src/idispatcher.rs:119-134`
- FR-003: populate() allocates memory-tier via insert(), DMA copies via dma_copy_to_host, registers via create_memory_tier_entry, enqueues write-through, runs evict_for_space → `src/lib.rs:1792-1903`
- FR-004: Background writer reads via peek() (no LRU refresh) and writes to SSD via extent manager, calls convert_to_storage → `src/lib.rs:544-617`
- FR-005: Memory-tier entry remains accessible after write-through (no slot free on write completion) → `src/lib.rs:614-616`
- FR-006: lookup() queries dispatch map, handles MemoryTier (memcpy_h2d_async + touch LRU), BlockDevice (promote_and_serve), Staging (DMA from buffer). Uses warm stream with fallback to sync dma_copy_to_device → `src/lib.rs:1609-1722`
- FR-007: lookup returns KeyNotFound when key not in dispatch map → `src/lib.rs:1639,1720-1721`
- FR-008: check() returns bool without data transfer → `src/lib.rs:1724-1743`
- FR-009: remove() frees memory-tier slot, frees extent via extent manager, removes dispatch map entry → `src/lib.rs:1745-1785`
- FR-010: Uses define_component! macro exposing only IDispatcher → `src/lib.rs:148-173`
- FR-011: Receptacles for ILogger, IDispatchMap, IGpuServices, ISPDKEnv, IMemoryTier declared. DispatcherConfig includes poller_base_cpu field. set_actor_cpu called when set → `src/lib.rs:148-173`, `interfaces/src/idispatcher.rs:59`
- FR-012: initialize() validates dispatch_map and memory_tier bound before proceeding → `src/lib.rs:930-936`
- FR-013: Dispatch map uses appropriate locking (trait-level contract enforced by IDispatchMap) → architecture
- FR-014: shutdown() drains background writer, checkpoints all extent managers, shuts down block devices in reverse → `src/lib.rs:1101-1177`
- FR-015: N block devices coordinated with N extent managers, each EM stores metadata on its own data device → `src/lib.rs:832-847`
- FR-016: Passes data_disk_size and computed FormatParams to each extent manager → `src/lib.rs:851-879`
- FR-017: Background writer silently drops failed jobs → `src/lib.rs:592-607`
- FR-018: remove() does not block on write-through → `src/lib.rs:1745-1785`
- FR-019: I/O segmented via io_segmenter respecting MDTS. Pipeline uses sliding window with tag-based completion routing. Periodic stream sync every PIPELINE_RING_SIZE=8 commands. batch_lookup uses 16/num_queues per thread → `src/pipeline.rs`, `src/io_segmenter.rs`
- FR-020: prepare_store validates size>0, AlreadyExists for duplicates, registers in dispatch map, reserves extent, returns DMA buffer → `src/lib.rs:1905-1999`
- FR-021: commit_store writes buffer to SSD with MDTS segmentation, publishes extent, transitions to block-device state → `src/lib.rs:2002-2041`
- FR-022: cancel_store drops PendingWrite (WriteHandle aborts), removes dispatch map entry → `src/lib.rs:2044-2064`
- FR-023: touch() updates dispatch map timestamp AND refreshes memory-tier LRU without DMA → `src/lib.rs:2066-2080`
- FR-024: evict_for_space uses sparse-probe + shard-targeted LRU (evict_lru_for_key). Every 8th attempt probes oldest_keys(4) + is_evictable. Bounded by max_attempts (configurable, default 2048). Returns AllocationFailed on exhaustion → `src/lib.rs:483-542`
- FR-025: format_on_init flag supported. When false, extent managers recovered; dispatch map reconstructed via for_each_extent + recover_extent. Count and elapsed time logged → `src/lib.rs:950-969`
- FR-028: On BlockDevice lookup, re-inserts to memory-tier and re-registers as MemoryTier with ssd_offset preserved → `src/lib.rs:451-459`
- FR-029: Background SSD evictor started if threshold > 0.0 and drives configured. Joined during shutdown() → `src/lib.rs:1062-1093,1104-1106`
- FR-030: Evictor checks combined utilization. Interval configurable via ssd_eviction_interval_secs → `src/background.rs:270-283,342-349`
- FR-031: Evictor evicts BlockDevice entries using oldest_keys(batch_size), stops below low_watermark → `src/background.rs:295-330`
- FR-032: Evictor skips MemoryTier entries. dm.remove() failure handled gracefully → `src/background.rs:361-366,312-313`
- FR-033: DispatcherConfig includes all required fields with correct defaults → `interfaces/src/idispatcher.rs:30-79`
- FR-034: initialize() calls register_host_memory on memory-tier pool. Caches ClientChannels per drive → `src/lib.rs:1003-1023,896`
- FR-035: shutdown() calls unregister_host_memory before memory-tier teardown → `src/lib.rs:1127-1131`
- FR-038: clear_memory_tier() evicts all via evict_lru() loop, transitions to BlockDevice or removes on failure, returns count. IMemoryTier provides clear() → `src/lib.rs:2233-2254`
- FR-040: promote_to_memory_tier() implemented — BlockDevice entries read via pipelined_ssd_to_dram_only; MemoryTier/Staging entries get timestamp refresh; missing keys skipped. Errors logged not propagated → `src/lib.rs:2083-2231`
- FR-041: pipelined_ssd_to_dram_only and pipelined_multi_ssd_to_dram_only both implemented in pipeline.rs → `src/pipeline.rs:551-815`

#### Drifted
- FR-036: Spec says "The synchronous lookup method MUST delegate to lookup_async internally and call stream_synchronize before returning". Code correctly delegates lookup to lookup_async (line 1181-1190). However, spec also says "pre-allocate a pool of warm CUDA streams (default 4) stored as RwLock<Vec<u64>>". Code allocates only ONE warm stream stored as AtomicU64. The spec describes multi-stream round-robin distribution for batch_lookup; actual code uses a single stream for lookup_async and creates fresh per-thread streams for cold promotion.
  - Location: src/lib.rs:977-982 (single stream), AtomicU64 warm_stream field
  - Severity: moderate
  - Notes: FR-036 synchronous delegation is correct. FR-037 stream pool is the issue — single stream vs pool of 4.

- FR-037: Spec says "MUST pre-allocate a pool of warm CUDA streams (default 4)" stored as RwLock<Vec<u64>> and used by lookup_async (first stream) and batch_lookup (round-robin across all streams). Code allocates a SINGLE warm stream stored as AtomicU64 (not a pool of 4). batch_lookup hot path uses the same single warm_stream. Multi-stream distribution as described in spec is not implemented.
  - Location: src/lib.rs:977-982 (single stream creation), line 1273 (single warm_stream load in batch_lookup)
  - Severity: moderate

- FR-039: Spec says batch_lookup hot entries should issue memcpy_h2d_async round-robin across warm stream pool "WITHOUT synchronizing per-key — all async copies are issued first, then all used streams are synchronized once at the end (deferred batch sync)". Code synchronizes per-key for hot entries in batch_lookup (stream_synchronize at line 1289 inside the per-entry MemoryTier branch). Hot entries do NOT benefit from deferred batch sync as specified.
  - Location: src/lib.rs:1286-1292 (per-key sync instead of deferred batch sync)
  - Severity: moderate

#### Not Implemented
(none)

## Unspecced Code
- **flush_to_ssd()**: Method on IDispatcher interface that blocks until all background write-through jobs complete. Defined in the interface and implemented, but no FR-* requirement covers it. → `src/lib.rs:2256-2269`, `interfaces/src/idispatcher.rs:497-505`
- **PipelineMetrics trait and instrumentation**: Comprehensive timing instrumentation (record_cold_ssd_read, record_hot_gpu_dma, record_populate_alloc, etc.) integrated throughout lib.rs and pipeline.rs. No FR-* covers observability or metrics. → `src/metrics.rs:1-48`, call sites throughout `src/lib.rs` and `src/pipeline.rs`
