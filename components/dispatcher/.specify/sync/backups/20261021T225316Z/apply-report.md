# Sync Apply Report — dispatcher

Applied: 2026-10-20T22:58:11Z (sync date 2026-10-14) · Branch: `fix/dispatcher-remove-clear-mt` (HEAD `cccfb328e`, base `origin/unstable` `af221dd82`) · Mode: batch
Approved by: Cornel Constantinescu, 2026-10-20 ("let's finish the PR")
Backup: `.specify/sync/backups/20261020T225811Z/specs/001-dispatcher-cache-interface/{spec.md,contracts/errors.md,contracts/idispatcher.md}`

## Changes Made

### Specs Updated

All BACKFILL edits use the exact old→new text from `proposals.md` / `proposals.json`. Before each replacement, the `old` string was confirmed to occur exactly once in its file.

| Proposal | Spec | Requirement | Change Type |
|---|---|---|---|
| B-1 | 001 `spec.md` | FR-018 (`remove` takes over the pending write-through's pin; slot/extent ownership afterwards) | Modified |
| B-2 | 001 `spec.md` | Edge case: remove (what refuses `remove`; the job frees the slot when mid-copy) | Modified |
| B-3 | 001 `spec.md` | FR-038 `clear_memory_tier` (`oldest_keys` scan, pinned entries keep their slot, exact count, 5 s `Timeout`) | Modified |
| B-4 | 001 `spec.md` | Assumptions: background writer read reference (`schedule_write_through` gap stated) | Modified |
| B-5 | 001 `contracts/errors.md` | `Timeout` row (`pin`, `clear_memory_tier`) and `KeyNotFound` row (`remove` refusals) | Modified (2 edits) |
| B-6 | 001 `contracts/idispatcher.md` | `remove` doc comment (no write reference) and `remove()` postcondition | Modified (2 edits) |
| — | 001 `spec.md` | `**Last Synced**: 2026-10-14` header for branch `fix/dispatcher-remove-clear-mt` | Added |

Spec 002 was not touched (no proposal).

### New Specs Created
- None.

### Supersede
- None proposed.

### Implementation Tasks Generated
- 4 ALIGN tasks appended to `.specify/sync/align-tasks.md`. All were approved as tasks only, so no code was changed:
  - **A-1** (FR-013 + Assumption: a `schedule_write_through` job must own the pin it settles, major)
  - **A-2** (SC-003 vs FR-009: `remove` must free the extent of a persisted resident entry, moderate)
  - **A-3** (FR-056 vs FR-038: `clear_memory_tier` must not free a reserved-but-not-completed slot, moderate). The open decision is recorded with the approval note's recommendation (a): skip an uncompleted reservation like a pinned entry.
  - **A-4** (FR-018 residual handshake windows, minor, LOW confidence)
- Closed in `align-tasks.md` (the proposals' `close_after_apply`):
  - **A-03**: code fixed by `cccfb328e`, spec by B-3.
  - **N-04**: specified by B-2, B-5 and B-6.

### Not Applied
| Proposal | Reason |
|---|---|
| — | None. All 10 proposals of this run were approved. No proposal is unapproved with an `approval_note`, so no new open item was added. |
| B-3 companion edit (`components/interfaces/src/idispatcher.rs:802-806` doc comment) | Outside `specs/**`. Not part of the BACKFILL, so not edited. The doc comment still says "Entries still being written are removed entirely". |

Still open from the `fix/bundle-dispatcher-ems` run, unchanged: A-01, A-02, A-04, A-05, A-06, A-08 (tasks), and A-07 and N-01 (decisions).

No source files were modified, nor the interfaces crate or any file outside `components/dispatcher/specs/` and `components/dispatcher/.specify/sync/`. The `spec_sync_*` frontmatter in `drift-report.md` was not touched; a script re-stamps it.

## Next Steps

1. Review the updated specs: `git diff -- components/dispatcher/specs`.
2. Re-stamp the drift-report frontmatter with `scripts/spec-sync-hash.sh`.
3. Implement A-1 (major) and A-2 from `align-tasks.md`. Confirm the A-3 decision. Model-check A-4.
4. Fix the `IDispatcher::clear_memory_tier` doc comment in the interfaces crate in a follow-up.
