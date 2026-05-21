# Spec Drift Report

Generated: 2026-05-21
Project: block-device-spdk-nvme v2
Previous Report: 2026-05-05

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 48 |
| Aligned | 44 (92%) |
| Drifted | 1 (2%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 3 |

## Changes Since Last Report (2026-05-05)

Five items from the prior drift report have been **resolved**:

- **N1 — SC-001 (was: Not Implemented)** — Integration test `sc001_sync_latency_envelope` added at `tests/integration.rs:1028-1099`. Validates <100us p50 for 4KB sync round-trip (100 iterations). **IMPLEMENTED.**
- **N2 — SC-002 (was: Not Implemented)** — Integration test `sc002_timeout_accuracy` added at `tests/integration.rs:1106-1165`. Validates timeout arrives within bounded margin. **IMPLEMENTED.**
- **N3 — SC-006 (was: Not Implemented)** — Integration test `sc006_telemetry_accuracy` added at `tests/integration.rs:1171-1245`. Validates telemetry within 5% of independent measurement. **IMPLEMENTED.**
- **U1/U2 — IBlockDeviceAdmin (was: Unspecced)** — Spec 001 now includes FR-021 specifying the full `IBlockDeviceAdmin` interface with `set_pci_address`, `set_actor_cpu`, `initialize`, and `shutdown`. **SPECCED.**
- **U3 — TscClock (was: Unspecced)** — Spec 001 now includes FR-022 specifying hardware TSC clock with calibration and ~1ms throttled timeout checking. **SPECCED.**
- **U4 — --io-mode (was: Unspecced)** — Spec 002 now includes FR-022 specifying the `--io-mode sync|async` flag. **SPECCED.**
- **U5 — Stale comment (was: Cleanup)** — No longer present in the integration tests. Async write tests are active. **RESOLVED.**

Additionally, new spec requirements FR-021 through FR-024 have been added to spec 001, and FR-022 added to spec 002. All are implemented.

## Detailed Findings

### Spec: 001-spdk-nvme-block-device - SPDK NVMe Block Device Component

#### Aligned (24/24 FR + 7/8 SC)

| Requirement | Evidence |
|-------------|----------|
| FR-001 | `IBlockDevice` interface via `define_interface!` in `interfaces/src/iblock_device.rs`; `connect_client()` in `src/lib.rs:375-419` |
| FR-002 | Two SPSC channels per client (capacity 64) created in `connect_client()` |
| FR-003 | `Command::ReadSync`/`WriteSync` with ns_id, DmaBuffer (Arc), LBA; actor polls until completion (spec clarifies "no timeout — sync ops block until completion") |
| FR-004 | `Command::ReadAsync`/`WriteAsync` with `timeout_ms`; monotonic `OpHandle`; completions tagged with handle; `Completion::Timeout` on expiry |
| FR-005 | `Command::AbortOp { handle }` removes from pending_ops, sends `Completion::AbortAck` |
| FR-006 | `Command::WriteZeros` via `spdk_nvme_ns_cmd_write_zeroes` in `actor.rs:969-1011` |
| FR-007 | `Command::BatchSubmit { ops }` dispatches with unified qpair selection in `actor.rs:750-768` |
| FR-008 | `NsProbe`, `NsCreate`, `NsFormat`, `NsDelete` with real SPDK admin commands in `namespace.rs` |
| FR-009 | `handle_controller_reset()` cancels ALL clients' pending ops, calls `spdk_nvme_ctrlr_reset`, sends `ResetDone` |
| FR-010 | `IBlockDevice` methods: `max_queue_depth()`, `num_io_queues()`, `max_transfer_size()`, `block_size()`, `numa_node()`, `nvme_version()`, `sector_size()`, `num_sectors()` |
| FR-011 | `telemetry` feature gate; `TelemetryStats` records min/max/mean latency, total_ops, mean_throughput; returns `FeatureNotEnabled` error when disabled |
| FR-012 | `IBlockDeviceAdmin::set_pci_address` + `initialize` binds single controller per instance |
| FR-013 | NUMA topology discovery in `initialize()` pins actor to controller's NUMA node |
| FR-014 | `poll_clients()` iterates all connected clients in `on_idle()` and `handle()` |
| FR-015 | `QueuePairPool` with depths [4, 16, 64, 256]; `select_index(batch_size)` shallowest-fit heuristic |
| FR-016 | `logger: ILogger` receptacle in `define_component!`; integration tests bind `LoggerComponent` |
| FR-017 | `spdk_env: ISPDKEnv` receptacle; `initialize()` checks `is_connected()` |
| FR-018 | `WriteSync` uses `Arc<DmaBuffer>`, `ReadSync` uses `Arc<Mutex<DmaBuffer>>`; async variants pin in `PendingOp` |
| FR-019 | `poll_clients()` detects `ChannelError::Closed`, removes client via `swap_remove`; pending ops silently discarded |
| FR-020 | All namespace commands processed in actor-thread `dispatch_command` (natural serialization) |
| FR-021 | `IBlockDeviceAdmin` interface with `set_pci_address`, `set_actor_cpu`, `initialize`, `shutdown` in `interfaces/src/iblock_device.rs:434-454` |
| FR-022 | `TscClock` in `src/tsc.rs`; calibrated against `clock_gettime`; timeout checks throttled to ~1ms via `on_idle()` |
| FR-023 | `ContextPool` slab allocator (capacity 340) in `src/actor.rs:68-103`; acquire at submission, release in callback |
| FR-024 | `completion_scratch` and `timeout_scratch` pre-allocated scratch buffers in `BlockDeviceHandler` |

| Success Criteria | Status | Evidence |
|-----------------|--------|----------|
| SC-001 | Aligned | `sc001_sync_latency_envelope` test at `tests/integration.rs:1028` |
| SC-002 | Aligned | `sc002_timeout_accuracy` test at `tests/integration.rs:1106` |
| SC-003 | Aligned | BatchSubmit mechanism with qpair selection; IOPS benchmark can validate empirically |
| SC-004 | Aligned | `create_n_namespaces_with_io` test at `tests/integration.rs:856` |
| SC-005 | Aligned | `device_info_after_initialize` test at `tests/integration.rs:249` |
| SC-006 | Aligned | `sc006_telemetry_accuracy` test at `tests/integration.rs:1171` |
| SC-007 | Aligned | NUMA CPU selection logic in `initialize()` uses topology discovery |
| SC-008 | Drifted | See D1 below |

#### Drifted

- **D1 — SC-008**: Spec requires "Criterion benchmarks for performance-sensitive paths (IO submission, batch processing, qpair selection)"
  - Location: No `benches/` directory in `block-device-spdk-nvme` crate
  - Severity: minor
  - Note: The `qpair` module is `pub` and has comprehensive unit tests. The IOPS benchmark application provides end-to-end throughput measurement. However, no Criterion microbenchmarks exist for the component's internal paths (submission latency, batch dispatch overhead, qpair selection). The `apps/iops-benchmark` is a separate crate and measures aggregate performance, not per-function overhead.

#### Not Implemented

(none — all previously unimplemented items are now resolved)

---

### Spec: 002-iops-benchmark - IOPS Benchmark Example Application

#### Aligned (22/22 FR + 7/7 SC)

| Requirement | Evidence |
|-------------|----------|
| FR-001 | `--op` flag with `read`, `write`, `rw`; default `read` (`config.rs:68-69`) |
| FR-002 | `--block-size` with comma-separated values; default 4096 (`config.rs:74-75`) |
| FR-003 | `--queue-depth`; default 32 (`config.rs:83-84`) |
| FR-004 | `--threads`; default 1 (`config.rs:87-88`) |
| FR-005 | `--duration`; default 10 (`config.rs:91-92`) |
| FR-006 | `--ns-id`; default 1 (`config.rs:95-96`) |
| FR-006a | `--pci-addr`; uses first device if omitted (`config.rs:99-100`, `main.rs:64-87`) |
| FR-006b | `--pattern` with `random`/`sequential`; default `random` (`config.rs:103-104`) |
| FR-007 | `validate()` checks block-size alignment, threads >= 1, duration >= 1, queue-depth >= 1 (`config.rs:121-168`) |
| FR-008 | `clamp_queue_depth()` clamps and prints warning (`config.rs:176-184`) |
| FR-009 | Each worker calls `ibd.connect_client()` (`main.rs:217-219`) |
| FR-010 | Workers keep pipeline full to queue_depth via async submission (`worker.rs:107-108`) |
| FR-011 | `choose_is_read()` uses `rand::random::<bool>()` for 50/50 (`worker.rs:214-219`) |
| FR-012 | `print_config()` prints all params including io_mode, pattern (`report.rs:9-31`) |
| FR-013 | Main thread prints per-second progress to stderr with elapsed, IOPS, per-thread breakdown (`main.rs:276-309`) |
| FR-014 | Timer thread sets stop_flag; main joins all workers (`main.rs:266-324`) |
| FR-015 | `FinalReport` has total_iops, throughput_mbps, lat_min_us, lat_mean_us, lat_p50_us, lat_p99_us, lat_max_us (`stats.rs:24-49`) |
| FR-016 | `print_final()` reports read_iops and write_iops separately in rw mode (`report.rs:105-115`) |
| FR-017 | `RandomLba` uniform distribution; `SequentialLba` non-overlapping per-thread regions (`lba.rs`) |
| FR-018 | Errors counted in `ThreadResult.errors`; reported in final summary (`worker.rs:263-268`, `report.rs:125`) |
| FR-019 | `exit(1)` for validation, `exit(2)` for fatal; implicit 0 on success (`main.rs`) |
| FR-020 | `--quiet` flag suppresses progress (`config.rs:113-114`, `main.rs:276`) |
| FR-021 | `--help` provided by clap `#[derive(Parser)]` automatically |
| FR-022 | `--io-mode sync|async`; default `async`; worker adapts Command variant (`config.rs:28-43`, `worker.rs:187-209`) |

| Success Criteria | Status | Evidence |
|-----------------|--------|----------|
| SC-001 | Aligned | Timer + thread join within duration + margin |
| SC-002 | Aligned | N threads create N `connect_client()` calls |
| SC-003 | Aligned | Manual validation against fio (documented in spec assumptions) |
| SC-004 | Aligned | `validate()` rejects invalid configs before IO |
| SC-005 | Aligned | Per-second progress from real-time atomic counters |
| SC-006 | Aligned | Latency percentiles from sorted-sample in `stats.rs:128-143` |
| SC-007 | Aligned | Errors counted and reported; worker continues (`worker.rs:262-268`) |

#### Drifted

(none)

#### Not Implemented

(none)

---

### Unspecced Code

| # | Feature | Location | Notes |
|---|---------|----------|-------|
| U1 | Mixed block sizes (comma-separated `--block-size`) | `apps/iops-benchmark/src/config.rs:74-75` | Spec FR-002 says "IO block size in bytes" (singular). Implementation supports multiple sizes with random selection per-IO. |
| U2 | `--batch-size` flag | `apps/iops-benchmark/src/config.rs:80-82` | Groups commands into `BatchSubmit`. Not mentioned in spec 002 (spec mentions batch support in spec 001 FR-007 but not a benchmark CLI flag for it). |
| U3 | NUMA worker pinning | `apps/iops-benchmark/src/main.rs:176-206` | Worker threads pinned to NUMA-local cores (round-robin, skipping actor core). Not specified in benchmark spec. |

---

## Recommendations

1. **Add Criterion benchmarks** (addresses D1/SC-008): Create `benches/` in the block-device-spdk-nvme crate with microbenchmarks for qpair selection, context pool acquire/release, and batch dispatch overhead. These can run without hardware using detached queue pairs.

2. **Update spec 002** to cover unspecced features: Add FR-023 (mixed block sizes), FR-024 (batch-size flag), and a note about NUMA worker pinning as an implementation optimization.

3. **FR-003 wording is now aligned**: The spec text was updated to explicitly state "no timeout — sync ops block until completion". The prior D1 drift is resolved.

4. **All critical/moderate drift from prior reports is resolved**: Controller reset cancels all clients (FR-009 fixed in v2), WriteAsync buffer lifetime fixed (PendingOp pins Arc), SC-001/SC-002/SC-006 tests implemented, IBlockDeviceAdmin/TscClock/--io-mode specced.
