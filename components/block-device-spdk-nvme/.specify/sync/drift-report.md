# Spec Drift Report

Generated: 2026-07-22T22:46:53Z
Project: block-device-spdk-nvme
Previous Report: 2026-06-18 (previous 2026-05-29)

## Summary

| Spec | Aligned | Drifted | Not Implemented | Total FRs |
|------|---------|---------|-----------------|-----------|
| 001-spdk-nvme-block-device | 23 | 3 | 0 | 26 |
| 002-iops-benchmark | 27 | 0 | 0 | 27 |
| **Totals** | **50** | **3** | **0** | **53** |

Overall alignment: **94%** (50/53)

Note: this run supersedes the 2026-06-18 report. All three items flagged there (FR-015, FR-025, FR-023) were subsequently fixed via a sync-apply cycle on 2026-07-21 and are now spec-accurate (spec.md was patched directly from code, and FR-026 was added to document non-blocking completion delivery). This run identified **three new, previously unreported** drift items below.

---

## Spec 001: SPDK NVMe Block Device Component

**File**: `specs/001-spdk-nvme-block-device/spec.md`

### Aligned (23 of 26)

FR-001 through FR-009, FR-012, FR-014, FR-016 through FR-026 (excluding FR-010, FR-011, FR-013, which are drifted — see below) are aligned with the implementation, including the previously-fixed FR-015 (`depth * 4`, `src/qpair.rs:173`), FR-023 (steady-state ContextPool warmup, `src/actor.rs`), FR-025 (dynamic `min(op.timeout_ms, 1000ms)` ENOMEM retry cap, `src/actor.rs`), and FR-026 (non-blocking per-client completion delivery via `ClientSession::deliver`/`flush_pending`, `src/command.rs:31-56`).

### Drifted (3 of 26)

| Requirement | Spec Says | Actual | Location | Severity |
|---|---|---|---|---|
| FR-013 / SC-007 | Actor thread MUST be pinned to a core in the same NUMA zone as the controller; verified at instantiation. | `probe_controller()` **hardcodes the NUMA node to `0`** for every controller ("NUMA node is not available from minimal bindings; default to 0"). `set_actor_cpu()`/`initialize()` derive `target_cpu` from this always-0 value, so pinning is only correct by coincidence when the device happens to sit on NUMA node 0. On a multi-socket host with the NVMe device on a non-zero node, the actor is silently pinned to the *wrong* NUMA zone, with no error/warning raised. | `src/lib.rs:213-236,324-325` | **High** |
| FR-010 / SC-005 | Device info (incl. NVMe version, max transfer size) MUST be accurate/consistent with physical hardware. | `NvmeController::attach()` **hardcodes** `nvme_version = 1.0.0` and `max_transfer_size = 131072` (128KB) for every controller ("Default version and transfer size (not available from minimal bindings)"). Unlike `num_io_queues`/`max_queue_depth` a few lines above (read from real SPDK opts), these two fields never reflect the actual device's Identify Controller data. | `src/controller.rs:150-159` | Medium |
| FR-011 / SC-008 | Telemetry feature MUST collect stats; all public methods MUST have unit tests; SC-008 requires test coverage. | `TelemetryStats::record()` takes 3 args (`latency_ns, bytes, is_read`), but the `#[cfg(feature="telemetry")]` tests `stats_record_single_op`/`stats_record_multiple_ops` call `stats.record(1000, 4096)` with only 2 args. **Confirmed by compilation**: `cargo test -p block-device-spdk-nvme --features telemetry --no-run` produces 4x `error[E0061]: this method takes 3 arguments but 2 arguments were supplied`. The telemetry feature cannot be tested at all in its current state; default (non-telemetry) build/tests compile fine. | `src/telemetry.rs:206,218-220` (signature at `src/telemetry.rs:60`) | **High** |

### Not Implemented (0 of 26)

None.

---

## Spec 002: IOPS Benchmark Example Application

**File**: `specs/002-iops-benchmark/spec.md`

### Aligned (27 of 27)

All functional requirements FR-001 through FR-025 (including FR-006a, FR-006b) are implemented as specified:

| FR | Evidence |
|----|----------|
| FR-001–FR-006b | `apps/iops-benchmark/src/config.rs` clap flags (`--op`, `--block-size`, `--queue-depth`, `--threads`, `--duration`, `--ns-id`, `--pci-addr`, `--pattern`) |
| FR-007, FR-008 | `config.rs::validate()`, `config.rs::clamp_queue_depth()` |
| FR-009–FR-011, FR-017 | `apps/iops-benchmark/src/worker.rs` (`run()`, `submit_batch()`, `build_command()`, `drain_completions()`) |
| FR-012, FR-013, FR-015, FR-016 | `apps/iops-benchmark/src/report.rs` (`print_config()`, `print_progress()`, `print_final()`) |
| FR-014, FR-019, FR-020 | `apps/iops-benchmark/src/main.rs` stop-timer/exit-code/quiet handling |
| FR-018 | `worker.rs` error counting surfaced via `report.rs`/`stats.rs` |
| FR-021 | Implicit via clap `#[derive(Parser)]` |
| FR-022, FR-023, FR-024 | `config.rs` (io-mode, comma-separated block sizes, batch-size) + `worker.rs` |
| FR-025 | `main.rs` NUMA discovery + `set_thread_affinity()` for worker pinning (note: relies on `numa_node()`, which is affected by the FR-013 drift above) |

### Drifted (0 of 27)

None.

### Not Implemented (0 of 27)

None.

---

## Unspecced Code

| # | Feature | Location | Impact |
|---|---------|----------|--------|
| 1 | `signal_stop()` on `IBlockDeviceAdmin` — closes command channel without joining, for coordinated multi-actor shutdown | `components/interfaces/src/iblock_device.rs:517-524`, `src/lib.rs` | Low — well-documented rationale, but absent from FR-021 |
| 2 | `detach_controller()` on `IBlockDeviceAdmin` — explicit `spdk_nvme_detach` after shutdown, needed because the component leaks (Arc cycle) | `components/interfaces/src/iblock_device.rs:531-539`, `src/lib.rs` | Low — same as above |
| 3 | Fair client-polling rotation (`poll_start_idx` round-robin) | `src/actor.rs:244,392-400` | Medium — architecturally significant fairness mechanism |
| 4 | Per-client per-poll command cap (`MAX_COMMANDS_PER_CLIENT_PER_POLL = 64`) | `src/actor.rs:378` | Medium — directly affects per-client throughput fairness; value has changed over time (previously 4) with no spec record |
| 5 | Graceful drain on actor stop (5s deadline, delivers `Completion::Error{Aborted}` for stragglers) | `src/actor.rs:1248-1292` | Medium — safety-relevant shutdown behavior |
| 6 | Controller parking for safe detach ordering | `src/actor.rs:1248-1292`, `components/interfaces/src/iblock_device.rs:531-539` | Low — implementation detail of shutdown sequencing |
| 7 | Unused `crossbeam-channel` dependency | `Cargo.toml:21` | Low — contradicts spec's Assumptions section, which cites crossbeam as the channel impl; actual channels are `component_core::channel::spsc::SpscChannel` |
| 8 | Dead `ControlMessage::DisconnectClient` variant (matched but never sent) | `src/command.rs:65`, `src/actor.rs:1216` | Low — dead code; real disconnect path is `ChannelError::Closed` |
| 9 | `--device-count` multi-device benchmarking in iops-benchmark | `apps/iops-benchmark/src/config.rs:103-105,162-163`, `apps/iops-benchmark/src/main.rs:63-165,255` | Medium — substantial feature (concurrent multi-device attach + worker scaling) entirely absent from spec 002 |
| 10 | README channel-capacity documentation mismatch ("64 slots" vs. actual `CLIENT_CHANNEL_CAPACITY = 256`) | `README.md:40` vs `src/lib.rs:68` | Low — documentation drift, also echoed in the spec's Assumptions section |

---

## Recommendations

### High Priority

1. **Fix NUMA node discovery (FR-013/SC-007)**: `probe_controller()` must read the real NUMA node (e.g. via `/sys/bus/pci/devices/<bdf>/numa_node` or an equivalent SPDK/sysfs lookup) instead of hardcoding `0`. This is a correctness bug that silently defeats the component's core NUMA-locality guarantee on any non-node-0 device, and it also undermines the `iops-benchmark` worker-pinning logic (FR-025 in spec 002), which trusts `numa_node()` as ground truth.
2. **Fix telemetry test suite (FR-011/SC-008)**: update `stats_record_single_op`/`stats_record_multiple_ops` in `src/telemetry.rs` to pass the required third `is_read: bool` argument to `record()`. Currently `cargo test --features telemetry` does not compile at all.

### Medium Priority

3. **Populate real NVMe version / max transfer size (FR-010/SC-005)**: `NvmeController::attach()` should derive `version` and `max_transfer_size` from the controller's actual Identify Controller data (VER register / MDTS) rather than hardcoding `1.0.0` / 128KB, mirroring how `num_io_queues`/`max_queue_depth` are already read from real SPDK opts.

### Spec Additions (Low/Medium Priority)

4. Add `signal_stop()` and `detach_controller()` to FR-021 (or a new FR) documenting the signal-stop → shutdown → detach_controller lifecycle and the Arc-cycle-leak rationale already captured in the interface doc comments.
5. Add a new FR covering fair client-polling rotation and the per-client per-poll command cap, including the current cap value (64) — this value has silently changed before (previously 4) without any spec update.
6. Add a new FR covering the graceful drain / 5s deadline / controller-parking behavior on actor shutdown.
7. Add a new FR to spec 002 covering `--device-count` multi-device benchmarking, given its scope (worker-thread scaling across devices, aggregated reporting).

### Documentation Cleanup (Low Priority)

8. Reconcile the README's "Channel capacity is 64 slots" statement (and the spec's matching Assumptions-section wording) with the actual `CLIENT_CHANNEL_CAPACITY = 256` in `src/lib.rs`.
9. Remove the unused `crossbeam-channel` dependency from `Cargo.toml`, or update the spec's Assumptions section (which cites crossbeam) to instead reference the actual `SpscChannel` implementation from `component-core`.
10. Remove the dead `ControlMessage::DisconnectClient` variant, or wire it up if disconnect-by-control-message was intended as an alternative to the `ChannelError::Closed` detection path.
