# Sync Apply Report — dispatcher

Applied: 2026-10-24 (analysis run 2026-10-23) · Branch: `fix/dispatcher-remove-ssd-extent` (HEAD `765f75ba3`, base `origin/unstable` `a905f89be`) · Mode: batch, no questions
Approved by: Cornel, 2026-10-24 ("Yes approve B-14 and B-15"; B-12 approved 2026-10-23 on the earlier run and re-checked against this branch by the analysis)
Backup: the pre-apply state of every edited file is the branch HEAD `765f75ba3` (all edited files are tracked; `git diff` shows this apply). The previous apply-report (2026-10-21, branch `fix/dispatcher-remove-clear-mt`) is at that HEAD too.

## Changes Made

All BACKFILL edits use the exact old→new text from `proposals.md` / `proposals.json`. Before
each replacement, the `old` string was confirmed to occur exactly once in its file.

| Proposal | Spec | Requirement | Change Type |
|---|---|---|---|
| B-12 | 001 `spec.md` | FR-009: the extent is freed when the entry has one (BlockDevice, or MemoryTier whose write-through landed); a lookup releases only a read reference it acquired | Modified |
| B-12 | 001 `spec.md` | US-4 narrative: same widening of the extent rule | Modified |
| B-12 | 001 `contracts/idispatcher.md` | `remove` doc comment: "If the entry has an SSD copy …" | Modified |
| B-14 | 001 `spec.md` | remove edge case: a refused `remove` frees nothing AND leaves the entry as it found it (same location, slot, references); the brief BlockDevice window recorded as a known gap (align task A-12) | Modified |
| B-14 | 001 `contracts/idispatcher.md` | `remove()` postcondition: unchanged entry on a held `KeyNotFound` | Modified |
| B-15 | 001 `spec.md` | FR-056 `CERTUS_LOG_RESERVE` citation `src/lib.rs:3380-3395` → `:3589-3604` | Modified |
| B-15 | 002 `spec.md` | `GetIoStats` citation `src/lib.rs:3257-3272` → `:4226-4236` (`read_write_stats`) | Modified |
| — | 001 `spec.md` | `**Last Synced**: 2026-10-24` header line for branch `fix/dispatcher-remove-ssd-extent` (the proposals' "Order" step 1 asked for it) | Added |

No source files were changed by this apply.

## Recorded in `align-tasks.md` (no code change)

- **Closed**: A-2 (resolved by `a7444f4ab`; restart criterion has no test), A-06 (resolved
  by PR #568), A-9 (implemented by `765f75ba3`, option (c); option (a) carried as A-12).
- **New tracked tasks**: A-11 (the SSD evictor releases a read reference it never took —
  the defect class of root cause #12, the last such site in `src/`; TRACK note, Cornel
  2026-10-24) and A-12 (close the flip window of `remove` with one dispatch-map operation;
  TRACK note, Cornel 2026-10-24).
- Carried over, unchanged: A-1, A-3 (decision), A-4, A-5, A-10, A-01, A-02, A-04, A-05,
  A-08, N-01.

## Scratch files

The analysis left `drift-report.note.part` and `drift-report.body.part` in this folder
(both empty; their content is already in `drift-report.md`). They can be deleted.
