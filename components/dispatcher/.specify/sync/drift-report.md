# Spec Drift Report

Generated: 2026-07-02  
Project: dispatcher (components/dispatcher)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 41 |
| Aligned | 33 (80%) |
| Drifted | 5 (12%) |
| Not Implemented | 0 (0%) |
| Removed (spec references dead code) | 3 (7%) |

## Detailed Findings

### Spec: 001-dispatcher-cache-interface - Dispatcher Cache Interface (Memory-Tier Architecture)

#### Aligned (no drift)

- FR-002: DispatcherError type defined in interfaces crate
- FR-003: populate allocates memory-tier slot, DMA from GPU, registers MemoryTier entry, enqueues write-through
- FR-004: Background writer reads via peek(), writes to SSD, calls convert_to_storage
- FR-005: Memory-tier entry remains accessible after write-through (not freed)
- FR-007: lookup returns KeyNotFound for missing keys
- FR-008: check returns presence without DMA
- FR-009: remove frees memory-tier slot, extent, and dispatch-map entry
- FR-010: Uses define_component! macro, exposes IDispatcher
- FR-011: Receptacles for ILogger, IDispatchMap, IGpuServices, ISPDKEnv, IMemoryTier, IRemoteLookup; poller_base_cpu supported
- FR-012: initialize validates dispatch_map and memory_tier bindings
- FR-013: Thread-safe via dispatch-map locking
- FR-014: shutdown drains background writer, checkpoints extent managers
- FR-015: N block devices with N extent managers, co-located metadata
- FR-016: Format params passed to extent managers
- FR-017: Write-through failure silently drops job
- FR-018: remove does not block on write-through
- FR-019: MDTS-aware segmented I/O, zero-copy pipelines with max_queue_depth
- FR-023: touch refreshes dispatch-map timestamp and memory-tier LRU
- FR-024: Capacity-based eviction with sparse-probe + shard-targeted LRU
- FR-025: format_on_init flag, recovery via for_each_extent
- FR-028: Promotion re-inserts into memory-tier
- FR-029: Background SSD evictor started if threshold > 0.0
- FR-030: Periodic SSD utilization check
- FR-031: Evicts oldest BlockDevice entries until below low-water mark
- FR-032: Skips MemoryTier entries and entries with active references
- FR-033: DispatcherConfig includes all SSD eviction fields
- FR-034: CUDA-pin and SPDK-register memory-tier pool
- FR-035: unregister_host_memory on shutdown
- FR-036: lookup_async returns GpuStream
- FR-037: Pre-allocated warm CUDA stream
- FR-038: clear_memory_tier evicts all entries
- FR-039: batch_lookup with parallel cold promotion
- FR-040: promote_to_memory_tier best-effort fire-and-forget
- FR-041: pipelined_ssd_to_dram_only functions exist

#### Drifted (spec says one thing, code does another)

- **FR-001**: Spec says IDispatcher MUST provide `prepare_store`, `commit_store`, `cancel_store` methods. Code has removed these methods.
  - Location: components/interfaces/src/idispatcher.rs
  - Severity: **major** (intentional removal — spec needs update)

- **FR-006**: Spec says "if Staging (legacy), DMA from staging buffer to GPU". Code no longer has a Staging path — LookupResult::Staging variant removed.
  - Location: components/interfaces/src/idispatch_map.rs
  - Severity: **major** (intentional removal — spec needs update)

- **FR-020**: Spec says `prepare_store(key, size)` MUST reserve extent and return DMA buffer. Method no longer exists.
  - Location: components/dispatcher/src/lib.rs
  - Severity: **major** (intentional removal — spec needs update)

- **FR-021**: Spec says `commit_store(key)` MUST write buffer to SSD. Method no longer exists.
  - Location: components/dispatcher/src/lib.rs
  - Severity: **major** (intentional removal — spec needs update)

- **FR-022**: Spec says `cancel_store(key)` MUST abort pending write. Method no longer exists.
  - Location: components/dispatcher/src/lib.rs
  - Severity: **major** (intentional removal — spec needs update)

#### Spec Sections Referencing Removed Code

- **User Story 6** (Direct Store Workflow): Entire story describes prepare/commit/cancel_store — now removed.
- **SC-008**: "prepare_store/commit_store workflow successfully persists data" — no longer applicable.
- **Key Entity "PendingWrite"**: Struct removed from implementation.
- **Key Entity "Dispatch Map Entry"**: Mentions "Staging (legacy DMA buffer)" — variant removed.
- **User Story 2**: Mentions "Staging (legacy)" path in lookup — removed.
- **FR-026, FR-027**: Already marked REMOVED in spec.

## Recommendations

1. **Remove FR-020, FR-021, FR-022** — These describe the removed prepare_store/commit_store/cancel_store methods.
2. **Remove User Story 6** entirely — the direct store workflow is no longer implemented.
3. **Update FR-001** — Remove `prepare_store`, `commit_store`, `cancel_store` from the method list.
4. **Update FR-006** — Remove the "if Staging (legacy)" clause. Lookup now only handles MemoryTier and BlockDevice.
5. **Remove SC-008** — No longer testable.
6. **Update Key Entities** — Remove "PendingWrite" and remove "Staging (legacy DMA buffer)" from the Dispatch Map Entry description.
7. **Update User Story 2** — Remove the "Staging (legacy)" bullet point from the lookup description.
