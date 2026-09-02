---
spec_sync_component: block-device-spdk-nvme
spec_sync_timestamp: 2026-09-02T21:40:20Z
spec_sync_commit: 2fc1cd3c
spec_sync_inputs_sha256: f72a633473822f146e0e33cf8c986f049952d2c078eabb39a486e6d3874ff899
spec_sync_drift_status: drift
spec_sync_specs_analyzed: 2
spec_sync_requirements_checked: 75
spec_sync_aligned: 72
spec_sync_drifted: 3
spec_sync_not_implemented: 0
spec_sync_unspecced: 1
spec_sync_backfills_applied: 3
spec_sync_align_tasks_open: 4
spec_sync_align_tasks_new: 1
spec_sync_notes: >-
  drift_status=drift: BACKFILLs applied to spec 001 (FR-030 histograms + FR-005/FR-031
  stale refs), but ALIGN Task BD-5 (telemetry tests use old record() signature — breaks
  cargo test --features telemetry) remains open, as do pre-existing BD-2/BD-3/BD-4.
  Hash covers components/block-device-spdk-nvme/{src,specs} + components/interfaces/{src,specs};
  it does NOT cover apps/iops-benchmark/src (spec 002 code), so spec-002 drift is not stamp-detected.
---

# Drift Report — block-device-spdk-nvme

**Generated**: 2026-09-02T21:40:20Z

Read-only spec↔implementation drift analysis. Two specs:
- `specs/001-spdk-nvme-block-device/spec.md` → `src/{lib.rs,actor.rs,qpair.rs,controller.rs,namespace.rs,command.rs,telemetry.rs,tsc.rs}`
- `specs/002-iops-benchmark/spec.md` → `apps/iops-benchmark/src/{main.rs,config.rs,worker.rs,stats.rs,report.rs,lba.rs}` (implementation lives in `apps/`, not this crate's `src/`, and is therefore NOT covered by `spec-sync-hash.sh`).

This is a re-sync. The prior report (2026-08-26) predated the current specs: spec 001 has since grown to **FR-001..032** (the 2026-08-27 sync added FR-031 FlushSync and FR-032 DMA logging) and spec 002's reporting/init extras were backfilled (FR-015/FR-024/FR-026/SC-001). Those items are now specced and are recorded as Aligned below. This pass re-verifies every FR/SC against the current source and surfaces the drift that has appeared since.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 2 |
| Requirements Checked | 75 (001: FR-001..032 = 32 + SC-001..008 = 8 → 40; 002: FR-001..026 incl 006a/006b = 28 + SC-001..007 = 7 → 35) |
| Aligned | 72 |
| Drifted | 3 |
| Not Implemented | 0 |
| Unspecced | 1 |

Drift this pass is concentrated in spec 001's telemetry area: FR-030 does not describe the per-transfer-size histograms that now ship in `ReadWriteStats`, and three FRs (FR-005, FR-030, FR-031) carry stale `src/actor.rs`/interface line references because the code moved after the last sync. Separately, a genuine code defect (telemetry unit tests call `record()` with the old 2-arg signature) breaks `cargo test --features telemetry`; it is filed as an ALIGN task, not a spec edit.

## Detailed Findings

### Spec 001-spdk-nvme-block-device — SPDK NVMe Block Device Component

#### Aligned ✓

- **FR-001** IBlockDevice create/connect channels — `src/lib.rs` (`connect_client`).
- **FR-002** ingress + callback SPSC channels per client; `CLIENT_CHANNEL_CAPACITY=256` — `src/lib.rs:68`, connect path.
- **FR-003** sync R/W — actor sync read/write paths (`src/actor.rs:1189,1240`).
- **FR-004** async R/W fire-and-forget; caller `tag` echoed in ReadDone/WriteDone from stored `PendingOp.tag`.
- **FR-005** abort defers AbortAck until real completion, keeps PendingOp+buffer alive, real `spdk_nvme_ctrlr_cmd_abort_ext` matched by `cmd_cb_arg`; unknown handle acked immediately. Behavior fully implemented — `src/actor.rs:999-1048` (AbortOp dispatch), `src/actor.rs:559-576` (deferred ack). (Spec text is accurate; only its embedded line refs are stale — see Drifted.)
- **FR-006** write-zeros via `spdk_nvme_ns_cmd_write_zeroes`.
- **FR-007** BatchSubmit (all sub-ops onto one selected qpair).
- **FR-008** namespace probe/create/format/delete incl. unallocated-capacity from tnvmcap/unvmcap — `src/actor.rs`, `src/namespace.rs`.
- **FR-009** controller reset cancels all pending, `spdk_nvme_ctrlr_reset`.
- **FR-010** device-info: `max_transfer_size` MDTS-derived (`src/controller.rs:169-177`, fallback 131072 when MDTS==0); `nvme_version`=1.0.0 (`src/controller.rs:156-161`) and `numa_id`=0 (`src/lib.rs:334`) genuinely hardcoded — matches the current (re-synced) spec text.
- **FR-011** telemetry min/max/mean + feature-off error — `src/telemetry.rs`; `src/lib.rs` telemetry().
- **FR-012** single controller via set_pci_address + initialize/attach.
- **FR-013** actor pinned to NUMA-local core (`src/lib.rs:229-245`); controller NUMA hardcoded 0 at probe (`src/lib.rs:334`) — matches the backfilled node-0 caveat.
- **FR-014** actor polls all client channels — `src/actor.rs` poll loop.
- **FR-015** qpair pool depths [4,16,64,256] (`src/qpair.rs:141`), capped by ctrlr max (`:162`), shallowest-with-capacity (`:258-265`), fallback most-available (`:274-279`), `io_queue_requests = depth*4` (`:173`).
- **FR-016** ILogger receptacle — `src/lib.rs`.
- **FR-017** spdk-env used for SPDK init.
- **FR-018** client DmaBuffer / Arc accepted in messages.
- **FR-019** client disconnect cancels in-flight + discards completions.
- **FR-020** namespace ops serialized through actor.
- **FR-021** IBlockDeviceAdmin set_pci_address/set_actor_cpu/initialize/shutdown — `src/lib.rs:351-390`.
- **FR-022** TscClock calibrated once; timeout throttled ~1ms.
- **FR-023** ContextPool slab allocator, acquire/release.
- **FR-024** reused `completion_scratch`/`timeout_scratch` (mem::swap / clear-and-reuse).
- **FR-025** ENOMEM (rc=-12) retry loop up to `min(timeout_ms,1000ms)`, polling qpairs each iter — `src/actor.rs:35`, async submit paths.
- **FR-026** non-blocking per-client FIFO backlog `deliver`/`flush_pending` — `src/command.rs:35-56`.
- **FR-027** signal_stop + detach_controller (explicit spdk_nvme_detach for Arc-cycle release) — `src/lib.rs:351-390`.
- **FR-028** on_stop order drain → deliver Error{Aborted} → park (NOT park-first).
- **FR-029** round-robin poll via rotating start index; `MAX_COMMANDS_PER_CLIENT_PER_POLL = 64` — `src/actor.rs:433`, poll loop.
- **FR-032** debug-only DMA issue-size logging via `log_dma_issue!` macro — `src/actor.rs:50` (macro), invoked at `:825` (read-async), `:945` (write-async), `:1189` (read-sync), `:1240` (write-sync). All spec line refs CORRECT (last synced 2026-08-27).
- **SC-001..004, SC-006, SC-008** design/hardware/coverage criteria satisfied by structure + `apps/iops-benchmark` coverage.
- **SC-005** hardware-consistent device-info fields incl. MDTS-derived `max_transfer_size`; `nvme_version`/`numa_id` fixed per FR-010 — matches re-synced spec text.
- **SC-007** actor on NUMA-local core (node-0 caveat per FR-013) — `src/lib.rs:229-245`.

#### Drifted ⚠️

- **FR-030** — *moderate* (spec lags code: missing feature + stale refs).
  - Spec text: `read_write_stats()` returns per-direction byte/op/latency counters; impl cited at `iblock_device.rs:494`, `lib.rs:511`, `telemetry.rs:140`.
  - Actual: `ReadWriteStats` (in `components/interfaces/src/iblock_device.rs`) now ALSO carries per-transfer-size histograms — `read_size_buckets`/`write_size_buckets: [u64; IO_SIZE_BUCKETS]` with `IO_SIZE_BUCKETS = 25` (`iblock_device.rs:139,159,161`), a power-of-two bucketing helper `size_bucket()` (`:177`) and `bucket_lower_bound()` (`:193`), plus aggregation/derived accessors `merge_from()` (`:218`), `total_ops()` (`:232`), `total_bytes()` (`:237`), `mean_read_latency_ns()` (`:242`), `mean_write_latency_ns()` (`:251`). The interface method is `read_write_stats()` at `iblock_device.rs:589`; the component impl is `src/lib.rs:525` and the collector is `src/telemetry.rs:150-162`, which fills the buckets in `record()` (`src/telemetry.rs:67-85`). None of the histogram surface is described by FR-030, and the three cited impl line numbers are stale.
  - Location: `components/interfaces/src/iblock_device.rs:139-259,589`; `src/telemetry.rs:67-85,150-162`; `src/lib.rs:525`.
  - Severity: moderate — a shipped, public interface capability is undocumented. BACKFILL (describe histograms + correct refs).

- **FR-005** — *minor* (stale line references only; behavior aligned).
  - Spec text cites `src/actor.rs:972-1020` (abort dispatch) and `src/actor.rs:528-537` (deferred ack).
  - Actual: AbortOp dispatch is at `src/actor.rs:999-1048`; the deferred AbortAck on real completion is at `src/actor.rs:559-576`.
  - Severity: minor — evidence pointers drifted after code movement. BACKFILL (correct refs).

- **FR-031** — *minor* (stale line references only; behavior aligned).
  - Spec text cites `src/actor.rs:941-951` (dispatch) and `src/actor.rs:1214-1260` (`do_sync_flush`).
  - Actual: FlushSync dispatch is at `src/actor.rs:968-978`; `do_sync_flush` is at `src/actor.rs:1249-1288` (submit via `spdk_nvme_ns_cmd_flush` at `:1271`).
  - Severity: minor — evidence pointers drifted after code movement. BACKFILL (correct refs).

#### Not Implemented ✗

None. All FR-001..032 and SC-001..008 are present.

---

### Spec 002-iops-benchmark — IOPS Benchmark Example Application

Implementation: `apps/iops-benchmark/src/` (external to this crate; not covered by `spec-sync-hash.sh`).

#### Aligned ✓

- **FR-001..006** CLI flags + defaults: `--op`=read, `--block-size`=4096, `--queue-depth`=32, `--threads`=1, `--duration`=10, `--ns-id`=1 — `config.rs`.
- **FR-006a** `--pci-addr` (first device if omitted); **FR-006b** `--pattern`=random default — `config.rs`.
- **FR-007** startup validation; **FR-008** clamp queue depth + warn — `config.rs`, `main.rs`.
- **FR-009** each thread connects via IBlockDevice; **FR-010** async pipeline kept full to queue depth — `worker.rs`.
- **FR-011** rw 50/50 via `rand` — `worker.rs`.
- **FR-012** config summary at startup; **FR-013** per-second progress to stderr; **FR-014** signal stop + collect — `main.rs`, `report.rs`.
- **FR-015** latency min/mean/p50/p99/max + GB/s throughput (`stats.rs:38,83`; `report.rs:122-124`) + per-thread IOPS breakdown (`report.rs:74-103`) — matches backfilled text.
- **FR-016** rw read/write IOPS separate — `stats.rs`, `report.rs`.
- **FR-017** random/sequential LBA, non-overlapping per-thread regions — `lba.rs`, `worker.rs`.
- **FR-018** IO errors counted, benchmark continues; **FR-019** exit codes — `worker.rs`, `main.rs`.
- **FR-020** `--quiet`; **FR-021** `--help` (clap) — `config.rs`.
- **FR-022** `--io-mode` sync/async default async — `config.rs`, `worker.rs`.
- **FR-023** `--block-size` comma list, per-IO random size — `config.rs:73-75`, `worker.rs`.
- **FR-024** `--batch-size` default 1 + per-op timing + batch send-failure in-flight rollback — `worker.rs:158-172` (rollback). (Spec cites `worker.rs:158-171`; the rollback block actually spans `:158-172` — off-by-one, immaterial.)
- **FR-025** NUMA worker pinning + node CPU discovery — `main.rs:221-253,294-302`.
- **FR-026** `--device-count` default 1, parallel `thread::scope` init + `[timing]` output + `=== Per-Device Summary ===` block — `main.rs:52-55,105-153,397-428`. (Cosmetic format defect at `main.rs:423` remains — tracked as ALIGN Task BD-4.)
- **SC-001** barrier-based start sync excluding init time — `main.rs:262,328-329`; `worker.rs:106`.
- **SC-002..005, SC-007** measurement/robustness criteria satisfied by implementation.
- **SC-006** stats from client-side completion timestamps; telemetry cross-check intentionally unwired (iops→block-device dependency has no `telemetry` feature — `apps/iops-benchmark/Cargo.toml`) — matches backfilled SC-006 text; tracked as ALIGN Task BD-3.

#### Drifted ⚠️

None material to the spec. (Internal note, unchanged from prior sweeps: `stats.rs` percentile doc comment says "nearest-rank" but the implementation interpolates — a code-internal doc/behavior mismatch, not spec drift.)

#### Not Implemented ✗

None. All FR-001..026 and SC-001..007 accounted for.

## Unspecced Code

| Feature | Location | Suggested Spec |
|---|---|---|
| `ReadWriteStats` per-transfer-size histograms (`read_size_buckets`/`write_size_buckets`, `IO_SIZE_BUCKETS=25`) + helpers (`size_bucket`, `bucket_lower_bound`, `merge_from`, `total_ops`, `total_bytes`, `mean_read_latency_ns`, `mean_write_latency_ns`) | `components/interfaces/src/iblock_device.rs:139-259,589`; populated in `src/telemetry.rs:67-85,150-162` | Backfill into spec 001 FR-030 (describe the histograms + derived accessors). |

## Code Defects (not spec drift — ALIGN)

| Defect | Location | Impact |
|---|---|---|
| Telemetry unit tests call `TelemetryStats::record()` with the OLD 2-arg signature `record(1000, 4096)`; the current signature is `record(&self, latency_ns: u64, bytes: u64, is_read: bool)` (3 args, `src/telemetry.rs:67`). | `src/telemetry.rs:218,230,231,232` (all under `#[cfg(feature = "telemetry")]`) | `cargo test -p block-device-spdk-nvme --features telemetry` fails to compile — the telemetry test suite cannot build. Filed as ALIGN Task BD-5 (no `.rs` edited by this sweep). |

## Recommendations

1. **FR-030 (moderate) — BACKFILL**: document the `ReadWriteStats` per-transfer-size histograms and derived accessors, and correct the stale impl refs to `iblock_device.rs:589`, `lib.rs:525`, `telemetry.rs:150`.
2. **FR-005 / FR-031 (minor) — BACKFILL**: refresh the embedded `src/actor.rs` line references (FR-005 → `999-1048`/`559-576`; FR-031 → `968-978`/`1249-1288`).
3. **Telemetry test build (ALIGN Task BD-5)**: update the three `record(...)` test calls to the 3-arg signature so `--features telemetry` compiles again. No production code change required; this is a code fix (do not edit code in this sweep).
4. Spec 002 remains fully aligned; the only open items are the pre-existing ALIGN tasks BD-3 (telemetry cross-check) and BD-4 (cosmetic `main.rs:423` format string).
5. Note: `spec-sync-hash.sh` does not hash `apps/iops-benchmark/src`, so spec-002 code changes do not invalidate this report's stamp — spec 002 drift must be re-checked manually.
