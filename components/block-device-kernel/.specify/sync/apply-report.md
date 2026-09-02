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

---

# 2026-08-07 Sweep

Component: `block-device-kernel`

Mode: Interactive drift sweep (`component-sync-specs`, all specced components)

Applied: 2026-08-07 on branch `sync/spec-drift-sweep-20260807`

Source: `.specify/sync/drift-report.{json,md}` (regenerated 2026-08-07).

Pre-edit backups: `.specify/sync/backups/20260807T160256Z/actor.rs`.

## ⚠️ Discovered pre-existing breakage (surface to reviewer)

`block-device-kernel` **did not compile at HEAD**. The shared
`interfaces::Command` enum gained a `FlushSync { ns_id }` variant (handled by
`block-device-spdk-nvme`), but this crate's `process_command` match never
covered it → `E0004` non-exhaustive patterns. It went unnoticed because
`block-device-kernel` is a workspace **member but not a `default-member`**, so
neither `cargo build`, `cargo test --all`, nor CI ever builds it. The sibling
`block-device-filesys` had the identical breakage. This is a CI blind spot,
independent of the planned telemetry work.

## Changes Made (all staged on branch, nothing committed to `unstable`)

### Code (drafted for review — NOT auto-applied to `unstable`)

| File | Change | Category |
|------|--------|----------|
| `src/actor.rs` | Telemetry latency: `InflightOp.start_ns: u64` → `start: Instant`; real `Instant::now()` at submit; `start.elapsed()` at all sync + async completion sites | ALIGN (resolves 2026-07-22 FR-021/SC-006 defect task) |
| `src/actor.rs` | Added `Command::FlushSync { ns_id }` arm — validates `ns_id==1`, returns `FlushDone{Ok(())}` (validated no-op: device is `O_DIRECT\|O_DSYNC`, no volatile write cache) | Compile-unblocker (pre-existing breakage) |

Verified: `cargo build -p block-device-kernel`, `--features telemetry`, and
`cargo test` (both feature sets) all clean; my edits are `cargo fmt`-clean
(remaining fmt diffs in the crate are pre-existing committed long-line drift).

### Specs Updated (BACKFILL — document reality)

| Spec | Requirement | Change |
|------|-------------|--------|
| 001-block-device-kernel | Header | `Last Synced 2026-08-07` note |
| 001-block-device-kernel | FR-009 | Async `tag` currently emitted as `0`; clients correlate via `OpHandle`; differs from filesys (low-severity parity follow-up) |
| 001-block-device-kernel | US2 scenario 1 | Added `numa_node()==-1`, `nvme_version()=="N/A (kernel block device)"` |
| 001-block-device-kernel | FR-026 (new) | Device-info surface: `numa_node`/`nvme_version`/`read_write_stats` placeholder semantics |

### Implementation Tasks (see align-tasks.md 2026-08-07 section)

- FR-021/SC-006 telemetry latency — **DRAFTED on branch** (dedicated
  accuracy test still deferred).
- FlushSync arm — **DRAFTED on branch**, flagged as pre-existing-breakage fix
  needing review of the no-op contract.
- Async `tag` parity — LOW, queued (not drafted; behavioral choice).

### Not Applied / Deferred

None deferred. The two code fixes are drafted-but-uncommitted pending user
review per the branch-work-only directive.

## Next Steps

1. Review the two drafted `src/actor.rs` code changes (telemetry + FlushSync).
2. Confirm the kernel `FlushSync` no-op contract is intended.
3. Decide whether to fix the CI blind spot (add kernel/filesys to
   `default-members`, or a dedicated CI job that builds them).
4. Commit on the branch only — never to `unstable`.

---

# 2026-08-20 Phase B (Spec-Sync)

Component: `block-device-kernel`

Mode: Spec-Sync Phase B (policy `.specify/sync/PHASE_B_POLICY.md`)

Applied: 2026-08-20

Source: `.specify/sync/drift-report.{json,md}` (regenerated 2026-08-20 — 43
requirements checked, 41 aligned, 2 drifted, 0 not-implemented, 1 unspecced).

Pre-edit spec backup: `.specify/sync/backups/001-block-device-kernel.spec.md.20260820T171219Z.bak`.

**No `.rs` source was modified and no cargo command was run by this sync.**

## Key finding

The 2026-08-07 telemetry-latency fix ("drafted on branch") only **partially**
landed. The sync paths (`src/actor.rs:332,397,637`) and the blocking
`wait_for_cqe` completion site (`:718`) now record real elapsed latency, but the
primary async-completion path `harvest_completions()` still records a hardcoded
`0` (`src/actor.rs:776`). Async `ReadAsync`/`WriteAsync` ops thus log 0 ns
latency, so FR-021/SC-006 do **not** hold for async IO. Routed to an ALIGN task.

## Changes Made

### Specs Updated

| Spec | Requirement | Change Type | Direction |
|------|-------------|-------------|-----------|
| 001-block-device-kernel | FR-027 (new) | Added — `FlushSync` validated no-op | BACKFILL-UNSPECCED |
| 001-block-device-kernel | US2 acceptance scenario 5 (new) | Added — `FlushSync` behavior | BACKFILL-UNSPECCED |
| 001-block-device-kernel | Header `Last Synced` note | Modified — corrected the 2026-08-07 "fully fixed" claim to "partial; async path still 0 ns" | metadata |

FR-021 requirement text was **not** changed — it is correct as written; the
defect is in code, so it was routed to `align-tasks.md` rather than backfilled
(BACKFILL vs ALIGN rule).

### Align Tasks Generated

1 task in `.specify/sync/align-tasks.md` ("2026-08-20 Phase B" section):

- **FR-021 & SC-006 (moderate)** — `harvest_completions()` (`src/actor.rs:776`)
  records 0 ns latency for async ops despite `InflightOp.start: Instant` being
  populated. Single task covers both requirements (same root cause). Suggested
  fix: pass `op.start.elapsed().as_nanos() as u64`, mirroring the existing
  correct site at `:718`.

### Unspecced Backfilled

- **`Command::FlushSync` handler** (`src/actor.rs:233-247`) → new **FR-027** +
  US2 acceptance scenario 5. Validated no-op for `ns_id==1` (device is
  `O_DIRECT|O_DSYNC`, no volatile write cache); `ns_id!=1` → `InvalidNamespace`.
  Parallel to `block-device-filesys` FR-022 (which issues a real `fdatasync`).

### Resolved

None this run.

### Human Decision

None this run.

## Counts

| Category | Count |
|----------|-------|
| BACKFILL applied (drifted requirement) | 0 |
| UNSPECCED backfilled | 1 |
| ALIGN tasks | 1 (FR-021 + SC-006) |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

## Next Steps

1. Review the corrected header note and new FR-027 in
   `specs/001-block-device-kernel/spec.md`.
2. Implement the one-line async-latency fix per `align-tasks.md` (small effort).

---

# 2026-09-02 Re-verify (Spec-Sync)

Component: `block-device-kernel`

Mode: Spec-Sync re-verify (single component)

Applied: 2026-09-02T21:28:14Z

Source: `.specify/sync/drift-report.{json,md}` (regenerated 2026-09-02 against
HEAD `2fc1cd3c` — 44 requirements checked, 42 aligned, 2 drifted,
0 not-implemented, 0 unspecced).

Pre-edit spec backup: `.specify/sync/backups/20260902T212814Z/spec.md`.

**No `.rs` source was modified and no cargo command was run by this sync.**

## Key finding

The async-path telemetry-latency defect is **unchanged** at HEAD: `src/actor.rs:776`
still calls `self.telemetry.record_op(0, op.bytes)`, so async `ReadAsync`/`WriteAsync`
ops record 0 ns latency and FR-021/SC-006 do not hold for async IO. The FlushSync
backfill (FR-027) from 2026-08-20 is confirmed landed in the spec and aligned in
code — it is no longer unspecced, so the requirement count rose 43→44 and unspecced
dropped 1→0.

## Changes Made

### Specs Updated

| Spec | Requirement | Change Type | Direction |
|------|-------------|-------------|-----------|
| 001-block-device-kernel | Header `Last Synced` note | Modified (metadata) — recorded 2026-09-02 re-verification against HEAD `2fc1cd3c`, confirming the residual async defect is unchanged | metadata |

No FR/SC requirement text was changed. FR-021/SC-006 remain correct as written;
the defect is in code (ALIGN, not BACKFILL).

### Align Tasks Generated

1 task re-affirmed in `.specify/sync/align-tasks.md` ("2026-09-02 re-verify"
section):

- **FR-021 & SC-006 (moderate)** — `harvest_completions()` (`src/actor.rs:776`)
  records 0 ns latency for async ops despite `InflightOp.start: Instant` being
  populated. STILL OPEN — re-affirms the 2026-08-20 Phase B task. Suggested fix:
  pass `op.start.elapsed().as_nanos() as u64`, mirroring the correct site at `:718`.

### Unspecced Backfilled

None this run (FlushSync/FR-027 already landed 2026-08-20).

### Resolved / Human Decision

None this run.

## Counts

| Category | Count |
|----------|-------|
| BACKFILL applied (drifted requirement) | 0 |
| UNSPECCED backfilled | 0 |
| ALIGN tasks | 1 (FR-021 + SC-006, re-affirmed / still open) |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

## Next Steps

1. Implement the one-line async-latency fix per `align-tasks.md` (small effort),
   then re-run spec-sync to clear FR-021/SC-006 to `clean`.
