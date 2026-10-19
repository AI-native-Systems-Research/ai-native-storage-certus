# Sync Apply Report — dispatcher

Applied: 2026-10-19T07:04:14Z (sync date 2026-10-14) · Branch: `fix/bundle-dispatcher-ems` (HEAD `4f8b5554b`) · Mode: batch
Approved by: Cornel Constantinescu, 2026-10-18 (PR approval page, groups A-D)
Backup: `.specify/sync/backups/20261019T070414Z/specs/{001-dispatcher-cache-interface,002-served-by-tier-attribution}/spec.md`

## Changes Made

### Specs Updated

All BACKFILL edits are the exact old→new text from `proposals.md`. Before the replacement, each `old` string was confirmed to occur exactly once.

| Proposal | Spec | Requirement | Change Type |
|---|---|---|---|
| B-01 | 001 | FR-036 (lookup pin through promotion) | Modified |
| B-01 | 001 | FR-051 (single-key lookup race recovery) | Modified |
| B-02 | 001 | FR-032 (SSD evictor dm.remove first) | Modified |
| B-02 | 001 | Edge case: SSD evictor removal | Modified |
| B-03 | 001 | FR-003 (release slot after post-reserve failure) | Modified |
| B-03 | 001 | FR-059 (`batch_populate` per-entry release) | Modified |
| B-04 | 001 | FR-056 `copy_gpu_to_memory_completed` | Modified |
| B-05 | 001 | FR-042 (all emitters, `EvictionSender`) | Modified |
| B-05 | 001 | FR-050 (stale `Removed` clause) | Modified |
| B-06 | 001 | FR-019 / US-9 AS1 / Q&A / Key Entities (5 edits) | Modified |
| B-07 | 001 | FR-037 (stream lifetime at shutdown) | Modified |
| B-08 | 001 | Edge case: remove order | Modified |
| B-09 | 001 | FR-003 + Assumptions (D2H primitive) | Modified |
| B-10 | 002 | FR-008 (cold sub-paths) | Modified |
| B-11 | 001 | User Story 11 AS4 | Modified |
| N-02 | 001 | FR-045 — `CERTUS_LOG_REMOTE_DELIVERY` diagnostics note | Added (note) |
| N-03 | 001 | FR-056 `reserve_memory` — `CERTUS_LOG_RESERVE` diagnostics note | Added (note) |
| — | 001 | `**Last Synced**: 2026-10-14` header | Added |
| — | 002 | `**Last Synced**: 2026-10-14` header | Added |

### New Specs Created
- None. N-02 and N-03 are notes on existing FRs, appended with the draft text exactly as given.

### Supersede
- None proposed.

### Implementation Tasks Generated
- 7 ALIGN tasks in `.specify/sync/align-tasks.md`: A-01, A-02, A-03, A-04, A-05, A-06, A-08. All were approved as tasks only, so no code was changed.
- 3 open decision items in the same file: A-07, N-01, N-04.

### Not Applied
| Proposal | Reason |
|---|---|
| A-07 | Not approved. Waits for Cornel's decision (ALIGN or BACKFILL). Recorded as an open item. |
| N-01 | Not approved. A MEDIUM-confidence new requirement (Cornel's rule). Recorded as an open item. |
| N-04 | Not approved. MEDIUM confidence, and it belongs to the remove-no-wait PR. Recorded as an open item. |

No source files were modified, nor the interfaces crate or any file outside `components/dispatcher/specs/` and `components/dispatcher/.specify/sync/`. The `spec_sync_*` frontmatter in `drift-report.md` was not touched; a script re-stamps it.

## Next Steps

1. Review the updated specs: `git diff -- components/dispatcher/specs`.
2. Re-stamp the drift-report frontmatter with `scripts/spec-sync-hash.sh`.
3. Implement A-01, A-02 and A-03 (major) from `align-tasks.md`.
4. Decide A-07, N-01 and N-04.
