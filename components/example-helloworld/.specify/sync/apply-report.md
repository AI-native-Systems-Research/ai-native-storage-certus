# Apply Report: example-helloworld

**Generated**: 2026-09-02T21:28:34Z
**Git commit**: 2fc1cd3c
**Inputs sha256**: e70f6f8916c8214c3bcb5c9659b59d8edd8f499399154506c4b98c14a950dbe6
**Drift status**: CLEAN

## Actions Taken

| Category | Count | Detail |
|----------|-------|--------|
| BACKFILL (spec edits) | 0 | No drift; no spec `.md` edits required. |
| ALIGN (code follow-ups appended) | 0 | No new align tasks this run. |
| HUMAN_DECISION | 0 new | 2 pre-existing align tasks carried over unchanged. |
| Backups created | 0 | No spec files were modified, so no backup was needed. |

## Notes

- This run re-verified all 10 requirements plus both user stories and the
  success criteria against `src/lib.rs`, `Cargo.toml`, `components/interfaces/`,
  and `apps/helloworld-mainline/`. Every item is aligned with `file:line`
  evidence; nothing drifted, nothing unspecced.
- The drift-report frontmatter stamp was refreshed (commit, timestamp, inputs
  hash) via `scripts/spec-sync-hash.sh`.
- `align-tasks.md` was left intact: Task 1 (deferred mainline logger-wiring code
  change) and Task 2 (informational IGreeter-promotion decision) remain open and
  are correctly out of scope for spec-sync's spec-only apply.

## Files Changed (this run)

- `.specify/sync/drift-report.md` (refreshed + stamped)
- `.specify/sync/drift-report.json` (refreshed + stamped)
- `.specify/sync/proposals.md` (regenerated)
- `.specify/sync/proposals.json` (regenerated)
- `.specify/sync/apply-report.md` (this file)
- `.specify/sync/apply-report.json`
