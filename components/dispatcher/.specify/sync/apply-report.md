# Sync Apply Report — dispatcher

Applied: 2026-10-21T22:53:16Z (analysis run 2026-10-21) · Branch: `fix/dispatcher-remove-clear-mt` (HEAD `efd502331`, base `origin/unstable` `a25e9be29`) · Mode: batch, no questions
Approved by: Cornel, 2026-10-21 ("Approved by Cornel 2026-10-21 (groups 1, 3, 4)")
Backup: `.specify/sync/backups/20261021T225316Z/specs/001-dispatcher-cache-interface/{spec.md,contracts/errors.md}` (the previous `apply-report.{md,json}` of 2026-10-20 are kept in the same folder)

## Changes Made

### Specs Updated

All BACKFILL edits use the exact old→new text from `proposals.md` / `proposals.json`. Before each replacement, the `old` string was confirmed to occur exactly once in its file.

| Proposal | Spec | Requirement | Change Type |
|---|---|---|---|
| B-7 | 001 `spec.md` | FR-056 `flush_to_ssd` bullet: return type `Result<usize, DispatcherError>`; the count is taken after the drain, counts `is_evictable` entries, and is a lower bound; only `Ok` vs `IoError` is the persistence guarantee. The approved sentence and the trailing `*(Sync 2026-10-21 — CODE CHANGE …)*` note are kept. | Modified |
| B-8 | 001 `spec.md` | FR-017: failed background write-through is no longer "silent": recorded per key, reported by `flush_to_ssd` as `IoError` while the key is resident, cleared by a landed write-through or the key leaving the tier (commit `9bd231d85`) | Modified |
| B-9 | 001 `contracts/errors.md` | `IoError` row: `flush_to_ssd` added to "Raised by", with its condition | Modified |
| — | 001 `spec.md` | `**Last Synced**: 2026-10-21` header line for branch `fix/dispatcher-remove-clear-mt` (the proposals' "Priority" asked for it) | Added |

Spec 002 was not touched (no proposal).

### Applied by hand before this run (recorded, not re-applied)

The two interfaces proposals that change `src/` doc comments were approved and already applied by hand in the working tree (`git diff -- components/interfaces/src/idispatcher.rs`). This apply did **not** edit `src/`.

| Proposal | File | Change |
|---|---|---|
| interfaces 5 (ALIGN, doc) | `components/interfaces/src/idispatcher.rs` `clear_memory_tier` doc | "Empty the memory tier …", pinned entries kept and retried, count = demoted or removed, `# Errors` `Timeout` |
| interfaces 6 (ALIGN, doc) | `components/interfaces/src/idispatcher.rs` `batch_lookup` doc | "Returns one [`LookupOutcome`] per input entry …" |

The B-7 companion (`idispatcher.rs` `flush_to_ssd` line "Returns the number of entries that now have a valid SSD offset") is outside `specs/**` and was not part of the BACKFILL; it is not changed.

### New Specs Created
- None.

### Supersede
- None proposed.

### Implementation Tasks Generated
- No new approved ALIGN task: the only ALIGN proposal of this run, A-5, is not approved.
- Closed in `align-tasks.md` (the proposals' `close_after_apply`):
  - **A-07** (FR-058 `store_already_resident`): resolved in code by `efd502331`, ALIGN option.

### Open items recorded (not approved, with an `approval_note`)
- **A-5** (FR-056 + FR-017: tie the failed-write record to the entry it describes, minor) → `align-tasks.md`, quoting *"TRACK (Cornel 2026-10-21): code change for a later repair with a test first; not in this PR."*
- **interfaces 7** (`dispatcher-p2p` `remove` / `clear_memory_tier` / `flush_to_ssd`, major) → recorded in `components/interfaces/.specify/sync/align-tasks.md` as a task for the component **`dispatcher-p2p`**, quoting *"TRACK (Cornel 2026-10-21): port the 3 dispatcher repairs to dispatcher-p2p in a separate PR with its own Loom tests and spec sync."* A cross-reference is in this component's `align-tasks.md`.

### Not Applied
| Proposal | Reason |
|---|---|
| A-5 | Not approved (TRACK; later repair, test first) |
| B-7 ALIGN alternative (`IDispatchMap::ssd_offset`) | Not chosen; depends on A-2 |

### Still open (carried over)
A-1, A-2, A-3 (decision recorded), A-4; A-01, A-02, A-04, A-05, A-06, A-08; N-01 (decision).

### Files modified by this apply
- `specs/001-dispatcher-cache-interface/spec.md`, `specs/001-dispatcher-cache-interface/contracts/errors.md`
- `.specify/sync/align-tasks.md`, `.specify/sync/apply-report.{md,json}`, `.specify/sync/drift-report.md` (one note below the frontmatter; `spec_sync_*` frontmatter not touched)
- `components/interfaces/.specify/sync/align-tasks.md` (interfaces 7, as instructed for this run)
- No source files.

## Next Steps

1. Review the updated specs (`git diff -- components/dispatcher/specs`).
2. Commit: `git add components/dispatcher/specs components/dispatcher/.specify/sync && git commit -m "sync(dispatcher): apply B-7, B-8, B-9"`.
3. Implement A-5 together with A-1 (test first); port the repairs to `dispatcher-p2p` (interfaces 7).
