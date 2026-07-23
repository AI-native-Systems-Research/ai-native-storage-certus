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
