# Spec-Sync Phase B — Apply Report: disk-partition-manager

**Generated**: 2026-08-20 (Phase B)
**Spec**: `001-gpt-partition-management`
**Policy**: `.specify/sync/PHASE_B_POLICY.md`
**Based on drift report**: `drift-report.json` (2026-08-07T15:30:30Z)
**Supersedes**: 2026-08-07 sync run (branch `sync/spec-drift-sweep-20260807`)

## Counts

| Category | Count |
|----------|-------|
| BACKFILL applied | 2 (FR-003, PR-002) |
| BACKFILL-UNSPECCED | 2 |
| ALIGN tasks | 0 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |
| New specs | 0 |

## Specs Updated

| Requirement | Change type | Summary |
|-------------|-------------|---------|
| FR-003 | BACKFILL | Rewrote the note from "fix drafted on branch / see align-tasks.md" to "implemented in `read_gpt` (`src/gpt.rs:66-96`) — backup attempted on both `CorruptTable` and `NoPartitionTable`". Added US2 acceptance scenario 4 (signature-damaged primary + valid backup recovers, no reformat). |
| PR-002 | BACKFILL (no-op) | Current spec text already describes the actual per-sector read (~33 round-trips, O(1) in device size); no edit needed this run. |
| Metadata (Last Synced) | metadata | Updated to 2026-08-20; recorded FR-003 re-classification ALIGN → BACKFILL. |

## Align Tasks Generated

None. FR-003's 2026-08-07 drafted code fix is now present in `src/gpt.rs`, so it is
resolved by BACKFILL, not ALIGN. See `align-tasks.md` for the historical (superseded)
note and the residual test-coverage gap.

## Unspecced Backfilled

| Item | Change type | Status |
|------|-------------|--------|
| Hardcoded primary entry LBA on read (`src/gpt.rs:68`) | BACKFILL-UNSPECCED (Implementation Notes) | already reflected in `spec.md` |
| `generate_guid` zero-fallback on `/dev/urandom` failure (`src/gpt.rs:564-574`) | BACKFILL-UNSPECCED (Implementation Notes) | already reflected in `spec.md` |

## Resolved

None.

## Backups

| Edited spec | Backup |
|-------------|--------|
| `.specify/specs/001-gpt-partition-management/spec.md` | `.specify/sync/backups/.specify/specs/001-gpt-partition-management/spec.md.bak` |

## Notes

- No `.rs` source was modified; `cargo` was not run.
- **Known coverage gap** (not an ALIGN item): SC-001/SC-002/SC-003 and the FR-003
  signature-recovery scenario have no automated tests; `[dev-dependencies]` is empty
  (see `drift-report.json` `conflicts` and `tasks.md`). Track as a normal test-authoring
  task.
