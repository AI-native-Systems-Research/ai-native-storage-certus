# Drift Resolution Proposals — block-device-spdk-nvme

Generated: 2026-08-20 (Spec-Sync Phase B)
Based on: `.specify/sync/drift-report.json` (generated 2026-08-20)
Policy: `.specify/sync/PHASE_B_POLICY.md`

## Summary

| Resolution Type | Count |
|-----------------|-------|
| BACKFILL (spec → code) — drifted reqs | 3 |
| BACKFILL-UNSPECCED (new/extended reqs) | 8 |
| ALIGN (task, no code change) | 1 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

All 3 drifted requirements are spec-lag (working/intentional code, stale spec text) →
BACKFILL. All 8 unspecced features are working behaviors → BACKFILL-UNSPECCED. One
genuine (cosmetic) code defect was found *inside* unspecced feature #3 and is filed as
an ALIGN task (no `.rs` edited).

---

## Drifted requirements (BACKFILL)

### Proposal 1 — 001/FR-005 — abort buffer-lifetime contract

Direction: **BACKFILL** (spec-lag).

- Spec said: the buffer-safe abort fix is "drafted on branch
  `sync/spec-drift-sweep-20260807` and requires hardware validation (Task BD-1)."
- Code does: fully implements defer-until-completion — `Command::AbortOp` marks the op
  `aborting`, retains the `PendingOp` + pinned buffer, issues a real
  `spdk_nvme_ctrlr_cmd_abort_ext` (matched by `cmd_cb_arg`), and defers
  `Completion::AbortAck` until the original command's real completion, where the buffer
  is released; unknown handles ack immediately. See `src/actor.rs:972-1020` and
  `src/actor.rs:528-537`.
- Resolution: rewrote the FR-005 status note to state the contract is implemented in
  mainline; the "drafted / needs hardware validation" wording is superseded. BD-1 marked
  RESOLVED in align-tasks.md.

### Proposal 2 — 001/FR-010 — device-info fixed fields

Direction: **BACKFILL** (spec-lag).

- Spec said: `max_transfer_size` returns 131072 (128 KiB) as a fixed constant (one of
  three fixed fields).
- Code does: `max_transfer_size` is auto-detected from the controller MDTS via
  `spdk_nvme_ctrlr_get_max_xfer_size` (`src/controller.rs:169-177`); 131072 is only the
  fallback when MDTS == 0. `nvme_version` (1.0.0, `src/controller.rs:156-161`) and
  `numa_id` (0, `src/lib.rs:333-334`) ARE genuinely hardcoded.
- Resolution: moved `max_transfer_size` to the hardware-derived list; the two remaining
  fixed fields (`nvme_version`, `numa_id`) stay tracked under align-tasks.md Task BD-2.

### Proposal 3 — 001/SC-005 — device-info consistency

Direction: **BACKFILL** (spec-lag, same root cause as FR-010).

- Spec said: `nvme_version`, `max_transfer_size`, and `numa_id` are fixed constants.
- Code does: `max_transfer_size` is MDTS-derived (hardware-consistent); only
  `nvme_version` and `numa_id` are fixed.
- Resolution: reworded SC-005 to include `max_transfer_size` among hardware-consistent
  fields and list only the two remaining fixed fields.

---

## Unspecced features (BACKFILL-UNSPECCED)

### Proposal 4 — 001/FR-031 (new) — synchronous flush durability barrier

`Command::FlushSync { ns_id }` → `do_sync_flush` via `spdk_nvme_ns_cmd_flush`, returning
`Completion::FlushDone` (`src/actor.rs:941-951,1214-1260`). Added **FR-031** to spec 001
plus a new acceptance scenario under User Story 1. (The in-code comment cites
extent-manager FR-030, a different spec; spec 001 now owns this requirement.)

### Proposal 5 — 001/Assumptions — dead `probe()` helper note

`namespace::probe()` (`src/namespace.rs:20-47`) is `#[allow(dead_code)]`, superseded by
`discover_namespaces`. Not a live behavior, so per the suggested_spec ("note as internal
helper") it is documented in the Assumptions section as a superseded legacy helper /
removal candidate — no behavioral FR added — so the drift sweep stops re-flagging it.

### Proposal 6 — 002/FR-015 — GB/s throughput + per-thread IOPS breakdown

`throughput_gbps` (`stats.rs:38,83`, printed at `report.rs:122-124`) and the per-thread
IOPS breakdown (`report.rs:74-103`, read/write split in `rw` mode) are additive report
fields. Extended FR-015 to describe them.

### Proposal 7 — 002/FR-024 — batch send-failure rollback

On a failed `Command::BatchSubmit` send, the worker rolls back the just-enqueued
in-flight entries and decrements its submit counter (`worker.rs:158-171`), preventing
phantom in-flight ops. Backfilled into FR-024.

### Proposal 8 — 002/FR-026 — parallel init + per-device summary

Parallel device init via `std::thread::scope` with distinct NUMA-local actor-CPU
assignments and `[timing]` stderr lines (`main.rs:52-55,105-153`), plus the multi-device
`=== Per-Device Summary ===` block (`main.rs:397-428`). Backfilled both into FR-026.
The cosmetic format-string defect at `main.rs:423` is filed as an ALIGN task (below).

### Proposal 9 — 002/SC-001 — barrier-based start sync

A start barrier (`Barrier::new(total_workers + 1)`) ensures init/attach/connect time is
excluded from the measured wall-clock window; the clock is taken immediately before
`start_barrier.wait()` (`main.rs:262,328-329`; `worker.rs:106`). Documented in SC-001 as
measurement methodology.

---

## ALIGN (task, no code change)

### Proposal 10 — 002/FR-026 (BD-4) — per-device summary format defect

Direction: **ALIGN** (genuine, cosmetic code bug found inside unspecced feature #3).

- Code: `apps/iops-benchmark/src/main.rs:423` format string
  `"\nDevice {} ({}: {:.0} IOPS, {:.1} MB/s"` has an unbalanced `(` — the PCI address is
  never closed, so lines render as `Device 0 (0000:03:00.0: ...`.
- Required change: balance the parenthesis, e.g. `"\nDevice {} ({}): {:.0} IOPS, {:.1} MB/s"`.
- Filed as **Task BD-4** in `align-tasks.md`. No `.rs` modified in this pass.
