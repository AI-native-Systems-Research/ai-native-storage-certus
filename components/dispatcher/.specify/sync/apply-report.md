# Sync Apply Report — dispatcher

Applied: 2026-11-09T07:13:09Z (apply of the 2026-11-08 analysis) · Branch `fix/dispatcher-j33-repairs` · HEAD `d45ab524d` · Mode: BATCH · Backup: `backups/20261109T071309Z/`

Approved by Cornel 2026-11-08. Each proposal carries the note *"Approved by Cornel 2026-11-08 (all proposals of the 2026-11-08 analysis except gpu-services P-5)."* All 4 proposals are `approved: true`.

Before each replacement the old text was checked to occur exactly once in its file. Every check passed (4 of 4, including B-50's additional header edit); nothing was guessed.

## Changes Made

### Specs Updated

| Id | Spec | Requirement | Change Type |
|---|---|---|---|
| B-50 | 001 | FR-061 drain-all-on-error bullet: a timeout fails every not-yet-failed job of its drive; reverses the applied B-44 | Modified |
| B-50 | 001 | Header: new `Last Synced 2026-11-08` line after the 2026-11-07 line (kept) | Added |
| B-51 | 001 | User Story 11, acceptance scenario 4: timeout / broken-channel exception, tracked as A-18 | Modified |
| B-52 | 001 | FR-056 `read_write_stats`: A-16 gap sentence replaced by the one-lock-hold rule (`d45ab524d`) | Modified |

All edits are in `specs/001-dispatcher-cache-interface/spec.md` (now at `:45`, `:221`, `:400`, `:473`). The new `Last Synced` header line is the spec's "Last Modified" metadata. No SUPERSEDE proposals in this run.

### New Specs Created

None.

### Implementation Tasks Generated

1 task in `.specify/sync/align-tasks.md` (section "Apply 2026-11-08"):

- **A-18** — 001 US-11 AS4 + FR-061: redo per-job timeout isolation safely. Never release a slot the drive may still write, and bound a hung drive to about one `READ_TIMEOUT_MS`. It subsumes A-15. Moderate, MEDIUM confidence.

Also in `align-tasks.md`:
- **A-16** marked ✅ RESOLVED by `d45ab524d` (requested by B-52's `on_apply`). Its concurrent-reader criterion becomes a Loom-test follow-up.
- **A-15** re-anchored (it stays open).
- Note **N-14** recorded.

### Not Applied

| Proposal | Reason |
|---|---|
| — | none: every proposal was approved and every old text matched exactly once |

Unapproved proposals with an `approval_note`: none, so no open items were added. The note's exception, gpu-services P-5, belongs to the gpu-services sync.

Not proposed (recorded in `proposals.json`): B-43 and A-13 do not apply, because fix 7 is out. Notes N-13 and N-14 are deferred to the `lookup_async` issue.

## Files

- **Edited:**
  - `specs/001-dispatcher-cache-interface/spec.md`
  - `.specify/sync/align-tasks.md`
  - `.specify/sync/drift-report.md`: one `Apply 2026-11-08` note inserted below the frontmatter; the `spec_sync_*` frontmatter is untouched.
  - `apply-report.md` and `apply-report.json` (this report).
- **Backed up** to `backups/20261109T071309Z/` with the Write tool:
  - `specs/001-dispatcher-cache-interface/spec.md`, byte-identical to the original (checked with `diff`).
  - The previous `apply-report.md.prev` and `apply-report.json.prev` (checked with `cmp`).
  - `align-tasks.md` had no uncommitted changes, so `git show HEAD:` holds its previous version. The `drift-report.md` change is a pure insertion of one note block.
- No `src/` file and no `components/interfaces` file changed.

## Next Steps

1. Review the updated spec: `git diff -- components/dispatcher/specs`.
2. Commit: `git add components/dispatcher/specs components/dispatcher/.specify/sync && git commit -m "dispatcher: spec-sync apply 2026-11-08"`.
3. Implement A-18, tests first. When it lands:
   - remove B-50's and B-51's exception text;
   - close A-15;
   - amend FR-028 for the leaked slot.
4. Optional: add a Loom model for the A-16 window.
5. Re-stamp with the stamping script after the commit.

Previous report: `backups/20261109T071309Z/apply-report.md.prev`.
