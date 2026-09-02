# Drift Resolution Proposals — block-device-spdk-nvme

Generated: 2026-09-02 (Spec-Sync re-sync)
Based on: `.specify/sync/drift-report.json` (generated 2026-09-02)

## Summary

| Resolution Type | Count |
|-----------------|-------|
| BACKFILL (spec → doc; drifted reqs) | 3 |
| ALIGN (task, no code change) | 1 |
| HUMAN_DECISION | 0 |

All 3 drifted requirements are spec-lag (working/intentional code, spec text/refs behind
code) → BACKFILL. One genuine code defect (telemetry test compile break) → ALIGN task
(no `.rs` edited). No conflicts requiring human adjudication.

---

## Drifted requirements (BACKFILL)

### Proposal 1 — 001/FR-030 — read_write_stats histograms + stale refs

Direction: **BACKFILL** (spec-lag; missing feature description + stale refs).

- Spec said: `read_write_stats()` returns per-direction byte/op/latency counters; impl at
  `iblock_device.rs:494`, `lib.rs:511`, `telemetry.rs:140`.
- Code does: `ReadWriteStats` also exposes per-transfer-size histograms
  (`read_size_buckets`/`write_size_buckets: [u64; IO_SIZE_BUCKETS]`, `IO_SIZE_BUCKETS = 25`,
  `components/interfaces/src/iblock_device.rs:139,159,161`) with a power-of-two bucketing
  helper `size_bucket()` (`:177`), `bucket_lower_bound()` (`:193`), and aggregation/derived
  accessors `merge_from()` (`:218`), `total_ops()` (`:232`), `total_bytes()` (`:237`),
  `mean_read_latency_ns()` (`:242`), `mean_write_latency_ns()` (`:251`). The interface
  method is at `iblock_device.rs:589`; the component impl at `src/lib.rs:525`; the collector
  fills buckets in `src/telemetry.rs:67-85` and snapshots them at `:150-162`.
- Resolution: extend FR-030 to describe the histograms and derived accessors, and correct
  the three cited impl line numbers. (Interface source is not edited — spec text only.)

### Proposal 2 — 001/FR-005 — abort dispatch line references

Direction: **BACKFILL** (spec-lag; stale refs only, behavior aligned).

- Spec said: abort dispatch at `src/actor.rs:972-1020`, deferred ack at `:528-537`.
- Code is at: AbortOp dispatch `src/actor.rs:999-1048`; deferred AbortAck `:559-576`.
- Resolution: update the two embedded references in FR-005. No behavioral wording change.

### Proposal 3 — 001/FR-031 — FlushSync line references

Direction: **BACKFILL** (spec-lag; stale refs only, behavior aligned).

- Spec said: FlushSync dispatch at `src/actor.rs:941-951`, `do_sync_flush` at `:1214-1260`.
- Code is at: dispatch `src/actor.rs:968-978`; `do_sync_flush` `:1249-1288`
  (`spdk_nvme_ns_cmd_flush` at `:1271`).
- Resolution: update the two embedded references in FR-031. No behavioral wording change.

---

## ALIGN (task, no code change)

### Proposal 4 — 001 telemetry tests (BD-5) — record() signature mismatch

Direction: **ALIGN** (genuine code defect; do not edit `.rs` in this sweep).

- Code: `src/telemetry.rs:218,230,231,232` call `stats.record(1000, 4096)` (2 args), but
  the current signature is `record(&self, latency_ns: u64, bytes: u64, is_read: bool)`
  (3 args, `src/telemetry.rs:67`). These calls are under `#[cfg(feature = "telemetry")]`.
- Impact: `cargo test -p block-device-spdk-nvme --features telemetry` fails to compile —
  the telemetry unit tests cannot build (constitution "Comprehensive Testing" mandate).
- Required change: pass the `is_read` argument to each call (e.g. `record(1000, 4096, true)`
  / `false`) and, where relevant, assert on the new read/write-split counters.
- Filed as **Task BD-5** in `align-tasks.md`. No `.rs` modified in this pass.
