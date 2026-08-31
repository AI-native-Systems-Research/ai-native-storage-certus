# Spec Sync Apply Report — dispatch-map

Generated: 2026-08-31
Mode: interactive
Spec: components/dispatch-map/specs/001-dispatch-map/spec.md
Drift source: components/dispatch-map/.specify/sync/drift-report.{json,md}

## Result: no spec changes applied

The implementation is unchanged since the 2026-08-20 sync (no commits to `components/dispatch-map/` or `components/interfaces/src/idispatch_map.rs`). The spec is correct and current — the previously-unspecced `reuse_count` is already covered by FR-029 + Key Entities. The only two drift items are code-side ALIGN tasks, which this sync does not apply to source. Therefore **no edit to `spec.md` was made and no backup was needed.**

## Specs Updated

None.

## Align Tasks

| Task | Requirement | Status |
|---|---|---|
| Unbound `IEvictionPolicy` must error, not panic | FR-012 | Retained (open) — see `align-tasks.md` |
| Null-pointer guard in `create_memory_tier_entry` | FR-003 / US1-AS3 | Retained (open) — see `align-tasks.md` |

Both require `src/**` (and, for FR-003, `interfaces/**` + contract) edits, which are outside this sync's editable scope. Hand to a `speckit-implement` code pass.

## Unspecced Backfilled

None. (`reuse_count` was backfilled as FR-029 in the 2026-08-20 sync.)

## Resolved Since Last Sync (no action needed)

| Item | Status |
|---|---|
| Unspecced `reuse_count: AtomicU32` field | Now specced as FR-029 + Key Entities (2026-08-20). No longer drift. |

## Not Applied / Deferred (out of scope)

| Item | Reason |
|---|---|
| FR-012 panic→error and FR-003 null-guard code changes | `src/**` / `interfaces/**` edits are outside sync editable scope (`.specify/sync/**`, `specs/**` only). Retained as align-tasks. |
| `components/dispatch-map/CLAUDE.md:35` stale crate path (`../../component-framework/crates/`) | CLAUDE.md is outside sync editable scope. Fix in a normal doc pass (mirror the dispatcher CLAUDE.md correction to `../../lib/component-framework/crates/`). |

## Notes

- No Markdown under `specs/**` was modified this run; only `.specify/sync/**` artifacts were refreshed.
- No `src/**` source was touched and `cargo` was not run.
- Active requirement count: FR-001..FR-029 (FR-019 removed, FR-021 merged) and SC-001..SC-006 — unchanged.
