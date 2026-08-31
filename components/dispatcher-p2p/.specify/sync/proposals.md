# Spec Sync Proposals — dispatcher-p2p

Generated: 2026-08-31
Mode: interactive
Drift source: `.specify/sync/drift-report.{md,json}`

## Summary

| Direction | Count |
|---|---|
| BACKFILL (spec → matches shipped code) | 1 (FR-008 amend) |
| BACKFILL-UNSPECCED (new FR/SC) | 0 |
| ALIGN (code → matches correct spec) | 1 (FR-017, retained; no spec edit) |
| HUMAN_DECISION | 1 (`cold_staging_*`, carried; no spec edit) |
| REMOVE-FROM-SPEC | 0 |

## Approved & applied

### BACKFILL-FR008 — document the `tier_event_stats()` interface-parity stub ✅ approved, applied
- **Requirement**: FR-008 (drop-in `IDispatcher` parity)
- **Direction**: BACKFILL (code authoritative — the trait method shipped in commit `4659626b`)
- **Location**: `src/lib.rs:2665-2668` (impl); `../interfaces/src/idispatcher.rs:564` (trait method)
- **Before**: FR-008 ended "...performs the DRAM→GPU delivery itself using the memory-tier→device copy."
- **After**: appended — "Interface parity also covers the `IDispatcher` telemetry methods: `tier_event_stats()` (added to the trait in commit `4659626b` for the KV profiler) is satisfied here by a zeroed stub returning `TierEventStats::default()` — dispatcher-p2p performs no tier-movement tracking, so the actual counters live in the standard dispatcher (see dispatcher FR-058); `read_write_stats()` is likewise satisfied by aggregating per-drive block-device counters."
- **Rationale**: dispatcher-p2p must expose the full `IDispatcher` surface (FR-008); the new `tier_event_stats()` method is a deliberate no-op because the component does no tiering. Documenting the stub keeps FR-008 honest about the current interface without inventing counter behavior.
- **User decision**: **Approve** (full note, including the `read_write_stats()` clause).

## Retained (no spec edit — out of sync editable scope)

### ALIGN-FR017 — eviction drop-count never incremented (code defect)
- Spec correct; code violates it. `emit_eviction` is `#[allow(dead_code)]`; live publish sites bypass the counter. Fix requires `src/**` edits. Retained in `align-tasks.md`.

### HUMAN_DECISION — `cold_staging_slots` / `cold_staging_buf_bytes`
- Still dead config on the dispatcher-p2p surface (`interfaces/src/idispatcher.rs:84,87`); no `src/` references. Resolution (wire in or remove) requires `interfaces/**` + `src/**` changes. Carried, unresolved.

## Out-of-scope observations (informational)

- `memcpy_batch_async` in `#[cfg(test)]` MockGpuServices (`src/lib.rs:3424+`) — test mock keep-up with the `IGpuServices` trait (commit `495c5acc`); no requirement needed.
- `plan.md` source-layout / bench-name staleness (minor, carried).
