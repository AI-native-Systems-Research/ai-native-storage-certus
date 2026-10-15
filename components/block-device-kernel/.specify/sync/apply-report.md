# Sync Apply Report

Component: `block-device-kernel`

Mode: BATCH (no questions)

Applied: 2026-10-14T18:15:42Z

Branch: `fix/block-device-kernel-timeout-cancel` (HEAD `5d123390`)

Approved by: Cornel Constantinescu, 2026-10-14

Source: `.specify/sync/proposals.json` (generated 2026-10-14T17:31:02Z), exact
old/new text from `.specify/sync/proposals.md`. All 9 proposals are
`approved: true`. No source code, interface crate or other file outside
`specs/` and `.specify/sync/` was changed.

Backups: `.specify/sync/backups/20261014T181542Z/` (`spec.md`, `plan.md`, and
the previous `align-tasks.md`, `apply-report.md`, `apply-report.json`,
`drift-report.md`).

## Changes Made

### Specs Updated

| Proposal | Spec | Requirement | Change Type |
|----------|------|-------------|-------------|
| `bdk-2026-10-14-01` | 001-block-device-kernel | NFR-006 (`on_idle()` return value) | Modified (BACKFILL), exact proposal text |
| `bdk-2026-10-14-01` (`also_update`) | 001-block-device-kernel `plan.md` | Design decision 6 (`plan.md:75`) | Modified, same meaning as the new NFR-006 |
| `bdk-2026-10-14-02` | 001-block-device-kernel | FR-024 (graceful shutdown) | Modified (BACKFILL), proposal text **without its last sentence**, per the approval note |
| `bdk-2026-10-14-03` | 001-block-device-kernel | Edge case "Async operation timeout" (covers SC-005) | Modified (BACKFILL), exact proposal text |
| `bdk-2026-10-14-04` | 001-block-device-kernel | Key Entity `KernelHandler` | Modified (BACKFILL), exact proposal text |
| `bdk-2026-10-14-05` | 001-block-device-kernel | Implementation note: bit-63 `user_data` CQEs | Modified (BACKFILL), exact proposal text |
| `bdk-2026-10-14-06` | 001-block-device-kernel | Implementation note: `on_idle()` return value | Replaced (SUPERSEDE), exact proposal text; superseded by `bdk-2026-10-14-01` (NFR-006) and `bdk-2026-10-14-02` (FR-024) |
| (metadata) | 001-block-device-kernel | Header **Last Synced** | New 2026-10-14 entry; the 2026-09-03 entry was kept, relabelled **Previous sync** |

SUPERSEDE handling: the superseded statement is an implementation note inside
the same spec, not a separate spec. So no `superseded_by` field was added to
any spec header. The note was replaced in place, and its new text points to
NFR-006 and FR-024. The supersede link is recorded here and in
`apply-report.json`.

Text dropped from FR-024 (approval note on `bdk-2026-10-14-02`): "Async
operations still in flight at shutdown receive no completion." It was not
applied. It is now an open item in `align-tasks.md`.

### New Specs Created

None.

### Implementation Tasks Generated

4 entries were appended to `.specify/sync/align-tasks.md` under "2026-10-14
Apply":

| Entry | Source | Kind |
|-------|--------|------|
| FR-012: `AbortOp` drops its `AsyncCancel` on a full SQ | `bdk-2026-10-14-07` (ALIGN) | Task, no code change in this PR |
| FR-025 / FR-002: dropped clients are never reclaimed (`DisconnectClient` never sent) | `bdk-2026-10-14-08` (ALIGN) | Task, no code change in this PR |
| NFR-004: five `unsafe` blocks lack `// SAFETY:` | `bdk-2026-10-14-09` (ALIGN) | Task, no code change in this PR |
| FR-024: what happens to in-flight async ops at shutdown (see `tasks.md` T011) | approval note on `bdk-2026-10-14-02` | Open item, needs a decision |

### Not Applied

| Proposal | Reason |
|----------|--------|
| `bdk-2026-10-14-02`, last sentence only | Dropped per the approval note; recorded as an open item |
| `bdk-2026-10-14-07`, `-08`, `-09` code changes | Approved as ALIGN tasks only ("no code change in this PR") |

There are no unapproved proposals, so no unapproved approval notes needed
recording.

### Still open (not proposals)

- **Buffer lifetime after `Timeout`/`AbortAck`**: a spec gap from the drift
  report. No requirement exists either way, and it needs a human decision. Not
  touched.

## Verification

- Each `old_text` was matched exactly once in `spec.md` before replacement.
  The new text is the proposal's `new_text` verbatim, except for the FR-024
  sentence noted above.
- No build or tests were run. Only spec and sync documents changed.

## Next Steps

1. Review the updated `specs/001-block-device-kernel/spec.md` and `plan.md`.
2. Commit: `git add components/block-device-kernel/specs components/block-device-kernel/.specify/sync && git commit -m "sync(block-device-kernel): apply approved drift resolutions"`.
3. Re-stamp the drift-report frontmatter with the hash script (not done here).
4. Implement the three ALIGN tasks and decide the FR-024 shutdown open item in `align-tasks.md`.
