# Spec Drift Report

Generated: 2026-06-18
Project: block-device-spdk-nvme v2
Previous Report: 2026-05-29

## Summary

| Spec | Aligned | Drifted | Not Implemented | Total FRs |
|------|---------|---------|-----------------|-----------|
| 001-spdk-nvme-block-device | 22 | 2 | 1 | 25 |
| 002-iops-benchmark | 25 | 0 | 0 | 25 |
| **Totals** | **47** | **2** | **1** | **50** |

Overall alignment: **94%**

---

## Spec 001: SPDK NVMe Block Device Component

**File**: `specs/001-spdk-nvme-block-device/spec.md`

### Aligned (22 of 25)

| FR | Summary | Evidence |
|----|---------|----------|
| FR-001 | IBlockDevice interface for creating/connecting client channels | `lib.rs:383` impl IBlockDevice with `connect_client()` |
| FR-002 | Two shared-memory SPSC channels per client (ingress + callback) | `lib.rs:405-413` creates SpscChannel<Command> and SpscChannel<Completion> |
| FR-003 | Synchronous read/write with ns_id, DmaBuffer, LBA | `actor.rs:577-625` Command::ReadSync/WriteSync with blocking poll |
| FR-004 | Async read/write with timeout, unique handle, handle in completion | `actor.rs:626-848` ReadAsync/WriteAsync; handle from next_handle counter; OpHandle in completions |
| FR-005 | Abort in-flight async op by handle | `actor.rs:884-889` Command::AbortOp removes from pending_ops, sends AbortAck |
| FR-006 | Write-zeros operation | `actor.rs:849-863` uses spdk_nvme_ns_cmd_write_zeroes |
| FR-007 | Batch submission of IO operations | `actor.rs:864-883` Command::BatchSubmit recursively dispatches with shared qpair |
| FR-008 | Namespace probe, create, format, delete | `actor.rs:890-951` + `namespace.rs` full CRUD via SPDK admin commands |
| FR-009 | Controller hardware reset with graceful in-flight handling | `actor.rs:519-557` cancels all pending, issues spdk_nvme_ctrlr_reset, sends ResetDone |
| FR-010 | Device info via IBlockDevice | `lib.rs:436-479` capacity, max_queue_depth, io_queue_count, max_transfer_size, block_size, numa_node, nvme_version |
| FR-011 | Feature-gated telemetry (min/max/mean latency, ops, throughput) | `telemetry.rs` TelemetryStats with atomic counters; returns error without feature |
| FR-012 | Single controller per instance via set_pci_address + initialize | `lib.rs:342-377` IBlockDeviceAdmin impl |
| FR-013 | Actor thread NUMA-pinned to controller node | `lib.rs:215-236` discovers NUMA topology, pins to local CPU |
| FR-014 | Actor polls all attached client channels | `actor.rs:370-497` poll_clients() iterates all ClientState entries |
| FR-016 | ILogger receptacle for debug logging | `lib.rs:79` receptacle declaration; used throughout |
| FR-017 | Uses spdk-env component for SPDK init | `lib.rs:79` ISPDKEnv receptacle; checked in initialize() |
| FR-018 | DmaBuffer via Arc in messages | Commands use Arc<Mutex<DmaBuffer>> for reads, Arc<DmaBuffer> for writes |
| FR-019 | Client disconnect: cancel ops, release resources, discard completions | `actor.rs:427-439` detects closed channel, removes via swap_remove; pending_ops HashMap dropped (releases all Arcs); completions discarded since channel is closed |
| FR-020 | Namespace ops serialized through actor thread | All NS commands dispatched in single-threaded actor; no locks needed |
| FR-021 | IBlockDeviceAdmin with set_pci_address, set_actor_cpu, initialize, shutdown | `lib.rs:342-381` full implementation |
| FR-022 | TscClock for timeout checking, calibrated at construction, ~1ms throttle | `tsc.rs` calibrates against clock_gettime; `actor.rs:1210-1213` throttles to 1ms |
| FR-024 | Pre-allocated scratch buffers (completion_scratch, timeout_scratch) | `actor.rs:232-237` both Vec fields reused via swap/clear pattern |

### Drifted (2 of 25)

| FR | Spec Says | Code Does | Severity | Recommendation |
|----|-----------|-----------|----------|----------------|
| FR-015 | `io_queue_requests` MUST be set to `depth * 2` | `qpair.rs:173` sets `depth * 4`. Code comment explains: "allows for request splitting (large IO -> multiple NVMe commands) and absorbs transient bursts under concurrent multi-client load before submit returns -ENOMEM." | **Minor** | Update spec to `depth * 4` with rationale. Code is deliberately better than spec. |
| FR-025 | Retry on ENOMEM for up to 50ms | `actor.rs:686-715` retries for `min(timeout_ms, 1000ms)` via `SUBMIT_ENOMEM_MAX_BACKPRESSURE_MS = 1000`. The 50ms fixed value was replaced by a dynamic cap that adapts to the operation's own timeout. | **Minor** | Update spec to reflect dynamic backpressure: `min(op.timeout_ms, 1000ms)`. The implementation is superior; the old 50ms was too short under heavy load. |

### Not Implemented (1 of 25)

| FR | Requirement | Status | Notes |
|----|-------------|--------|-------|
| FR-023 | ContextPool slab allocator eliminating per-IO heap allocation | **Partially implemented** | The ContextPool exists (`actor.rs:80-115`) with acquire/release semantics. However, `acquire()` allocates via `Box::new()` on pool miss (line 94). The pool starts empty and grows on demand rather than being pre-allocated at construction. Steady-state eliminates allocation (contexts are recycled), but first-use of each slot hits the allocator. The pre-allocated capacity (340) is only for the Vec's backing array, not the context objects themselves. |

---

## Spec 002: IOPS Benchmark Example Application

**File**: `specs/002-iops-benchmark/spec.md`

### Aligned (25 of 25)

| FR | Summary | Evidence |
|----|---------|----------|
| FR-001 | `--op` flag: read, write, rw; default read | `config.rs:68-69` clap ValueEnum with OpType |
| FR-002 | `--block-size` flag, default 4096 | `config.rs:74-75` with value_delimiter for multi-size |
| FR-003 | `--queue-depth` flag, default 32 | `config.rs:83-84` |
| FR-004 | `--threads` flag, default 1 | `config.rs:87-88` |
| FR-005 | `--duration` flag in seconds, default 10 | `config.rs:91-92` |
| FR-006 | `--ns-id` flag, default 1 | `config.rs:95-96` |
| FR-006a | `--pci-addr` PCI BDF address; first device if omitted | `config.rs:99-100` + `main.rs:64-87` |
| FR-006b | `--pattern` random/sequential, default random | `config.rs:103-104` |
| FR-007 | Validate params at startup, exit with clear error | `config.rs:121-168` validates alignment, ranges, namespace existence |
| FR-008 | Clamp queue depth to device max with warning | `config.rs:176-184` |
| FR-009 | Each thread connects via IBlockDevice own channels | `main.rs:217-218` per-thread connect_client() |
| FR-010 | Async ops at configured queue depth, pipeline full | `worker.rs:107-134` fills to queue_depth, refills in loop |
| FR-011 | rw mode: 50/50 random interleave | `worker.rs:221` rand::random::<bool>() |
| FR-012 | Print config summary at startup | `report.rs:9-31` includes io_mode, pattern, all params |
| FR-013 | Per-second progress to stderr unless --quiet | `main.rs:277-309` 1s loop; `config.quiet` check |
| FR-014 | Signal threads to stop after duration | `main.rs:268-271` timer sets AtomicBool |
| FR-015 | Final summary: IOPS, MB/s, latency (min, mean, p50, p99, max) us | `report.rs:65-133` + `stats.rs:51-123` |
| FR-016 | rw mode: separate read/write IOPS | `report.rs:105-116` |
| FR-017 | Random = uniform; Sequential = contiguous non-overlapping per thread | `lba.rs` RandomLba + SequentialLba with region partitioning |
| FR-018 | Count and report IO errors without aborting | `worker.rs:264-267` errors counted; `stats.rs:59` reported |
| FR-019 | Exit 0 success, non-zero on failure | `main.rs` exit(1) validation, exit(2) fatal, implicit 0 |
| FR-020 | `--quiet` suppresses progress | `config.rs:112-113` + `main.rs:277` |
| FR-021 | `--help` prints usage | Implicit via clap `#[derive(Parser)]` |
| FR-022 | `--io-mode` sync/async, default async | `config.rs:107-108` IoMode enum |
| FR-023 | `--block-size` comma-separated for mixed workloads | `config.rs:74` value_delimiter; `worker.rs:172-177` random index selection |
| FR-024 | `--batch-size` for BatchSubmit grouping, default 1 | `config.rs:80-81` + `worker.rs:141-167` |
| FR-025 | Workers pinned to NUMA-local cores | `main.rs:175-206` discovers topology, round-robin pins |

### Drifted (0 of 25)

No drift detected.

### Not Implemented (0 of 25)

All requirements are implemented.

---

## Unspecced Features

Code features with no corresponding FR requirement:

### block-device-spdk-nvme component (spec 001)

| # | Feature | Location | Impact |
|---|---------|----------|--------|
| 1 | `signal_stop()` on IBlockDeviceAdmin | `lib.rs:355-359` | Low — convenience method for non-blocking stop signal |
| 2 | `detach_controller()` on IBlockDeviceAdmin | `lib.rs:378-380` | Low — explicit controller release after shutdown |
| 3 | Fair client polling rotation (`poll_start_idx`) | `actor.rs:244,376-382` | Medium — prevents HOL blocking; architecturally significant |
| 4 | Per-client command throttle (MAX_COMMANDS_PER_CLIENT_PER_POLL = 4) | `actor.rs:371` | Medium — fairness under multi-client load |
| 5 | Graceful drain on actor stop (5s deadline) | `actor.rs:1230-1247` | Medium — prevents use-after-free of completion buffers |
| 6 | Controller parking for safe detach ordering | `actor.rs:1262-1264` | Low — implementation detail of shutdown sequencing |

### iops-benchmark (spec 002)

| # | Feature | Location | Impact |
|---|---------|----------|--------|
| 7 | Per-thread IOPS breakdown in progress output | `report.rs:43-54` | Low — enhanced observability |
| 8 | Per-thread IOPS in final report | `report.rs:74-103` | Low — enhanced observability |
| 9 | Byte-level throughput counter per thread | `main.rs:224-225` + `worker.rs:242-243` | Low — enables MB/s progress display |

---

## Recommendations

### Spec Updates (Priority: High)

1. **FR-015**: Update to `depth * 4` with rationale for request-splitting headroom and burst absorption.
2. **FR-025**: Update fixed 50ms to dynamic `min(op.timeout_ms, SUBMIT_ENOMEM_MAX_BACKPRESSURE_MS)` with `SUBMIT_ENOMEM_MAX_BACKPRESSURE_MS = 1000ms`.

### Code Improvements (Priority: Medium)

3. **FR-023**: Consider pre-allocating 340 context objects at actor construction instead of growing on demand. This would make the "eliminating per-IO heap allocation" claim fully accurate.

### Spec Additions (Priority: Low)

4. Spec `signal_stop()` and `detach_controller()` in FR-021 or a new FR.
5. Spec the fair polling rotation and per-client throttle as a new FR (e.g., FR-026).
6. Spec the graceful drain behavior during actor shutdown.
