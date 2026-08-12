# Sync Apply Report

Applied: 2026-07-22
Based on: drift-report.md / drift-report.json (generated 2026-07-22T22:46:53Z)
Mode: AUTO-BACKFILL

## Backups

Pre-edit copies saved under `.specify/sync/backups/`:
- `001-spec.md.20260722-162218.bak`
- `001-research.md.20260722-162218.bak`
- `002-spec.md.20260722-162218.bak`

## Changes Made

### Specs Updated (BACKFILL: code -> spec)

| Spec | Requirement | Change Type | Source |
|------|-------------|--------------|--------|
| 001-spdk-nvme-block-device | FR-027 (new) | Added — `signal_stop()` / `detach_controller()` lifecycle methods on `IBlockDeviceAdmin` | Unspecced item #1, #2 |
| 001-spdk-nvme-block-device | FR-028 (new) | Added — graceful drain on actor stop (5s deadline, `Completion::Error{Aborted}` for stragglers) and controller parking for safe detach ordering | Unspecced item #5, #6 |
| 001-spdk-nvme-block-device | FR-029 (new) | Added — fair round-robin client-polling rotation and per-client-per-poll command cap (`MAX_COMMANDS_PER_CLIENT_PER_POLL = 64`) | Unspecced item #3, #4 |
| 001-spdk-nvme-block-device | Assumptions section | Modified — corrected channel description from "crossbeam bounded to 64 slots" to actual `SpscChannel` bounded to `CLIENT_CHANNEL_CAPACITY` (256) | Unspecced item #7, #10 |
| 001-spdk-nvme-block-device | research.md R-002 | Modified — corrected crossbeam alternative note to reflect that `SpscChannel` (256 slots) is used uniformly and crossbeam was never wired in | Unspecced item #7 |
| 002-iops-benchmark | FR-026 (new) | Added — `--device-count` multi-device concurrent benchmarking (parallel attach/init, worker scaling, aggregated reporting) | Unspecced item #9 |

No `NEW_SPEC` folders were created. All 10 previously-unspecced items were
either genuine extensions of the existing component's documented scope
(lifecycle/shutdown, polling fairness, multi-device benchmarking — folded
into specs 001/002 above as new FRs) or code-hygiene/documentation items
(dead code, unused dependency, stale doc) that do not describe a standalone
production feature warranting a new spec directory. See align-tasks.md for
the latter.

### Drift Items — NOT Backfilled (code defects, per HARD RULES)

Per instructions, specs are never rewritten to match behavior flagged as a
bug, dead code, or compile error. All three drifted FRs from the 2026-07-22
drift report describe defective code, not a spec that needs updating.
Deferred to `.specify/sync/align-tasks.md` instead:

| Spec | Requirement | Severity | Reason |
|------|-------------|----------|--------|
| 001-spdk-nvme-block-device | FR-013 / SC-007 | High | NUMA node hardcoded to `0` in `probe_controller()` — code defect (explicitly called out in task instructions) |
| 001-spdk-nvme-block-device | FR-010 / SC-005 | Medium | `nvme_version`/`max_transfer_size` hardcoded in `NvmeController::attach()` instead of read from real hardware — code defect |
| 001-spdk-nvme-block-device | FR-011 / SC-008 | High | Telemetry feature test suite fails to compile (`record()` arity mismatch) — code defect (explicitly called out in task instructions) |

### Additional Align Tasks (dead code / stale docs, not spec drift)

| Item | Severity | Reason |
|------|----------|--------|
| `ControlMessage::DisconnectClient` | Low | Dead code — matched but never constructed anywhere |
| Unused `crossbeam-channel` dependency + stale bench doc comments | Low | Cleanup: dependency and `benches/*.rs` doc comments reference crossbeam, but production code uses `SpscChannel` |
| README.md channel-capacity mismatch | Low | Non-spec companion doc (`README.md`) out of edit scope for this pass; corrected in spec.md Assumptions instead, flagged for README fix |

### Not Applied

None — all identified drift/unspecced items were either backfilled into
specs 001/002 or recorded in align-tasks.md.

## Next Steps

1. Review the three new FRs (001: FR-027, FR-028, FR-029) and the new
   FR-026 in spec 002, plus the corrected Assumptions/research.md text.
2. Address the align-tasks.md items as separate code-fix work (NUMA
   discovery, telemetry test fix, device-info hardcode, dead-code removal,
   crossbeam cleanup, README fix). These are source-code/README changes and
   are intentionally NOT made by this sync-apply pass.
3. Commit:
   `git add components/block-device-spdk-nvme/specs components/block-device-spdk-nvme/.specify/sync && git commit`

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`)

Based on: `.specify/sync/drift-report.{json,md}` (generated 2026-08-07T15:38:00Z).
Mode: auto-apply safe backfills/doc-softens; the one HIGH item (FR-005 abort
use-after-free) had its **code fix drafted on the branch** per the user decision
"Draft fix + queue task" — staged for review, NOT committed to `unstable`.

## Backups

Pre-edit copies in `.specify/sync/backups/20260807T160256Z/`:
`actor.rs`, `spdk-sys-build.rs`, `Cargo.toml` (block-device).

## User Decisions Driving This Pass

- **FR-005 abort UAF** = **Draft fix + queue task** (code drafted on branch, needs hardware validation; align-task BD-1).
- **NUMA / device-info constants (FR-010/FR-013/SC-005/SC-007)** = **Backfill (document reality) + queue enhancement** (align-task BD-2).
- **FR-004 / FR-028 / 002 FR-022 / 002 FR-024** = **Backfill (working design)**.
- **Unused crossbeam dep** = **Auto-apply safe cleanup** (removed + bench doc comments fixed).

## Code Drafted on Branch (NOT committed to unstable)

| File | Change |
|------|--------|
| `components/spdk-sys/build.rs` | Allowlisted `spdk_nvme_ctrlr_cmd_abort_ext` (binding regenerated + confirmed present in OUT_DIR). |
| `components/block-device-spdk-nvme/src/actor.rs` | FR-005 buffer-safe abort: `PendingOp` gained `cmd_cb_arg`/`aborting`; `AbortOp` issues a real NVMe abort and keeps the buffer alive; `AbortAck` deferred to the real completion; added `abort_completion_cb`; test updated. |

**Verification**: `cargo build -p spdk-sys`, `cargo build -p block-device-spdk-nvme`
(and `--features telemetry`), `cargo test -p block-device-spdk-nvme` (39 pass),
`cargo bench -p block-device-spdk-nvme --no-run` — all clean. The hardware abort
path itself is unexercised here and must be validated on an NVMe test node.

## Code Applied on Branch (safe ALIGN)

| File | Change |
|------|--------|
| `components/block-device-spdk-nvme/Cargo.toml` | Removed unused `crossbeam-channel = "0.5"`. |
| `benches/latency.rs`, `benches/throughput.rs` | Corrected stale "crossbeam 64-slot" doc comments to the 256-slot `component_core` SpscChannel. |

## Specs Updated (BACKFILL — applied directly)

| Spec | Requirement | Change |
|------|-------------|--------|
| 001 | Header | Added Last Synced 2026-08-07 note. |
| 001 | FR-004 | Fire-and-forget submit; `tag` is the correlation key (handle echoed in completion, not returned at submit). |
| 001 | FR-005 | Documented the buffer-lifetime abort contract matching the drafted fix; cross-ref BD-1. |
| 001 | FR-010 / SC-005 | Documented `nvme_version`/`max_transfer_size`/`numa_id` as fixed constants pending BD-2; other fields real. |
| 001 | FR-013 / SC-007 | Documented NUMA pinning targets node 0 (controller NUMA discovery unimplemented); cross-ref BD-2. |
| 001 | FR-028 | Corrected to the implemented drain → `Error{Aborted}` → park order (race-free). |
| 001 | FR-030 (new) | `read_write_stats()` / `ReadWriteStats` per-direction counters. |
| 002 | Header | Added Last Synced 2026-08-07 note. |
| 002 | FR-022 | Sync QD1 via actor serialization, not worker submit-one-wait. |
| 002 | FR-024 | Per-sub-op latency timing (not aggregate-per-batch). |
| 002 | SC-006 | Stats from client-side timestamps; telemetry cross-check unwired (BD-3). |

## Align Tasks (see align-tasks.md)

- **BD-1** (High) — FR-005 abort UAF fix DRAFTED on branch; needs hardware validation. Includes a related `check_timeouts` UAF follow-up (same shape, NOT drafted).
- **BD-2** (High/Medium) — real device NUMA + Identify (VER/MDTS) discovery; continues July FR-013 + FR-010 tasks.
- **BD-3** (Low) — iops-benchmark telemetry cross-check (spec 002 SC-006).
- July FR-011 (telemetry test arity) and dead-code `DisconnectClient` remain open (not in scope this sweep).
- Unused-crossbeam task ✅ RESOLVED this sweep.

## Not Applied / Deferred

| Item | Reason |
|------|--------|
| Committing any of the above | Branch-only per standing instruction; nothing committed to `unstable`. |
| Hardware execution of the abort path | No NVMe hardware exercise in this environment; deferred to a test node. |
| FR-011 telemetry test fix, `DisconnectClient` removal | Pre-existing July align-tasks, out of scope for this drift sweep. |
