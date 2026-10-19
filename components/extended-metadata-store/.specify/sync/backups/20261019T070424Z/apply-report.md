# Sync Apply Report

Applied: 2026-10-14T18:11:38Z (branch `fix/extended-metadata-store-flush`, batch mode)
Component: `extended-metadata-store`
Source: `.specify/sync/proposals.{json,md}` (generated 2026-10-14, based on the drift-report at `2970e0a8`)
Approved by: Cornel Constantinescu, 2026-10-14 (all 14 proposals `approved: true`)
Backups: `.specify/sync/backups/20261014T181138Z/` (`spec.md`, `plan.md`, `tasks.md`, `align-tasks.md`, `drift-report.md` as they were before this apply)

This replaces the 2026-08-20 Phase-B apply report.

No source code, interface crate, or file outside `specs/` and `.specify/sync/` was modified. No cargo command was run.

## Changes Made

Paths are under `components/extended-metadata-store/specs/001-extended-metadata-store/`. Every edit used the exact old/new text from `proposals.md`; each old text was found verbatim exactly once.

### Specs Updated

| Proposal | File | Requirement / location | Change Type |
|----------|------|------------------------|-------------|
| P7 | spec.md | FR-12 row | Modified (BACKFILL) |
| P7 | spec.md | US6 AC2 (concurrent `force_flush()` coalescing) | Modified (BACKFILL) |
| P7 | spec.md | US6 AC5 (no dirty entries → no disk I/O) | Modified (BACKFILL) |
| P8 | spec.md | FR-11 row (timer + threshold trigger details) | Modified (BACKFILL) |
| P9 | spec.md | FR-14 row (snapshot-scoped dirty count) | Modified (BACKFILL) |
| P9 | spec.md | FR-17 row (`snapshot_entries`/`mark_flushed` semantics, single-flusher precondition) | Modified (BACKFILL) |
| P10 | spec.md | FR-13 row (unconditional final flush on Drop) | Modified (BACKFILL) |
| P10 | spec.md | Key Entities → `FlushManager` row (`completed_seq()`, ticket coalescing) | Modified (BACKFILL) |
| P11 | spec.md | FR-05 row (evidence refresh, trigger read-lock behaviour, **Open** P1 clause) | Modified (BACKFILL) |
| P12 | spec.md | US6 Tests: `flush_manager_dirty_threshold_triggers` caveat | Modified (BACKFILL) |
| P12 | spec.md | US6 Tests: 5 repair tests (`repair_test_flush_threshold.rs`, `repair_test_flush_lost_dirty.rs`) | Added (BACKFILL) |
| P13 | plan.md | :25 component fields | Modified (BACKFILL) |
| P13 | plan.md | :32 flush coalescing | Modified (BACKFILL) |
| P13 | plan.md | :96 `snapshot_entries` data-flow step | Modified (BACKFILL) |
| P13 | plan.md | :101 `mark_flushed` data-flow step | Modified (BACKFILL) |
| P13 | plan.md | :104 `force_flush` wiring note | Modified (BACKFILL) |
| P13 | plan.md | :162 workspace-membership note | Modified (BACKFILL) |
| P13 | tasks.md | :13 T056 (no longer blocked) | Modified (BACKFILL) |
| P13 | tasks.md | :18 ALIGN-001 → RESOLVED, checked | Modified (BACKFILL) |
| P13 | tasks.md | :19 ALIGN-002 → RESOLVED, checked, P1 follow-up | Modified (BACKFILL) |
| P14 | spec.md | US6 note: "002-FR-007's ALIGN task" → ALIGN-EMS-001 / proposal P1 | Modified (SUPERSEDE) |
| P14 | spec.md | Known Gaps `CapacityExhausted`: `src/lib.rs:179`, `src/flush.rs:36-42`, `test_capacity_exhaustion` (`tests/integration_ssd.rs:526-547`, arm `:542`), "(Task ALIGN-EMS-002)" | Modified (SUPERSEDE) |
| P14 | spec.md | Known Gaps "Retained public API surface": `src/on_disk.rs:147`, `src/test_support.rs:275` | Modified (SUPERSEDE) |
| (step 2.5) | spec.md | Header: new `**Last Synced**: 2026-10-14` line; the 2026-09-03 line is kept as `**Previous Sync**` | Metadata |

**SUPERSEDE handling (P14):** the superseded spec, `002-ssd-integration-test`, is not in the tree. It exists only as backups under `.specify/sync/backups/`, so there is no live spec metadata to add a `superseded_by` field to, and backups were not edited. The supersession is recorded as cross-references in spec.md instead: the 002 requirement IDs are replaced by the ALIGN task IDs (ALIGN-EMS-001, ALIGN-EMS-002) that now carry them. The 002 spec was not recreated, as the proposal specifies.

### New Specs Created

None.

### Implementation Tasks Generated

`.specify/sync/align-tasks.md` (approval note on P1–P6: "approved as an ALIGN task only: record in align-tasks.md, no code change in this PR"):

| Task | Proposal | Requirement | Severity | Action |
|------|----------|-------------|----------|--------|
| ALIGN-EMS-003 | P1 | FR-05 / US6 AC1 — `trigger_flush` reports a failed flush as `Ok` | major | New task |
| ALIGN-EMS-004 | P2 | FR-09 — corrupt entries do not cause fallback | moderate | New task |
| ALIGN-EMS-005 | P3 | NFR-04 — after a fallback, the next flush overwrites the only good region | moderate | New task |
| ALIGN-EMS-006 | P4 | FR-01 — keys > 65 535 bytes accepted but not persistable (open sub-decision: error variant) | moderate | New task |
| ALIGN-EMS-007 | P5 | SC-7 — public items without doc comments | minor | New task |
| ALIGN-EMS-002 | P6 | `CapacityExhausted` never constructed | moderate | Existing task refreshed: line refs updated; dependency on ALIGN-EMS-003 recorded; stays parked |

A status table was also added at the top of `align-tasks.md`.

### Not Applied

| Proposal | Reason |
|----------|--------|
| — | None. All 14 proposals were approved. P1–P6 were approved as ALIGN tasks only, so no code was changed for them. |

### Open items from unapproved proposals with an `approval_note`

None. Every proposal has `approved: true`.

## Still open after this apply

- FR-05 carries an explicit **Open** clause (P11) until ALIGN-EMS-003 (P1) is implemented. Drop the clause when that lands.
- ALIGN-EMS-001 (interface-only durability in `tests/integration_ssd.rs`) and ALIGN-EMS-002 (parked) remain open.
- ALIGN-EMS-006 needs a maintainer decision on the error variant before it can be implemented.

## Next Steps

1. Review the updated `spec.md`, `plan.md` and `tasks.md`.
2. Commit: `git add components/extended-metadata-store/specs components/extended-metadata-store/.specify/sync && git commit -m "docs(spec-sync): apply approved extended-metadata-store proposals"`.
3. Re-stamp the `spec_sync_*` frontmatter of `drift-report.md` with `scripts/spec-sync-hash.sh`. This apply did not touch it.
4. Implement ALIGN-EMS-003..007 in a follow-up PR. Start with ALIGN-EMS-003, because ALIGN-EMS-002's alternative fix depends on it.
