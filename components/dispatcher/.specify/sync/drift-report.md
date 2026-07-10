# Spec Drift Report

Generated: 2026-07-10
Project: dispatcher (components/dispatcher)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 42 (active FR-* and SC-*; excluding 5 REMOVED FRs and 1 REMOVED SC) |
| Aligned | 42 (100%) |
| Drifted | 0 (0%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 0 |

## Detailed Findings

### Spec: 001-dispatcher-cache-interface - Dispatcher Cache Interface (Memory-Tier Architecture)

#### Aligned (no drift)

- FR-001: IDispatcher interface defined with all required methods (initialize, shutdown, lookup, lookup_async, batch_lookup, check, remove, populate, touch, promote_to_memory_tier, plus reserve_memory, copy_gpu_to_memory_async, copy_gpu_to_memory_completed, release_memory, pin, unpin, clear_memory_tier, flush_to_ssd)
- FR-002: DispatcherError type in interfaces crate covers all modes (NotInitialized, KeyNotFound, AlreadyExists, AllocationFailed, IoError, Timeout, InvalidParameter)
- FR-003: populate() evicts if needed, allocates memory-tier via insert(), DMA copies D2H via warm stream, registers in dispatch-map via create_memory_tier_entry, enqueues write-through
- FR-004: Background writer reads via peek() (no LRU refresh), writes to SSD via segmented I/O, calls convert_to_storage on success
- FR-005: Memory-tier entry remains accessible after write-through (slot not freed on completion)
- FR-006: lookup uses memcpy_h2d_async on warm stream for MemoryTier path (raw pointer, no DmaBuffer wrapping overhead); BlockDevice path uses pipelined_ssd_to_gpu_zero_copy for promotion; fallback to sync dma_copy_to_device when no stream available
- FR-007: lookup returns KeyNotFound for missing keys
- FR-008: check() queries dispatch-map, returns bool without DMA
- FR-009: remove() frees memory-tier slot (mt.remove), dispatch-map entry (dm.remove), and SSD extent (extent_mgr.remove_extent)
- FR-010: Uses define_component! macro, provides [IDispatcher]
- FR-011: Receptacles for ILogger, IDispatchMap, IGpuServices, ISPDKEnv, IMemoryTier, IRemoteLookup. poller_base_cpu field supported with per-drive CPU pinning via set_actor_cpu
- FR-012: initialize() validates dispatch_map and memory_tier bound before proceeding; rejects empty data_pci_addrs with InvalidParameter
- FR-013: Thread-safe via dispatch-map read/write reference protocol
- FR-014: shutdown() drains ParallelBackgroundWriter, checkpoints extent managers via checkpoint(), two-phase block device shutdown (signal then join)
- FR-015: N block devices paired with N extent managers (DataDrive struct)
- FR-016: FormatParams computed from data partition size and passed to extent manager format()
- FR-017: Write-through failure silently drops (process_write_job returns early without propagating)
- FR-018: remove() does NOT block on background write-through; proceeds immediately. Interface doc updated to match.
- FR-019: io_segmenter provides MDTS-aware splitting. Zero-copy pipeline with configurable max_queue_depth (16 for single, 128 for batch). Tag-based completion routing. Periodic stream sync every PIPELINE_RING_SIZE=8 GPU commands
- FR-023: touch() calls dm.touch() + mt.touch(), no DMA, returns KeyNotFound if absent
- FR-024: evict_for_space uses shard-targeted LRU primary (evict_lru_for_key(target_key)) with clean-eviction probe every 8th iteration (oldest_keys(4) + is_evictable). Bounded by max_eviction_attempts (default 2048)
- FR-025: format_on_init flag; when false, calls for_each_extent + recover_extent to rebuild dispatch-map with logging
- FR-028: promote_and_serve re-inserts into memory-tier, re-registers as MemoryTier, preserves ssd_offset via convert_to_storage
- FR-029: Background evictor started if ssd_eviction_threshold > 0.0 and drives exist. Thread joined in shutdown()
- FR-030: Evictor computes utilization via sum(used_bytes)/sum(capacity_bytes). Interval from ssd_eviction_interval_secs
- FR-031: Evicts oldest BlockDevice entries via dm.oldest_keys(batch_size), stops at low_watermark
- FR-032: Skips MemoryTier entries (get_evictable_offset returns None). dm.remove failure handled gracefully (continue)
- FR-033: DispatcherConfig has all required fields with correct defaults: ssd_eviction_threshold=0.9, ssd_eviction_low_watermark=0.8, ssd_eviction_batch_size=64, ssd_eviction_interval_secs=5, max_eviction_attempts=2048
- FR-034: register_host_memory called on memory-tier pool during initialize. ClientChannels cached per drive at init
- FR-035: unregister_host_memory called in shutdown before memory-tier teardown
- FR-036: lookup_async returns GpuStream for MemoryTier path; null stream for BlockDevice path. lookup() delegates to lookup_async + stream_synchronize
- FR-037: Warm CUDA stream stored as AtomicU64, single stream (lock-free load). Destroyed in shutdown
- FR-038: clear_memory_tier() loops evict_lru(), transitions to BlockDevice or removes. IMemoryTier::clear() method exists in trait
- FR-039: batch_lookup classifies entries, issues memcpy_h2d_async on warm stream with batched stream_synchronize, groups cold by drive (key % num_drives via splitmix hash), uses ColdReadPool with pipelined_multi_object_zero_copy at depth=128
- FR-040: promote_to_memory_tier for BlockDevice entries: reads via pipelined_ssd_to_dram_only per-drive threads; MemoryTier entries get dm.touch + mt.touch; errors logged not propagated
- FR-041: pipelined_ssd_to_dram_only and pipelined_multi_ssd_to_dram_only exist in pipeline.rs
- FR-042: Eviction event channel (create_eviction_channel, emit_eviction, eviction_dropped_count) — bounded crossbeam channel for external consumers (gRPC TakeEvents)
- FR-043: PipelineMetrics trait provides timing hooks for pipeline stages (cold reads, hot DMA, populate)
- FR-044: ColdReadPool pre-allocates per-drive NVMe channels and CUDA streams for batch cold reads
- FR-045: batch_lookup forwards local cache misses to IRemoteLookup when bound; merges remote results into batch response

#### Success Criteria

| ID | Status | Notes |
|----|--------|-------|
| SC-001 | Aligned | populate + lookup round-trip via memory-tier path |
| SC-002 | Aligned | check returns accurate presence info |
| SC-003 | Aligned | remove frees memory-tier slot + SSD extent |
| SC-004 | Aligned | Concurrent safety via dispatch-map references |
| SC-005 | Aligned | Descriptive error for unbound receptacles |
| SC-006 | Aligned | shutdown drains ParallelBackgroundWriter |
| SC-007 | Aligned | N drives with splitmix64 key-based selection |
| SC-009 | Aligned | Capacity-based LRU eviction transitions to BlockDevice |
| SC-010 | Aligned | touch updates dm + mt without DMA |
| SC-011 | Aligned | promote_and_serve promotes from SSD via pipeline |
| SC-012 | Aligned | Zero-copy sliding-window pipeline with FIFO assumption |
| SC-013 | Aligned | SSD evictor respects threshold and low-watermark |
| SC-014 | Aligned | batch_lookup parallel cold promotion via cold pool |

## Unspecced Code

None — all implementation features are now covered by FR-001 through FR-045.

## Resolutions Applied (2026-07-10)

1. **FR-018 doc fix**: Updated `IDispatcher::remove()` doc comment in `components/interfaces/src/idispatcher.rs` to state non-blocking behavior (matching spec and implementation).

2. **FR-042 added**: Eviction event channel (`create_eviction_channel`) — previously unspecced.

3. **FR-043 added**: PipelineMetrics trait — previously unspecced.

4. **FR-044 added**: ColdReadPool persistent worker pool — previously unspecced.

5. **FR-045 added**: Remote lookup forwarding in batch_lookup — previously unspecced.
