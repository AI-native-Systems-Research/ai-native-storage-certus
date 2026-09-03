---
spec_sync_component: block-device-spdk-nvme
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T17:48:19Z
spec_sync_git_commit: a1b649f4
spec_sync_inputs_sha256: 02e17aab7111f30ad96043ce07100372e30529894b80b079812dc23460c38b22
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report — block-device-spdk-nvme

**Generated**: 2026-09-03
**Mode**: Spec↔implementation drift analysis with ALIGN + BACKFILL edits applied
to `specs/**` only. No `src/` or `interfaces/src/` code was changed this sweep.
Two specs:
- `specs/001-spdk-nvme-block-device/spec.md` → `src/{lib.rs,actor.rs,qpair.rs,controller.rs,namespace.rs,command.rs,telemetry.rs,tsc.rs}` + `components/interfaces/src/iblock_device.rs`
- `specs/002-iops-benchmark/spec.md` → `apps/iops-benchmark/src/{main.rs,config.rs,worker.rs,stats.rs,report.rs,lba.rs}` (implementation lives in `apps/`, **outside** this crate's `src/` and therefore outside the CI input hash — see note below).

> **Correction of the prior artifact.** The previous report read
> "**Generated**: pending" and claimed **3 drifted / 8 unspecced** items
> (FR-005 "drafted", FR-010/SC-005 `max_transfer_size`, unspecced FlushSync,
> GB/s, per-thread breakdown, batch rollback, parallel init, barrier sync). That
> report was **stale**: it was generated against an older spec revision. The
> 2026-08-20 (spec 002) and 2026-08-27 (spec 001) syncs had **already backfilled
> every one of those items** into the current `spec.md` files, and the prior
> report's own `file:line` anchors had themselves drifted. This sweep re-verifies
> the current specs against the current code and finds the behavior fully
> aligned; the only remaining spec-level drift was **stale embedded line anchors**
> (fixed) plus **one unspecced interface cluster** (backfilled).

> **Note on the CI input hash.** `scripts/spec-sync-hash.sh components/block-device-spdk-nvme`
> hashes this crate's `src/**` + `specs/**` and the `components/interfaces`
> tree. Spec 002's implementation lives in `apps/iops-benchmark/src/**`, which is
> **not** in the hash scope, so the committed digest does not cover it; the spec
> 002 findings below were nonetheless verified by hand against that tree.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 2 |
| Requirements Checked (FR + SC) | 73 (001: FR-001..032 + SC-001..008; 002: FR-001..026 incl. 006a/006b + SC-001..007) |
| Aligned (behavior) | 73 |
| Drifted this sweep | 6 spec-anchor/backfill items → **all resolved via ALIGN/BACKFILL** |
| Not Implemented | 0 |
| Unspecced Features | 1 (backfilled this sweep) |
| Parked (documented, hardware-discovery / cosmetic) | 3 |

**Verification run this sweep** (grounds the SC claims): `cargo test -p
block-device-spdk-nvme --no-fail-fast` — **exit 0** (SPDK prebuilt at
`deps/spdk-build/`; tests use mocks, no hardware needed).

---

## Spec 001-spdk-nvme-block-device — SPDK NVMe Block Device Component

**Behavior: all 32 FRs + 8 SCs verified CONFIRMED against the current code.**
The only spec drift was stale embedded `file:line` anchors (code shifted since
the 2026-08-27 sync) plus one unspecced interface cluster. Both resolved.

### Resolved this sweep

- **FR-005 (ALIGN — stale anchors).** Behavior confirmed: `Command::AbortOp`
  marks the op `aborting`, retains the `PendingOp`+buffer, issues a real
  `spdk_nvme_ctrlr_cmd_abort_ext` matched by `cmd_cb_arg`, defers `AbortAck`
  until the real completion, and acks an unknown handle immediately. The spec's
  cited anchors had drifted: abort dispatch `972-1020` → **`src/actor.rs:999-1047`**;
  deferred ack `528-537` → **`src/actor.rs:543-576`** (the `pending.aborting`
  branch that emits `AbortAck` on the real completion). Spec anchors corrected.
- **FR-030 (ALIGN — stale anchors + BACKFILL — unspecced surface).** Behavior
  confirmed: `read_write_stats()` returns per-direction byte/op/latency counters.
  Anchors corrected: trait decl `iblock_device.rs:494` → **`:589`**; impl
  `lib.rs:511` → **`:525`**; backing accumulation `telemetry.rs:140` → **`:150`**.
  **Unspecced cluster backfilled:** `ReadWriteStats` also carries per-transfer-size
  IO histograms (`read_size_buckets`/`write_size_buckets`, `IO_SIZE_BUCKETS = 25`
  log2-spaced buckets via `size_bucket()`/`bucket_lower_bound()`) and a
  `merge_from()` dispatcher-wide aggregation helper
  (`components/interfaces/src/iblock_device.rs:139,155-161,177,193,218`) — none of
  which FR-030 previously mentioned. Backfilled into FR-030.
- **FR-031 (ALIGN — stale anchors).** Behavior confirmed: `Command::FlushSync
  { ns_id }` validates the namespace, issues `spdk_nvme_ns_cmd_flush`, delivers
  `Completion::FlushDone`, and surfaces a bad ns / non-zero submit rc as an error
  rather than crashing. Anchors corrected: dispatch `941-951` →
  **`src/actor.rs:968-978`**; `do_sync_flush` `1214-1260` → **`src/actor.rs:1249-1288`**.

### Verified accurate (no edit needed)

- **FR-010 / SC-005.** `max_transfer_size` MDTS-derived via
  `spdk_nvme_ctrlr_get_max_xfer_size` (`controller.rs:171`, within the cited
  `169-177`), 131072 fallback only when MDTS==0; `nvme_version` fixed at 1.0.0
  (`controller.rs:157`, within cited `156-161`); `numa_id` hardcoded 0
  (`lib.rs:333`). All cited anchors accurate. The two fixed fields
  (`nvme_version`, `numa_id`) remain **parked** pending hardware-discovery Task
  BD-2 (documented in spec + align-tasks.md) — not actionable drift.
- **FR-032.** `log_dma_issue!` macro `actor.rs:50`; invocations `:825`
  (read-async), `:945` (write-async), `:1189` (read-sync), `:1240`
  (write-sync) — **all five exact.** `#[cfg(debug_assertions)]`-gated,
  telemetry-independent.
- **FR-013 / SC-007.** Actor pinned NUMA-local (`lib.rs:229-245`); controller
  NUMA hardcoded 0 (`lib.rs:333-334`) — matches the node-0 caveat (Task BD-2).
- **FR-015.** Qpair pool depths `[4,16,64,256]` (`qpair.rs:141`), capped by ctrlr
  max (`:162-164`), shallowest-with-capacity (`:261-265`), fallback most-available
  (`:274-279`), `io_queue_requests = depth*4` (`:173`).
- **FR-021.** `IBlockDeviceAdmin::{set_pci_address,set_actor_cpu,initialize,shutdown}`
  (`iblock_device.rs:601,607,610,625`; impl `lib.rs:352-390`).
- **FR-022.** `TscClock` calibrated at construction (`tsc.rs:43-49,79-102`);
  timeout throttled ~1ms (`actor.rs:1418-1422`, field `:296-297`).
- **FR-023.** `ContextPool` slab, cap **340** (`actor.rs:344`), acquire/release
  (`:118-139`), acquire sites `:751`/`:873`.
- **FR-025.** ENOMEM (`-12`) retry to `min(timeout,1000)` via `clamp(1,1000)`,
  polling all qpairs each iter (`actor.rs:35`; read `:771-808`; write `:893-928`).
- **FR-026.** Non-blocking per-client FIFO backlog `deliver`/`flush_pending`
  (`command.rs:35-56`); `Completion` derives `Clone` (`iblock_device.rs:438`);
  flush-retry call site `actor.rs:441-445`.
- **FR-029.** Round-robin via rotating `poll_start_idx` (`actor.rs:447-455`);
  `MAX_COMMANDS_PER_CLIENT_PER_POLL = 64` (`actor.rs:433`, cap break `:463`).
- **Assumption (dead `probe()`)** — `namespace.rs:19-47`, `#[allow(dead_code)]`,
  off all live paths. Accurately documented.
- **crossbeam-channel** — fully removed from production path; only stale doc
  references remain (below threshold).

All 13 `Command` variants, 12 `Completion` variants, 11 `IBlockDevice` methods,
and 6 `IBlockDeviceAdmin` methods map to existing FRs (after the FR-030 backfill).

---

## Spec 002-iops-benchmark — IOPS Benchmark Example Application

Implementation: `apps/iops-benchmark/src/` (outside the CI hash scope).

**Behavior: all FRs + SCs verified CONFIRMED.** Two additive, previously
unspecced reporting behaviors were backfilled this sweep.

### Resolved this sweep

- **FR-013 (BACKFILL).** The per-second progress line also prints instantaneous
  throughput (MB/s) and, with >1 worker thread, a per-thread instantaneous-IOPS
  breakdown (`report.rs:37-62`, called from `main.rs:374`) — additive to the
  required elapsed-time + instantaneous-IOPS fields. Backfilled.
- **FR-012 (BACKFILL).** The startup config summary also shows the active IO mode
  (already required by FR-022) and, when `--batch-size > 1`, the batch size
  (`report.rs:25-27`); the multi-device path emits an `[info] assigning actor
  CPUs …` diagnostic to stderr (`main.rs:127-130`). Backfilled.

### Verified accurate (no edit needed)

- **FR-001..006** defaults: `--op`=read (`config.rs:68`), `--block-size`=4096
  (`:74`), `--queue-depth`=32 (`:84`), `--threads`=1 (`:88`), `--duration`=10
  (`:92`), `--ns-id`=1 (`:96`). **FR-006a** `--pci-addr` first-if-omitted
  (`config.rs:100-101`; `main.rs:87-91`). **FR-006b** `--pattern`=random
  (`config.rs:108`).
- **FR-007** validation (`config.rs:133-152`); **FR-008** clamp+warn
  (`config.rs:182-190`).
- **FR-011** rw 50/50 via `rand::random::<bool>()` (`worker.rs:225`);
  **FR-017** random/sequential non-overlapping per-thread regions
  (`lba.rs:38-78`; `worker.rs:73-81`).
- **FR-015** GB/s + per-thread breakdown (`stats.rs:38,83`; `report.rs:74-103,122`);
  latency min/mean/p50/p99/max present.
- **FR-022** `--io-mode`=async (`config.rs:112`; `worker.rs:192-217`);
  **FR-023** comma-list block sizes, per-IO random (`config.rs:74`; `worker.rs:177-182`);
  **FR-024** `--batch-size`=1 + send-failure rollback of in-flight entries
  (`worker.rs:165-171`).
- **FR-025** NUMA worker pinning (`main.rs:221-246`, affinity ~`296-302`,
  actor CPU `483-485`); **FR-026** `--device-count`=1, parallel init via
  `std::thread::scope` + `[timing]` lines + `=== Per-Device Summary ===`
  (`main.rs:52-55,132-153,397-428`).
- **SC-001** barrier start sync `Barrier::new(total_workers+1)`, `bench_start`
  before `wait()` (`main.rs:262,328-329`; `worker.rs:106`).
- **SC-006** stats from client-side completion timestamps; telemetry cross-check
  intentionally unwired — iops depends on `block-device-spdk-nvme` without the
  `telemetry` feature (`apps/iops-benchmark/Cargo.toml`). Matches backfilled text.

---

## Parked (documented; not actionable spec↔impl behavioral drift)

1. **`nvme_version` / `numa_id` hardcoded** (spec 001, FR-010/FR-013/SC-005/SC-007).
   Tracked as align-tasks.md **Task BD-2** (hardware discovery). Documented in the
   spec; behavior matches the documented caveat.
2. **iops per-device summary cosmetic format defect** — unbalanced `(` in the
   format string at `apps/iops-benchmark/src/main.rs:423` (confirmed still
   present this sweep). Tracked as an ALIGN task in
   `apps/iops-benchmark/.specify/sync/align-tasks.md`. Cosmetic output only; not a
   behavioral or spec discrepancy.
3. **iops telemetry cross-check unwired** (spec 002, SC-006). Tracked as
   align-tasks.md **Task BD-3**. Documented in the spec.

## Below-threshold notes (no change)

- **`stats.rs:127` "nearest-rank" doc comment** contradicts the interpolating
  `percentile` implementation (`stats.rs:135-143`). This is a **code-internal
  doc-comment** inconsistency, not spec↔impl drift — spec 002 Assumptions
  explicitly states the percentile algorithm "is an implementation detail." Worth
  fixing the comment when `stats.rs` is next touched.
- Stale `crossbeam-channel` references in `CLAUDE.md` / design docs (dependency
  already removed). Doc-only.

## Stamp rationale

`drift_status: clean`. All 73 FR+SC across both specs are behaviorally aligned
with the shipped code (independently re-verified this sweep, not carried over
from the stale prior report). The six spec-level drift items found — four stale
embedded line anchors (FR-005, FR-030, FR-031) and two additive-reporting gaps
plus one unspecced interface cluster (FR-013, FR-012, FR-030 histograms) — were
all resolved in-place via ALIGN/BACKFILL edits to `specs/**`. No `src/` or
`interfaces/src/` code was changed, so no test/clippy/doc/bench state changed
(`cargo test -p block-device-spdk-nvme` was green before and remains applicable).
The three parked items are documented hardware-discovery tasks (BD-2, BD-3) and
one cosmetic format defect — none is a spec↔implementation behavioral
contradiction. This is not a clean stamp over an unacknowledged mismatch; every
remaining gap is documented here and in the specs.
