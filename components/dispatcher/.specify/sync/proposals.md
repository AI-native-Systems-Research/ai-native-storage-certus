# Sync Proposals: Dispatcher Component

**Generated**: 2026-07-15
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`
**Status**: 4 proposals approved

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 4 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: FR-047 — Evictor paced drain loop

**Direction**: BACKFILL
**Status**: ✅ Approved

**Current State**:
- Spec says: "The check interval MUST be configurable via memory_tier_eviction_interval_secs (default: 2 seconds)."
- Code does: Uses config.interval as idle sleep below threshold; above threshold uses 200ms/500ms paced loop.

**Proposed Resolution**:
> FR-047: The memory-tier evictor MUST use `memory_tier_eviction_interval_secs` (default: 2 seconds) as the idle polling interval when utilization is below threshold. When above threshold, the evictor MUST loop continuously with a 200ms pace between successful demotion batches and a 500ms backoff when no entries are evictable (to allow write-through to release references).

**Rationale**: The paced drain loop converges to the low watermark in seconds instead of minutes. The config field remains meaningful for idle polling.
**Confidence**: HIGH

---

### Proposal 2: FR-048 — Exponential batch scaling

**Direction**: BACKFILL
**Status**: ✅ Approved

**Current State**:
- Spec says: "the evictor MUST demote entries using IMemoryTier::oldest_keys(batch_size)"
- Code does: Quadratic pressure curve (1×–8×) and adaptive scan widening (up to 4× on dry runs).

**Proposed Resolution**:
> FR-048: When memory-tier utilization exceeds `memory_tier_eviction_threshold`, the evictor MUST scale the effective batch size using a quadratic pressure curve: `multiplier = 1.0 + 7.0 × pressure²` where `pressure = (utilization - threshold) / (1.0 - threshold)`, giving 1× to 8× the configured `batch_size`. On consecutive dry runs (no entries evictable), the scan window MUST widen up to 4× the effective batch to find evictable entries deeper in the LRU list. Demotion stops when utilization drops below `memory_tier_eviction_low_watermark` or the scan is exhausted.

**Rationale**: Exponential scaling prevents stalls at high utilization; wider scans find evictable entries past write-through holdbacks.
**Confidence**: HIGH

---

### Proposal 3: FR-049 — Race-safe demotion ordering

**Direction**: BACKFILL
**Status**: ✅ Approved

**Current State**:
- Spec says: "Demotion path: IMemoryTier::remove(key) followed by IDispatchMap::convert_memory_tier_to_block(key)"
- Code does: Reverse order — try_evict_to_block first, then mt.remove.

**Proposed Resolution**:
> FR-049: For each candidate, the evictor MUST call `IDispatchMap::try_evict_to_block(key)` which atomically verifies evictability (write-through complete, no active references) and transitions the entry to BlockDevice state under a single lock hold. Only after the dispatch-map reflects BlockDevice state MUST the DRAM slot be freed via `IMemoryTier::remove(key)`. This ordering prevents a race where concurrent lookups obtain a freed memory-tier pointer.

**Rationale**: Fixes SPDK vtophys crash caused by TOCTOU race in the original ordering.
**Confidence**: HIGH

---

### Proposal 4: FR-049 — Skip on failure (no data loss)

**Direction**: BACKFILL
**Status**: ✅ Approved

**Current State**:
- Spec says: "If BlockDevice transition fails, the dispatch-map entry is removed entirely."
- Code does: Entry is skipped (continue) — no removal on failure.

**Proposed Resolution**:
> Add to FR-049: "If `try_evict_to_block` fails (active references or write-through not complete), the entry MUST be skipped. The evictor does NOT remove the dispatch-map entry on transition failure — the entry remains in MemoryTier state and will be re-evaluated on subsequent sweeps."

**Rationale**: Removing entries with active references causes data loss; skipping is strictly safer.
**Confidence**: HIGH
