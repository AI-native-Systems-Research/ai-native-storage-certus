# Spec-Sync Apply Report — block-device-spdk-nvme

Generated: 2026-09-02
Based on: `.specify/sync/drift-report.json` + `.specify/sync/proposals.json` (generated 2026-09-02)

## Counts

| Category | Count |
|---|---|
| BACKFILL applied (drifted reqs) | 3 |
| ALIGN tasks generated | 1 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

All 3 drifted requirements were spec-lag (working/intentional code with spec text or
embedded line references behind the code) and were backfilled into spec 001. The one
unspecced feature (`ReadWriteStats` per-transfer-size histograms) was folded into the
FR-030 backfill. One genuine code defect (telemetry tests using the old `record()`
signature) was filed as an ALIGN task; no `.rs` source was modified.

## Specs Updated

| Spec | Requirement | Change Type | Summary |
|---|---|---|---|
| 001 | FR-030 | BACKFILL | Documented `ReadWriteStats` per-transfer-size histograms (`read_size_buckets`/`write_size_buckets`, `IO_SIZE_BUCKETS=25`) + helpers (`size_bucket`, `bucket_lower_bound`, `merge_from`) and derived accessors (`total_ops`, `total_bytes`, `mean_read_latency_ns`, `mean_write_latency_ns`); corrected stale impl refs to `iblock_device.rs:139-259,589`, `lib.rs:525`, `telemetry.rs:67-85,150-162`. |
| 001 | FR-005 | BACKFILL | Corrected embedded refs: abort dispatch `src/actor.rs:999-1048`, deferred ack `:559-576`. |
| 001 | FR-031 | BACKFILL | Corrected embedded refs: FlushSync dispatch `src/actor.rs:968-978`, `do_sync_flush` `:1249-1288`, submit `:1271`. |

Metadata `Last Synced` line updated in `specs/001-spdk-nvme-block-device/spec.md`. Spec 002
was fully aligned this pass and was not edited.

## Align Tasks Generated

| ID | Spec/Req | Severity | Summary | Files |
|---|---|---|---|---|
| BD-5 | 001/FR-011,FR-030 | Medium | Telemetry unit tests call `TelemetryStats::record()` with the old 2-arg signature; current signature is 3-arg (`latency_ns, bytes, is_read`). `cargo test --features telemetry` fails to compile. | `src/telemetry.rs:218,230,231,232` |

(Appended to `align-tasks.md` under the 2026-09-02 sweep section, with acceptance criteria.)

## Unspecced Backfilled

| # | Feature | Location | Target |
|---|---|---|---|
| 1 | `ReadWriteStats` per-transfer-size histograms + helpers/accessors | `components/interfaces/src/iblock_device.rs:139-259,589`; `src/telemetry.rs:67-85,150-162` | 001/FR-030 |

## Resolved

None resolved in this pass.

## Backups

Pre-edit backup written before `spec.md` was modified:

- `.specify/sync/backups/20260902T213438Z/specs/001-spdk-nvme-block-device/spec.md`

## Scope compliance

- Edited only files under `components/block-device-spdk-nvme/.specify/sync/` and
  `components/block-device-spdk-nvme/specs/001-spdk-nvme-block-device/spec.md`.
- No `.rs` source modified; `components/interfaces/` NOT touched (the histogram surface it
  defines was documented in spec 001, not edited in the interface crate); `cargo` not run.
