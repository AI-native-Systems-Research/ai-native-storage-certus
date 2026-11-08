# Sync Apply Report — dispatcher

Applied: 2026-11-08T00:23:07Z (apply of the 2026-11-07 analysis) · Branch `fix/dispatcher-j33-repairs` · HEAD `f4579a41f` · Mode: BATCH · Backup: `backups/20261108T002307Z/`

Approved by Cornel 2026-11-07. Each proposal carries the note *"Approved by Cornel 2026-11-07 (all proposals except gpu-services P-5; B-38 and B-45 with the options as proposed)."* All 17 proposals are `approved: true`.

Before each replacement the old text was checked to occur exactly once in its file. Every check passed; nothing was guessed.

## Changes Made

### Specs Updated

| Id | Spec | Requirement | Change Type |
|---|---|---|---|
| B-35 | 001 | FR-012 (failed `initialize` leaves the tunables unchanged) | Modified |
| B-36 | 001 | FR-056 `read_write_stats` (monotonic) | Modified |
| B-37 | 002 | FR-033 (variant mapping; third cause counts neither) | Modified |
| B-38 | 002 | FR-035 (`dispatcher` background-evictor gap declared; BACKFILL option as proposed) | Added (sub-bullet) |
| B-39 | 001 | FR-039 step (1) (exhausted retries → `Timeout`/`IoError`) | Modified |
| B-40 | 001 | `contracts/errors.md` `Timeout` row; `IoError` row (raised-by + condition) | Modified |
| B-41 | 001 | FR-028 (no-drives promotion; closes A-04) | Modified |
| B-42 | 001 | User Story 9, acceptance scenario 3 | Modified |
| B-44 | 001 | FR-061 drain-all-on-error bullet (timeout fails only its job) | Modified |
| B-45 | 001 + 002 | 001 FR-058, 001 SC-017, 002 FR-025 (route partition; remote-on-fetch exception; BACKFILL option as proposed) | Modified |
| B-46 | 001 | FR-062 drive bullet + User Story 13 AS1 | Modified |
| B-47 | 001 | FR-001 method inventory (two lines) | Modified |
| B-48 | 001 | FR-034 (registration in every mode) | Modified |
| B-49 | 001 | FR-037 + FR-052 (`warm_stream` has no reader) | Modified |

Header lines: spec 001 and spec 002 each get a `Last Synced 2026-11-07` line. No SUPERSEDE proposals in this run.

### New Specs Created

None.

### Implementation Tasks Generated

3 tasks in `.specify/sync/align-tasks.md` (section "Apply 2026-11-07"):

- **A-15** — 001 FR-028 vs FR-062: leak (log), do not free, the slot of a timed-out read. Moderate, MEDIUM confidence.
- **A-16** — 001 FR-056 `read_write_stats`: hold the `data_drives` write lock across absorb and swap. Minor, HIGH confidence.
- **A-17** — 002 FR-033: count `evictions_blocked_unpersisted` only for a MemoryTier entry. Low, MEDIUM confidence.

Also in `align-tasks.md`: **A-04** marked ✅ RESOLVED by `60838384e` (requested by B-41). N-13 recorded as a note (deferred `lookup_async` issue). A-13 and A-14 recorded as withdrawn or closed.

### Not Applied

| Proposal | Reason |
|---|---|
| — | none: every proposal was approved and every old text matched exactly once |

Unapproved proposals with an `approval_note`: none, so no open items were added. The note's exception, gpu-services P-5, belongs to the gpu-services sync.

Withdrawn before this apply (not proposals): B-43 and A-13 (fix 7 was taken out, `209b01de5`). Closed: A-14 (`9a79224a1`) and A-04 (`60838384e`, by B-41).

### Out of scope, not done

- Optional interfaces doc edits that B-45 names (`components/interfaces/src/idispatcher.rs:346-349`, `:375-377`). The `interfaces` crate is outside this apply.
- Recorded-only items from the analysis: the five `repair_test_*` modules without `#[cfg(test)]`, the stale "100ms timeout" doc, and the `lookup_async` doc and FR-036 gap.

## Files

- Edited: `specs/001-dispatcher-cache-interface/spec.md`, `specs/001-dispatcher-cache-interface/contracts/errors.md`, `specs/002-served-by-tier-attribution/spec.md`, `.specify/sync/align-tasks.md`, `.specify/sync/drift-report.md` (one note inserted below the frontmatter; frontmatter untouched), `apply-report.md` and `apply-report.json` (this report).
- Backed up to `backups/20261108T002307Z/`: the three spec files, each byte-identical to its original (checked with `cmp`), and the previous `apply-report.md.prev` and `apply-report.json.prev`. `align-tasks.md` had no uncommitted changes, so `git show HEAD:` holds its previous version. The `drift-report.md` change is a pure insertion of one note block.
- No `src/` file and no `components/interfaces` file changed.

## Next Steps

1. Review the updated specs (`git diff -- components/dispatcher/specs`).
2. Commit: `git add components/dispatcher/specs components/dispatcher/.specify/sync && git commit -m "dispatcher: spec-sync apply 2026-11-07"`.
3. Implement A-15, A-16 and A-17, each with a test written first. When each lands, remove the matching known-gap sentence (B-44, B-36, B-37).
4. Re-stamp with the stamping script after the commit.

Previous report: `backups/20261108T002307Z/apply-report.md.prev`.
