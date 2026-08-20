# Drift Report — block-device-spdk-nvme

**Generated**: pending

Read-only spec↔implementation drift analysis. Two specs:
- `specs/001-spdk-nvme-block-device/spec.md` → `src/{lib.rs,actor.rs,qpair.rs,controller.rs,namespace.rs,command.rs,telemetry.rs,tsc.rs}`
- `specs/002-iops-benchmark/spec.md` → `apps/iops-benchmark/src/{main.rs,config.rs,worker.rs,stats.rs,report.rs,lba.rs}` (implementation lives in `apps/`, not this crate's `src/`).

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 2 |
| Requirements Checked | 73 (001: FR-001..030 + SC-001..008 = 38; 002: FR-001..026 incl 006a/006b + SC-001..007 = 35) |
| Aligned | 70 |
| Drifted | 3 |
| Not Implemented | 0 |
| Unspecced | 8 |

## Detailed Findings

### Spec 001-spdk-nvme-block-device — SPDK NVMe Block Device Component

#### Aligned ✓

- **FR-001** IBlockDevice create/connect channels — `src/lib.rs` (`connect_client`, ~`lib.rs:414-422`).
- **FR-002** ingress + callback SPSC channels per client — `src/lib.rs:51,414-422`.
- **FR-003** sync R/W — actor do_read/do_write sync paths.
- **FR-004** async R/W fire-and-forget; caller `tag` echoed in ReadDone/WriteDone from stored `PendingOp.tag` — `src/actor.rs:538-550,713-722,832-843`.
- **FR-005** abort defers AbortAck until real completion, keeps PendingOp+buffer alive, real `spdk_nvme_ctrlr_cmd_abort_ext` matched by `cmd_cb_arg`; unknown handle acked immediately — `src/actor.rs:972-1020,534-537,742-744,863-865`. (Code IMPLEMENTS the UAF-safe contract — see Drifted for the stale "drafted" spec wording.)
- **FR-006** write-zeros via `spdk_nvme_ns_cmd_write_zeroes` — `src/actor.rs:926-940,1262-1305`.
- **FR-007** BatchSubmit (all sub-ops onto one selected qpair) — `src/actor.rs:952-971`.
- **FR-008** namespace probe/create/format/delete incl. unallocated-capacity from tnvmcap/unvmcap — `src/actor.rs:1028-1080`; `src/namespace.rs:73-228`.
- **FR-009** controller reset cancels all pending, `spdk_nvme_ctrlr_reset` — `src/actor.rs:577-616,1081-1083`.
- **FR-011** telemetry min/max/mean + feature-off error — `src/telemetry.rs`; `src/lib.rs` telemetry().
- **FR-012** single controller via set_pci_address + initialize/attach — `src/lib.rs`.
- **FR-013** actor pinned to NUMA-local core (`src/lib.rs:229-245`); controller NUMA hardcoded 0 at probe (`src/lib.rs:334`) — matches the backfilled node-0 caveat.
- **FR-014** actor polls all client channels — `src/actor.rs` poll loop.
- **FR-015** qpair pool depths [4,16,64,256] (`src/qpair.rs:141`), capped by ctrlr max (`:162`), shallowest-with-capacity (`:261-265`), fallback most-available (`:274-279`), `io_queue_requests = depth*4` (`:173`).
- **FR-016** ILogger receptacle — `src/lib.rs`.
- **FR-017** spdk-env used for SPDK init — `src/lib.rs` (spdk-env dependency/init path).
- **FR-018** client DmaBuffer / Arc accepted in messages — `src/command.rs`, actor buffer handling.
- **FR-019** client disconnect cancels in-flight + discards completions — actor DisconnectClient path.
- **FR-020** namespace ops serialized through actor — `src/actor.rs` command dispatch.
- **FR-021** IBlockDeviceAdmin set_pci_address/set_actor_cpu/initialize/shutdown — `src/lib.rs:351-390`.
- **FR-022** TscClock calibrated once (`src/tsc.rs:43-49,79-102`); timeout throttled ~1ms (`src/actor.rs:1390-1394,271-272`).
- **FR-023** ContextPool slab allocator, acquire/release, cap 340 — `src/actor.rs:80-115,318-319,726-731,847-852`.
- **FR-024** reused `completion_scratch`/`timeout_scratch` (mem::swap / clear-and-reuse; lazy first-alloc then no hot-path alloc) — `src/actor.rs:262-267,508-510,562-568`.
- **FR-025** ENOMEM (rc=-12) retry loop up to `min(timeout_ms,1000ms)`, polling qpairs each iter — `src/actor.rs:35,746,752,755-783,867,871,874-902`.
- **FR-026** non-blocking per-client FIFO backlog `deliver`/`flush_pending`; Completion derives Clone — `src/command.rs:35-56`; `src/actor.rs:416-420`.
- **FR-027** signal_stop + detach_controller (explicit spdk_nvme_detach for Arc-cycle release) — `src/lib.rs:351-390`.
- **FR-028** on_stop order drain → deliver Error{Aborted} → park (NOT park-first) — `src/actor.rs:1411-1448`.
- **FR-029** round-robin poll via rotating `poll_start_idx`; `MAX_COMMANDS_PER_CLIENT_PER_POLL = 64` — `src/actor.rs:408,422-430`.
- **FR-030** read_write_stats() per-direction ops/bytes/latency + size buckets — `src/telemetry.rs:150-162`; `src/lib.rs:520-541`.
- **SC-001..004, SC-006, SC-008** design/hardware/coverage criteria — satisfied by structure + `apps/iops-benchmark` coverage (SC-008).
- **SC-007** actor on NUMA-local core (node-0 caveat per FR-013) — `src/lib.rs:229-245`.
- (Assumption) crossbeam-channel fully removed from `Cargo.toml` and `src/`; production path uses component_core SpscChannel. Only stale doc references remain (`CLAUDE.md:39`, `info/FUNCTIONAL-DESIGN.md:49`, `specs/001-.../{tasks.md:198-199,plan.md:22}`).

#### Drifted ⚠️

- **FR-005** — *minor* (spec lags code).
  - Spec text: the buffer-lifetime/UAF fix "is drafted on branch `sync/spec-drift-sweep-20260807` and requires hardware validation — see align-tasks.md Task BD-1."
  - Actual: the defer-until-completion contract is fully IMPLEMENTED on this branch (op marked `aborting`, PendingOp+buffer retained, real abort issued, AbortAck deferred to completion, unknown handle acked immediately).
  - Location: `src/actor.rs:972-1020,534-537`.
  - Severity: minor — code is the safe/intended behavior; the spec understates status. Update FR-005 to mark implemented (pending hardware validation).

- **FR-010** — *minor* (inaccurate backfilled claim for one field).
  - Spec text: "`max_transfer_size` returns 131072 (128 KiB)" as a fixed constant.
  - Actual: `max_transfer_size` is auto-detected from the controller's MDTS via `spdk_nvme_ctrlr_get_max_xfer_size`, using 131072 only as a fallback when MDTS==0; the init log advertises the detected MDTS.
  - Location: `src/controller.rs:169-177`; `src/lib.rs:468-472,186-191`.
  - Severity: minor. (`nvme_version`="1.0.0" at `controller.rs:157-161` and `numa_node`=0 at `lib.rs:334` ARE genuinely hardcoded and match the spec claim — aligned.)

- **SC-005** — *minor* (same root as FR-010).
  - Spec text: "`nvme_version`, `max_transfer_size`, and `numa_id` are currently fixed constants."
  - Actual: `max_transfer_size` is MDTS-derived (hardware-consistent), not fixed; only `nvme_version` and `numa_id` are fixed.
  - Location: `src/controller.rs:169-177`; `src/lib.rs:468-472`.
  - Severity: minor.

#### Not Implemented ✗

None. All FR-001..030 and SC-001..008 are present.

---

### Spec 002-iops-benchmark — IOPS Benchmark Example Application

Implementation: `apps/iops-benchmark/src/` (external to this crate).

#### Aligned ✓

- **FR-001..006** CLI flags + defaults: `--op`=read (`config.rs:68`), `--block-size`=4096 (`:74`), `--queue-depth`=32 (`:85`), `--threads`=1 (`:88`), `--duration`=10 (`:92`), `--ns-id`=1 (`:96`).
- **FR-006a** `--pci-addr` (first device if omitted) — `config.rs:100-101`; `main.rs:65-91,531-555`.
- **FR-006b** `--pattern`=random default — `config.rs:108-109`.
- **FR-007** startup validation — `config.rs:124-174`; `main.rs:203-212`.
- **FR-008** clamp queue depth + warn — `config.rs:182-190`; `main.rs:214-219`.
- **FR-009** each thread connects via IBlockDevice — `main.rs`/`worker.rs` connect path.
- **FR-010** async pipeline kept full to queue depth — `worker.rs` run loop.
- **FR-011** rw 50/50 via `rand::random::<bool>()` — `worker.rs:221-227`.
- **FR-012** config summary at startup — `report.rs`/`main.rs` header.
- **FR-013** per-second progress to stderr — `report.rs:37-62` (eprintln); `main.rs:342-376`.
- **FR-014** signal stop + collect after duration — `main.rs`.
- **FR-015** latency min/mean/p50/p99/max — `stats.rs:93-104,128-144`; `report.rs:127-132`.
- **FR-016** rw read/write IOPS separate — `stats.rs:22-34,56-77`; `report.rs:105-116`.
- **FR-017** random/sequential LBA, non-overlapping per-thread regions — `lba.rs:14-90`; `worker.rs:73-81`.
- **FR-018** IO errors counted, benchmark continues — `worker.rs`/`stats.rs`.
- **FR-019** exit 0 success / non-zero on failure — `main.rs`.
- **FR-020** `--quiet` — `config.rs:116-117`; `main.rs:342`.
- **FR-021** `--help` (clap) — `config.rs:65`.
- **FR-022** `--io-mode` sync/async default async — `config.rs:111-113`; `worker.rs:192-217`.
- **FR-023** `--block-size` comma list, per-IO random size — `config.rs:73-75`; `worker.rs:177-182`.
- **FR-024** `--batch-size` default 1 + per-op timing (each op own Instant, FIFO-popped) — `config.rs:80-81`; `worker.rs:146-173,189-190,240-241,255`.
- **FR-025** NUMA worker pinning + node CPU discovery — `main.rs:221-253,294-302,483-485`.
- **FR-026** `--device-count` default 1, multi-device select/init/distribute — `config.rs:104-105`; `main.rs:88-91,125-179,255-323`.
- **SC-001..005, SC-007** design/measurement criteria satisfied by implementation.
- **SC-006** stats from client-side completion timestamps; telemetry cross-check intentionally unwired (iops→block-device dependency has no `telemetry` feature) — matches backfilled SC-006 text — `apps/iops-benchmark/Cargo.toml`; `worker.rs:241,255`.

#### Drifted ⚠️

None material to the spec. (Note: `stats.rs:127` doc comment says "nearest-rank" but `percentile` at `stats.rs:135-143` interpolates — a code-internal doc/behavior mismatch, not spec drift.)

#### Not Implemented ✗

None. All FR-001..026 and SC-001..007 accounted for.

## Unspecced Code

| Feature | Location | Suggested Spec |
|---|---|---|
| `Command::FlushSync`/`FlushDone` + `do_sync_flush` (comment references extent-manager's FR-030, a different spec) | `src/actor.rs:941-951,1218-1260` | Backfill an FR in spec 001 for the FlushSync durability barrier. |
| Dead `probe()` free function superseded by `discover_namespaces` | `src/namespace.rs:20-47` | Remove dead code or note as internal helper. |
| Multi-device per-device summary block (has cosmetic unbalanced-`(` format bug) | `apps/iops-benchmark/src/main.rs:397-428` (bug at `:423`) | Backfill under FR-026 reporting; fix format string. |
| Barrier-based start sync (excludes init time from wall-clock) | `apps/iops-benchmark/src/main.rs:262,328-329`; `worker.rs:106` | Note in spec 002 as measurement methodology. |
| `throughput_gbps` reporting | `apps/iops-benchmark/src/stats.rs:38,83`; `report.rs:122-124` | Extend FR-015 output list. |
| Per-thread IOPS breakdown in final report | `apps/iops-benchmark/src/report.rs:74-103` | Extend FR-015/FR-016 reporting. |
| Batch send-failure rollback of in-flight entries | `apps/iops-benchmark/src/worker.rs:165-171` | Note under FR-024. |
| Parallel device init via `thread::scope` with distinct actor-CPU assignments; `[timing]` init eprintlns | `apps/iops-benchmark/src/main.rs:52-55,105-153` | Note under FR-026. |

## Recommendations

1. **FR-005 (minor)**: update spec text from "drafted" to "implemented (pending hardware validation)" — the defer-until-completion buffer-lifetime fix is fully present at `src/actor.rs:972-1020,534-537`.
2. **FR-010 / SC-005 (minor)**: correct the backfilled claim — `max_transfer_size` is MDTS-auto-detected (`src/controller.rs:169-177`) with 131072 as fallback, not a fixed constant. Only `nvme_version` (1.0.0) and `numa_id` (0) are genuinely hardcoded.
3. **Unspecced FlushSync** in spec 001: backfill an FR; the handler exists (`src/actor.rs:1218-1260`) but the SPDK component's own spec never mentions FlushSync.
4. Clean up stale crossbeam-channel doc references (dependency already removed from `Cargo.toml`/`src`).
5. Fix the cosmetic unbalanced-`(` in the iops per-device summary (`apps/iops-benchmark/src/main.rs:423`) and reconcile the `stats.rs:127` "nearest-rank" comment with the interpolated implementation.
6. Spec 002 is essentially fully aligned; remaining items are unspecced reporting extras worth backfilling under FR-015/FR-026.
