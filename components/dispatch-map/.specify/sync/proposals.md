# Spec Sync Proposals — dispatch-map

Generated: 2026-08-31
Mode: interactive
Drift source: `.specify/sync/drift-report.{md,json}`

## Summary

| Direction | Count |
|---|---|
| BACKFILL (spec → matches shipped code) | 0 |
| BACKFILL-UNSPECCED (add FR/SC for shipped code) | 0 |
| ALIGN (code → matches correct spec) | 2 (both retained from prior cycle) |
| HUMAN_DECISION | 0 |
| REMOVE-FROM-SPEC | 0 |

**No spec-editing proposals this cycle.** The implementation is unchanged since 2026-08-20 (no commits), the spec is correct and current, and the only two drift items are code-side ALIGN tasks — the sync-apply workflow does **not** edit source, so these are retained as follow-up tasks in `align-tasks.md` rather than applied here. There is nothing requiring interactive approval against `spec.md`.

## ALIGN proposals (code changes — retained, not applied by sync)

### ALIGN-FR012 — `initialize` must error, not panic, on unbound `IEvictionPolicy`
- **Requirement**: FR-012 (correct as written; code violates it)
- **Change target**: `components/dispatch-map/src/lib.rs` (`get_pool_id` / `initialize` ordering)
- **Status**: retained in `align-tasks.md`; requires a `speckit-implement` code pass
- **Spec edit**: none

### ALIGN-FR003 — null-pointer guard in `create_memory_tier_entry`
- **Requirement**: FR-003 / US1-AS3 (correct as written; code violates it)
- **Change target**: `components/interfaces/src/idispatch_map.rs` (new `NullPointer` variant + `Display`), `components/dispatch-map/src/lib.rs` (guard), contract error list
- **Status**: retained in `align-tasks.md`; requires a `speckit-implement` code pass
- **Spec edit**: none (contract error-list touch happens in the code pass alongside the enum variant)

## Out-of-scope observations (informational)

- `components/dispatch-map/CLAUDE.md:35` — stale crate path `../../component-framework/crates/`; CLAUDE.md is outside sync editable scope.
