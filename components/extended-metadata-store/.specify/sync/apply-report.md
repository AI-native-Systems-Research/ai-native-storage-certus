# Sync Apply Report

Applied: 2026-10-19T07:04:24Z (sync note date 2026-10-14)
Component: `extended-metadata-store`. Branch: `fix/bundle-dispatcher-ems`. Mode: batch (no questions).
Source: `.specify/sync/proposals.{json,md}` (generated 2026-10-18, based on drift-report 2026-10-14 @ `4f8b5554`, component source `4a8fe3e7`).
Approved by: Cornel Constantinescu, 2026-10-18 (PR approval page, groups A–D).
Backups: `.specify/sync/backups/20261019T070424Z/` (`spec.md`, `tasks.md`, `align-tasks.md`, `drift-report.md`, and the previous `apply-report.{md,json}`).

This report replaces the 2026-10-14 apply report of branch `fix/extended-metadata-store-flush` (its copy is in the backup folder). The proposal IDs P1–P15 are reused with new meanings.

## Changes Made

All 15 proposals are `approved: true`. Paths: `spec.md` and `tasks.md` are under `specs/001-extended-metadata-store/`.

### Specs Updated

| Proposal | File | Requirement | Change Type |
|----------|------|-------------|-------------|
| P1 | spec.md | FR-05 status cell: Open clause replaced by the error-propagation evidence and a pointer to the FR-13 caveat | Modified (BACKFILL) |
| P2 | spec.md | US6 test list: `force_flush_reports_failed_disk_write`, `force_flush_ok_on_healthy_device` added | Added (BACKFILL) |
| P2 | spec.md | US6 note: "listed below … follow-up test" replaced; SSD test points to ALIGN-EMS-001 | Modified (BACKFILL) |
| P3 | spec.md | Known Gaps, FR-05 entry: line refs `lib.rs:221-235`/`:72`/`:131`; failure propagation since `4a8fe3e7` | Modified (BACKFILL) |
| P4 | spec.md | FR-12: result of a released call (`Ok` only if a serving pass succeeded); line refs | Modified (BACKFILL) |
| P5 | spec.md | FR-11 line refs (`:193-214`, `:243`); FR-13 line refs (`:223-227`, `:276-290`) and "see P6" clause (kept, because P6 was approved) | Modified (BACKFILL) |
| P12 | tasks.md | line 19: ALIGN-EMS-003 follow-up marked RESOLVED by `4a8fe3e7` | Modified (BACKFILL) |
| P12 | align-tasks.md | ALIGN-EMS-003 status `open` → resolved; Status paragraph added after "Current Code"; ALIGN-EMS-002 dependency note rewritten | Modified (BACKFILL) |
| P14 | spec.md | Known Gaps, `CapacityExhausted`: with a trigger attached, the capacity error reaches `force_flush()` as `StorageError` | Modified (BACKFILL) |
| P15 | spec.md | Header history line (Previous Sync): appended "(now ALIGN-EMS-001 and ALIGN-EMS-002; the 002 spec is not tracked in this tree)" | Modified (SUPERSEDE) |
| — | spec.md | Header metadata: new **Last Synced** line for this apply; the old one kept as **Previous Sync (2)** | Metadata |

SUPERSEDE note (P15): the 002 spec is not tracked in this tree, so there was no old spec metadata that could take a `superseded_by` field. The cross-reference is the appended mapping in `spec.md` itself, as the proposal specifies.

### New Specs Created
- None.

### Implementation Tasks Generated
In `.specify/sync/align-tasks.md`, every one of these was approved "as an ALIGN task only: record in align-tasks.md, no code change in this PR":

| Proposal | Task | Requirement | Severity | Action |
|----------|------|-------------|----------|--------|
| P6 | ALIGN-EMS-008 | FR-13 / FR-05 wiring: reference cycle prevents the final flush | moderate | new task |
| P7 | ALIGN-EMS-009 | FR-05: ALIGN-EMS-003 AC2 (reboot) and restated AC3 tests | minor | new task |
| P13 | ALIGN-EMS-010 | FR-12 / US6 AC5 / FR-14: doc comments at `flush.rs:79`, `:156`, `lib.rs:75` | minor | new task |
| P8 | ALIGN-EMS-004 | FR-09 | moderate | re-confirmed, unchanged |
| P9 | ALIGN-EMS-005 | NFR-04 | moderate | re-confirmed, unchanged |
| P10 | ALIGN-EMS-006 | FR-01 (error-variant decision still open) | moderate | re-confirmed, unchanged |
| P11 | ALIGN-EMS-007 | SC-7 | minor | re-confirmed, unchanged |

### Not Applied
| Proposal | Reason |
|----------|--------|
| — | None. Every proposal was approved and handled. |

### Open items from unapproved proposals with an approval note
- None. No proposal is unapproved.

## Still open after this apply
- Code-side: ALIGN-EMS-001 (open), ALIGN-EMS-002 (parked), ALIGN-EMS-004..007, and the new ALIGN-EMS-008..010.
- ALIGN-EMS-006: the error-variant decision is still pending.
- FR-13 status cell: the "see P6" clause stays until ALIGN-EMS-008 lands and FR-05/FR-13 are backfilled.

## Verification
- No source, test or interface file was changed. `cargo` was not run.
- Each `old` text was matched exactly once before it was replaced.

## Next Steps
1. Review the updated `spec.md`, `tasks.md` and `align-tasks.md`.
2. Commit: `git add components/extended-metadata-store/specs components/extended-metadata-store/.specify/sync && git commit -m "sync(extended-metadata-store): apply bundle drift resolutions"`.
3. Implement ALIGN-EMS-008 (most important: graceful-shutdown durability), 009 and 010.
