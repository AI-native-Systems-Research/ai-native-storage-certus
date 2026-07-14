# Sync Proposals: Dispatcher Component

**Generated**: 2026-07-14
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`
**Status**: 2 proposals applied

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 2 |
| Align (Spec -> Code) | 0 |
| Human Decision | 0 |

## Proposals

### Proposal 1: FR-033 — Add memory-tier eviction config fields

**Direction**: BACKFILL
**Status**: APPLIED

**Current State**:
- Spec says: "DispatcherConfig MUST include `ssd_eviction_threshold`, `ssd_eviction_low_watermark`, `ssd_eviction_batch_size`, `ssd_eviction_interval_secs`, and `max_eviction_attempts`"
- Code does: additionally includes `memory_tier_eviction_threshold` (f64, default 0.0), `memory_tier_eviction_low_watermark` (f64, default 0.70), `memory_tier_eviction_batch_size` (usize, default 64), `memory_tier_eviction_interval_secs` (u64, default 2)

**Proposed Resolution**: Update FR-033 to include the four new memory-tier eviction fields.

**Confidence**: HIGH

---

### Proposal 2: New User Story 12 + FR-046 through FR-050 — Background Memory-Tier Demotion

**Direction**: BACKFILL (NEW_SPEC)
**Status**: APPLIED

**Feature**: MemoryTierEvictor — a background thread that proactively demotes LRU entries from DRAM memory-tier to SSD when utilization exceeds a configurable threshold.

**Proposed Spec Additions**:

- **User Story 12**: Background Memory-Tier Demotion (Priority: P3)
- **FR-046**: The dispatcher MUST start a background memory-tier evictor thread during `initialize()` if `memory_tier_eviction_threshold > 0.0`. The evictor MUST be shut down (thread joined) during `shutdown()`.
- **FR-047**: The memory-tier evictor MUST periodically check memory-tier utilization (`IMemoryTier::used()` / `IMemoryTier::capacity()`). The check interval MUST be configurable via `memory_tier_eviction_interval_secs` (default: 2 seconds).
- **FR-048**: When memory-tier utilization exceeds `memory_tier_eviction_threshold` (default: disabled at 0.0), the evictor MUST demote entries using `IMemoryTier::oldest_keys(batch_size)` for LRU ordering, stopping when utilization drops below `memory_tier_eviction_low_watermark` (default: 0.70) or the batch is exhausted.
- **FR-049**: For each candidate, the evictor MUST check `IDispatchMap::is_evictable(key)` (write-through complete, no active references) before demoting. Demotion path: `IMemoryTier::remove(key)` followed by `IDispatchMap::convert_memory_tier_to_block(key)`. If BlockDevice transition fails, the dispatch-map entry is removed entirely.
- **FR-050**: The evictor MUST emit `EvictionEvent { key, reason: Demoted }` (or `Removed` on transition failure) via the eviction notification channel (FR-042) for each demoted entry.

**Confidence**: HIGH

---

## Previous Proposals (Applied)

### Proposal (2026-05-28): FR-001 — Add `batch_lookup`
**Status**: APPLIED

### Proposal (2026-05-28): FR-019 — Parameterized pipeline queue depth
**Status**: APPLIED

### Proposal (2026-05-28): New User Story 11 — Parallel Batch Cold Promotion
**Status**: APPLIED
