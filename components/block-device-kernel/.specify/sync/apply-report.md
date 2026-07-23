# Sync Apply Report

Component: `block-device-kernel`

Mode: AUTO-BACKFILL

Applied: 2026-07-22T23:20:00Z

Source: `.specify/sync/drift-report.{json,md}` (3 drifted items, 1 unspecced
feature, 0 not-implemented, 38/41 aligned).

## Changes Made

### Specs Updated

| Spec | Requirement | Change Type | Direction |
|------|-------------|-------------|-----------|
| 001-block-device-kernel | FR-002 | Modified | BACKFILL |
| 001-block-device-kernel | Edge case "Client callback channel disconnected" | Modified (renamed to "full or disconnected") | BACKFILL |
| 001-block-device-kernel | FR-025 (new) | Added | BACKFILL (unspecced feature) |
| 001-block-device-kernel | Key Entities / `ClientSession` | Modified (added `pending` field) | BACKFILL |
| 001-block-device-kernel | Header metadata | Added `Last Synced` line | metadata |

Backup of pre-edit `spec.md` saved to
`.specify/sync/backups/001-block-device-kernel.spec.md.20260722T232037Z.bak`.

**FR-002** previously claimed channel disconnections are logged at warn
level and completions are silently dropped on a full/disconnected callback
channel. Code has no `warn()` call anywhere in the crate and never drops a
completion — instead `ClientSession` buffers into an unbounded FIFO backlog
(`pending`) that `poll_clients()` retries every idle tick. This is a
deliberate, code-comment-documented anti-head-of-line-blocking design
(protects other clients on the drive from one slow/stalled client), not a
bug, so the spec was updated to describe the actual behavior rather than
generating an align-task to add warn logging. A new **FR-025** documents the
backlog/retry mechanism itself (previously the sole "Unspecced Code" entry in
the drift report), and the edge-case bullet was rewritten to match.

### New Specs Created

None — the one unspecced feature (completion backlog) was folded into the
existing `001-block-device-kernel` spec via FR-025 rather than split into a
new spec file, per the drift report's own suggestion ("amend FR-002 /
edge cases section") — it is an implementation detail of the existing
completion-delivery requirement, not an independently addressable feature.

### Superseded

None.

### Implementation Tasks Generated

1 task in `.specify/sync/align-tasks.md`:

- **FR-021 / SC-006 (major)** — Telemetry latency stats (`min/max/mean_latency_ns`)
  are hardcoded to 0 at every call site (`src/actor.rs:312,373,609,689,747`);
  `InflightOp.start_ns` uses `Instant::now().elapsed()` on a freshly-created
  instant instead of a real submission-time capture. This is a functional
  defect (the telemetry feature's core value proposition silently reports
  meaningless data) and a guarantee violation of FR-021/SC-006 as written, so
  per the HARD RULE it was **not** backfilled into the spec. Spec text is
  unchanged; a code-fix task was appended to `align-tasks.md` instead.

### Not Applied / Deferred

None — all 3 drifted items and the 1 unspecced item were resolved (2 items
backfilled into the spec, 1 routed to align-tasks per the DEFECT rule, and
the unspecced feature folded into the same backfill as a new FR).

## Next Steps

1. Review the updated `specs/001-block-device-kernel/spec.md` (FR-002,
   FR-025, edge case, Key Entities, header) for accuracy.
2. Implement the telemetry-latency fix described in `align-tasks.md`
   (severity: major).
3. Commit changes: `git add specs/ .specify/sync/ && git commit -m "sync: apply drift resolutions for block-device-kernel"`.
