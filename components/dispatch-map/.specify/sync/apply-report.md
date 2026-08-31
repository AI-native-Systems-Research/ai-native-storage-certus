# Sync Apply Report — Phase B

**Date**: 2026-08-20
Based on: `components/dispatch-map/.specify/sync/drift-report.json`
Policy: `.specify/sync/PHASE_B_POLICY.md`
Backups: `.specify/sync/backups/specs/001-dispatch-map/spec.md.bak`

## Specs Updated

| Spec | Requirement | Change Type |
|------|-------------|-------------|
| 001-dispatch-map | FR-029 | Added (BACKFILL-UNSPECCED) — per-entry `reuse_count` (`AtomicU32`) read-hit counter described as built: init 0 at creation; +1 (relaxed) on `lookup`/`take_read`/`downgrade_reference`; internal-only (Debug), no `IDispatchMap` accessor. |
| 001-dispatch-map | Key Entities → Dispatch Entry | Modified — added the `reuse_count` (`AtomicU32`) field to the entry layout. |
| 001-dispatch-map | User Story 2 / AS6 | Added — `lookup` success increments `reuse_count`. |
| 001-dispatch-map | User Story 4 / AS9 | Added — `take_read`/`downgrade_reference` increment `reuse_count`; other ref ops do not. |
| 001-dispatch-map | Header metadata | Modified — updated **Last Synced** to 2026-08-20 (Phase B). |

## Align Tasks Generated

Written to `.specify/sync/align-tasks.md` (2 tasks — code changes NOT applied here):

| Requirement | Location | Summary |
|-------------|----------|---------|
| FR-012 | `src/lib.rs:55` (`:68`, `:392`, `:573`) | `initialize()` must return `Err(NotInitialized)` instead of panicking (`eviction_policy.get().unwrap()`) when `IEvictionPolicy` is unbound. |
| FR-003 · US1-AS3 | `src/lib.rs:381` | Add a null-pointer guard to `create_memory_tier_entry` (new `DispatchMapError::NullPointer` variant); reject nulls before recording an entry. |

## Unspecced Backfilled

| Feature | Location | Resolution |
|---------|----------|------------|
| `reuse_count: AtomicU32` per-entry read-hit counter | `src/entry.rs:37`; `src/lib.rs:142-143, 220-222, 313-315` | Backfilled as FR-029 + Key Entities update + US2/AS6 & US4/AS9 acceptance scenarios. |

## Resolved (already fixed on main thread)

None for this component.

## Human Decision

None — both drift items and the unspecced feature were resolvable per policy after reading the referenced source.

## Counts

| Category | Count |
|----------|-------|
| BACKFILL applied (drifted requirements) | 0 |
| BACKFILL-UNSPECCED applied | 1 |
| ALIGN tasks | 2 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |
| spec.md files edited (all backed up) | 1 |

## Next Steps

1. Review the updated spec: `components/dispatch-map/specs/001-dispatch-map/spec.md` (FR-029 + Key Entities + US2/AS6, US4/AS9).
2. Implement the 2 code-side ALIGN tasks in `.specify/sync/align-tasks.md` (e.g. via `/speckit-implement`), then re-run `/speckit-sync-analyze` to confirm the drift closes.
3. Commit on a feature branch (never directly to `unstable`).
