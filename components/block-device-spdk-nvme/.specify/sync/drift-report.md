# Spec Drift Report

Generated: 2026-05-29
Project: block-device-spdk-nvme v2
Previous Report: 2026-05-21

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 51 |
| Aligned | 47 (92%) |
| Drifted | 2 (4%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 1 |

## Changes Since Last Report (2026-05-21)

Three items from the prior drift report have been **resolved**:

- **U1 — Mixed block sizes (was: Unspecced)** — Spec 002 now includes FR-023 covering comma-separated `--block-size` with random per-IO size selection. **SPECCED.**
- **U2 — `--batch-size` flag (was: Unspecced)** — Spec 002 now includes FR-024 specifying `--batch-size N` grouping into `BatchSubmit`. **SPECCED.**
- **U3 — NUMA worker pinning (was: Unspecced)** — Spec 002 now includes FR-025 specifying NUMA-local core assignment for worker threads. **SPECCED.**

Two new drift items have been identified:

- **D2 (new) — ENOMEM retry loop**: The actor now retries `spdk_nvme_ns_cmd_read`/`write` for up to 1ms when SPDK returns -12 (ENOMEM / queue full), polling all queue pairs inside the loop to drain completions. This behavior is not captured in any spec requirement.
- **D1 (carried) — SC-008**: No Criterion microbenchmarks in `benches/` for qpair selection, context pool, or batch dispatch (unchanged from prior report).

Additionally, the queue pair pool `STANDARD_DEPTHS [4, 16, 64, 256]` and the `select_index` shallowest-fit heuristic are exercised in the spec via FR-015 but the specific depth values and fallback behavior are not formally constrained.

---

## Detailed Findings

### Spec: 001-spdk-nvme-block-device — SPDK NVMe Block Device Component

#### Aligned (24/24 FR + 7/8 SC)

| Requirement | Evidence |
|-------------|----------|
| FR-001 | `IBlockDevice` interface via `define_interface!`; `connect_client()` in `src/lib.rs:375-419` |
| FR-002 | Two SPSC channels per client (capacity 64) in `connect_client()` |
| FR-003 | `Command::ReadSync`/`WriteSync` with ns_id, DmaBuffer (Arc), LBA; actor blocks until completion |
| FR-004 | `Command::ReadAsync`/`WriteAsync` with `timeout_ms`; monotonic `OpHandle`; `Completion::Timeout` on expiry |
| FR-005 | `Command::AbortOp { handle }` removes from `pending_ops`, sends `Completion::AbortAck` |
| FR-006 | `Command::WriteZeros` via `spdk_nvme_ns_cmd_write_zeroes` in `actor.rs:1008-1050` |
| FR-007 | `Command::BatchSubmit { ops }` dispatches with unified qpair selection in `actor.rs:789-808` |
| FR-008 | `NsProbe`, `NsCreate`, `NsFormat`, `NsDelete` with real SPDK admin commands in `namespace.rs` |
| FR-009 | `handle_controller_reset()` cancels ALL clients' pending ops, calls `spdk_nvme_ctrlr_reset`, sends `ResetDone` |
| FR-010 | `IBlockDevice` methods: `max_queue_depth()`, `num_io_queues()`, `max_transfer_size()`, `block_size()`, `numa_node()`, `nvme_version()`, `sector_size()`, `num_sectors()` |
| FR-011 | `telemetry` feature gate; `TelemetryStats` records min/max/mean latency, total_ops, mean_throughput; returns error when disabled |
| FR-012 | `IBlockDeviceAdmin::set_pci_address` + `initialize` binds single controller per instance |
| FR-013 | NUMA topology discovery in `initialize()` pins actor to controller's NUMA node |
| FR-014 | `poll_clients()` iterates all connected clients in `on_idle()` and `handle()` |
| FR-015 | `QueuePairPool` with `STANDARD_DEPTHS [4, 16, 64, 256]`; `select_index(batch_size)` shallowest-fit heuristic; see D3 for depth specifics |
| FR-016 | `logger: ILogger` receptacle in `define_component!`; integration tests bind `LoggerComponent` |
| FR-017 | `spdk_env: ISPDKEnv` receptacle; `initialize()` checks `is_connected()` |
| FR-018 | `WriteSync` uses `Arc<DmaBuffer>`, `ReadSync` uses `Arc<Mutex<DmaBuffer>>`; async variants pin in `PendingOp` |
| FR-019 | `poll_clients()` detects `ChannelError::Closed`, removes client via `swap_remove`; pending ops silently discarded |
| FR-020 | All namespace commands processed in actor-thread `dispatch_command` (natural serialization) |
| FR-021 | `IBlockDeviceAdmin` with `set_pci_address`, `set_actor_cpu`, `initialize`, `shutdown` |
| FR-022 | `TscClock` calibrated against `clock_gettime`; timeout checks throttled to ~1ms via `on_idle()` |
| FR-023 | `ContextPool` slab allocator (capacity 340 = sum of STANDARD_DEPTHS) in `src/actor.rs:68-103` |
| FR-024 | `completion_scratch` and `timeout_scratch` pre-allocated scratch buffers in `BlockDeviceHandler` |

| Success Criteria | Status | Evidence |
|-----------------|--------|----------|
| SC-001 | Aligned | `sc001_sync_latency_envelope` test at `tests/integration.rs:1028` |
| SC-002 | Aligned | `sc002_timeout_accuracy` test at `tests/integration.rs:1106` |
| SC-003 | Aligned | `BatchSubmit` mechanism with qpair selection; IOPS benchmark validates empirically |
| SC-004 | Aligned | `create_n_namespaces_with_io` test at `tests/integration.rs:856` |
| SC-005 | Aligned | `device_info_after_initialize` test at `tests/integration.rs:249` |
| SC-006 | Aligned | `sc006_telemetry_accuracy` test at `tests/integration.rs:1171` |
| SC-007 | Aligned | NUMA CPU selection logic in `initialize()` uses topology discovery |
| SC-008 | Drifted | See D1 below |

#### Drifted

**D1 — SC-008** (carried from 2026-05-21): Spec requires Criterion benchmarks for performance-sensitive paths.
- Location: No `benches/` directory in `block-device-spdk-nvme` crate
- Severity: minor
- Note: `QueuePairPool` is `pub` with unit tests. `apps/iops-benchmark` measures aggregate performance but not per-function overhead (context pool acquire/release, qpair selection under contention, batch dispatch).

**D2 — FR-004 / FR-015 (new): ENOMEM retry loop not specified**
- Location: `src/actor.rs:618-670` (ReadAsync), `src/actor.rs:718-773` (WriteAsync)
- Severity: moderate
- Spec text: FR-004 specifies async read/write submission and FR-015 specifies queue pair exploitation, but neither addresses the behavior when `spdk_nvme_ns_cmd_read`/`write` returns ENOMEM (-12, queue full).
- Actual implementation:
  - When `rc == ENOMEM`, the actor enters a spin-retry loop for up to **1ms** (measured via `TscClock`).
  - Inside the retry loop, all queue pairs are polled via `process_completions(0)` on every iteration to drain completions and free slots before re-submitting.
  - If the deadline expires with `rc` still non-zero, the context is reclaimed, the `pending_ops` entry is removed, and an error completion is sent to the client.
  - The 1ms retry window was chosen as approximately 10x the typical NVMe command latency (10–100µs per the inline comment).
- Gap: No requirement specifies (a) that ENOMEM triggers a retry rather than an immediate error, (b) the retry duration (1ms), (c) that all queue pairs are polled inside the retry loop, or (d) the error path when the retry deadline is exceeded.
- Suggested addition to spec 001: New FR-025 covering ENOMEM back-pressure handling.

**D3 — FR-015 (partial): Queue pair pool depth values and fallback not constrained**
- Location: `src/qpair.rs:141` (`STANDARD_DEPTHS`), `src/qpair.rs:256-265` (`select_index`)
- Severity: minor
- Spec text: FR-015 states "The component MUST exploit different NVMe IO queues with varying queue depths to minimize latency for a given batch size." The specific depth tiers and selection fallback are unspecified.
- Actual implementation:
  - `STANDARD_DEPTHS = [4, 16, 64, 256]` — four tiers (shallow for low-latency, deep for throughput).
  - `select_index` chooses the shallowest queue with `available() >= batch_size`; falls back to the deepest (last) queue when all queues are at capacity.
  - `io_queue_requests = depth * 2` to accommodate request splitting.
  - `CONTEXT_POOL_CAPACITY = 340` is the arithmetic sum of all standard depths (4+16+64+256).
- Gap: Depth values, the shallowest-fit selection algorithm, the fallback-to-deepest behavior, and the `2x` request pool sizing are all implementation decisions with no corresponding spec constraints. A change to these values would not constitute a spec violation under the current spec text.
- Severity: minor (existing behavior is correct and tested; gap is one of under-specification rather than conflict)

---

### Spec: 002-iops-benchmark — IOPS Benchmark Example Application

#### Aligned (25/25 FR + 7/7 SC)

| Requirement | Evidence |
|-------------|----------|
| FR-001 | `--op` flag with `read`, `write`, `rw`; default `read` |
| FR-002 | `--block-size` with comma-separated values; default 4096 |
| FR-003 | `--queue-depth`; default 32 |
| FR-004 | `--threads`; default 1 |
| FR-005 | `--duration`; default 10 |
| FR-006 | `--ns-id`; default 1 |
| FR-006a | `--pci-addr`; uses first device if omitted |
| FR-006b | `--pattern` with `random`/`sequential`; default `random` |
| FR-007 | `validate()` checks block-size alignment, threads >= 1, duration >= 1, queue-depth >= 1 |
| FR-008 | `clamp_queue_depth()` clamps and prints warning |
| FR-009 | Each worker calls `ibd.connect_client()` |
| FR-010 | Workers keep pipeline full to queue_depth via async submission |
| FR-011 | `choose_is_read()` uses `rand::random::<bool>()` for 50/50 |
| FR-012 | `print_config()` prints all params including io_mode, pattern |
| FR-013 | Main thread prints per-second progress to stderr |
| FR-014 | Timer thread sets stop_flag; main joins all workers |
| FR-015 | `FinalReport` has total_iops, throughput_mbps, lat_min_us, lat_mean_us, lat_p50_us, lat_p99_us, lat_max_us |
| FR-016 | `print_final()` reports read_iops and write_iops separately in rw mode |
| FR-017 | `RandomLba` uniform distribution; `SequentialLba` non-overlapping per-thread regions |
| FR-018 | Errors counted in `ThreadResult.errors`; reported in final summary |
| FR-019 | `exit(1)` for validation, `exit(2)` for fatal; implicit 0 on success |
| FR-020 | `--quiet` flag suppresses progress |
| FR-021 | `--help` provided by clap `#[derive(Parser)]` |
| FR-022 | `--io-mode sync|async`; default `async`; worker adapts Command variant |
| FR-023 | `--block-size` accepts comma-separated list; random per-IO selection from provided sizes |
| FR-024 | `--batch-size N` groups N commands into `BatchSubmit`; default 1 |
| FR-025 | Worker threads pinned to NUMA-local cores via OS thread affinity |

| Success Criteria | Status | Evidence |
|-----------------|--------|----------|
| SC-001 | Aligned | Timer + thread join within duration + margin |
| SC-002 | Aligned | N threads create N `connect_client()` calls |
| SC-003 | Aligned | Manual validation against fio (per spec assumptions) |
| SC-004 | Aligned | `validate()` rejects invalid configs before IO |
| SC-005 | Aligned | Per-second progress from real-time atomic counters |
| SC-006 | Aligned | Latency percentiles from sorted-sample in `stats.rs` |
| SC-007 | Aligned | Errors counted and reported; worker continues |

#### Drifted

(none)

#### Not Implemented

(none)

---

### Unspecced Code

| # | Feature | Location | Notes |
|---|---------|----------|-------|
| U1 | ENOMEM retry loop with 1ms deadline and all-qpair poll | `src/actor.rs:618-670`, `src/actor.rs:718-773` | See D2 above. Back-pressure / queue-full retry behavior is entirely unspecified in spec 001. Requires a new FR-025. |

---

## Recommendations

1. **Add FR-025 to spec 001** (addresses D2/U1): Specify ENOMEM back-pressure behavior for async IO submission. Suggested text:
   > FR-025: When `spdk_nvme_ns_cmd_read` or `spdk_nvme_ns_cmd_write` returns ENOMEM (-12), indicating the NVMe queue pair is temporarily full, the actor MUST retry the submission for up to 1ms (measured by TscClock). On each retry attempt, the actor MUST poll all queue pairs for completions to drain in-flight operations and free queue slots. If the 1ms deadline expires without a successful submission, the operation MUST fail immediately with an error completion to the client; the pending_ops entry and context pool slot MUST be reclaimed without leaking.

2. **Refine FR-015 in spec 001** (addresses D3): Add a note constraining the standard depth tiers and selection algorithm. Suggested addition:
   > The component allocates queue pairs at standard depths [4, 16, 64, 256] (skipping any depth that exceeds the controller's maximum). Queue pair selection for a batch of size N chooses the shallowest queue with at least N available slots; if no queue has sufficient capacity, the deepest queue is used as a fallback.

3. **Add Criterion benchmarks** (addresses D1/SC-008): Create `benches/` in the `block-device-spdk-nvme` crate with microbenchmarks for qpair selection, context pool acquire/release, and batch dispatch overhead. These can run without hardware using detached queue pairs.

4. **All three prior unspecced items are now resolved**: FR-023 (mixed block sizes), FR-024 (batch-size), and FR-025 (NUMA worker pinning) are now in spec 002.
