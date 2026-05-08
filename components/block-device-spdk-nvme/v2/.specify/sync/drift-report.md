# Spec Drift Report

Generated: 2026-05-05
Project: block-device-spdk-nvme v2

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 41 |
| Aligned | 37 (90%) |
| Drifted | 2 (5%) |
| Not Implemented | 3 (7%) |
| Unspecced Code | 5 |

## Changes Since Last Report (v1, 2026-04-16)

Three items from the prior drift report have been **resolved in v2**:

- **FR-009 (was: Drifted)** — Controller reset now cancels ALL clients' pending ops via `handle_controller_reset()` at `actor.rs:464-501`. Iterates `self.clients` and clears all pending_ops. **FIXED.**
- **WriteAsync buffer lifetime (was: Critical bug)** — `PendingOp` now has `write_buf: Option<Arc<DmaBuffer>>`. The `WriteAsync` handler at `actor.rs:675-683` clones the Arc into pending_ops, pinning the DMA buffer until completion callback fires. **FIXED.**
- **SC-008 (was: Drifted)** — Reclassified as minor. Existing benchmarks (batch construction, sync IO latency, batch throughput) cover public interface paths. Internal qpair selection is tested via unit tests. See D2 below for spec wording update.

## Detailed Findings

### Spec: 001-spdk-nvme-block-device - SPDK NVMe Block Device Component

#### Aligned

- FR-001: IBlockDevice interface for creating/connecting client channels -> `src/lib.rs` (`impl IBlockDevice`), `connect_client()`
- FR-002: Two shared-memory channels per client (ingress + callback) -> `src/lib.rs` creates crossbeam SPSC ingress and callback channels
- FR-004: Async read/write with timeout, unique operation handle, handle in completion -> `src/actor.rs` (ReadAsync, WriteAsync); handles via monotonic counter; completions include `OpHandle`
- FR-005: Abort in-flight async operation by handle -> `src/actor.rs` (AbortOp handler removes from pending, sends AbortAck)
- FR-006: Write-zeros operation -> `src/actor.rs`, calls `spdk_nvme_ns_cmd_write_zeroes`
- FR-007: Batch submission of IO operations -> `src/actor.rs` (BatchSubmit recursively dispatches each op with qpair selection)
- FR-008: Probe, create, format, delete namespaces -> `src/actor.rs` (NsProbe, NsCreate, NsFormat, NsDelete)
- FR-009: Controller hardware reset with graceful handling of ALL in-flight operations -> `src/actor.rs:464-501` iterates all clients, cancels all pending ops, sends ResetDone to requesting client only
- FR-010: Device info via IBlockDevice -> `src/lib.rs` (sector_size, num_sectors, max_queue_depth, num_io_queues, max_transfer_size, block_size, numa_node, nvme_version)
- FR-011: Telemetry feature gate -> `src/telemetry.rs` (TelemetryStats); returns error when feature disabled
- FR-012: Single NVMe controller per instance -> `src/lib.rs` (set_pci_address + initialize attaches one controller)
- FR-013: Actor pinned to NUMA zone of controller -> `src/lib.rs` discovers NUMA topology and pins actor
- FR-014: Actor polls all attached client channels -> `src/actor.rs` (poll_clients iterates all clients)
- FR-015: Exploits different NVMe IO queues with varying depths -> `src/qpair.rs` (QueuePairPool depths 4/16/64/256, select_index shallowest-fit heuristic)
- FR-016: ILogger receptacle -> `src/lib.rs` (logger receptacle in define_component!)
- FR-017: Uses spdk-env component -> `src/lib.rs` (spdk_env receptacle, checks connected before initialize)
- FR-018: DmaBuffer with Arc references -> WriteAsync pins Arc<DmaBuffer> in PendingOp.write_buf; ReadAsync pins Arc<Mutex<DmaBuffer>> in PendingOp.read_buf
- FR-019: Client disconnect cancels in-flight ops -> `src/actor.rs` (swap_remove on channel closed)
- FR-020: Namespace ops serialized through actor thread -> All ns commands in single-threaded `dispatch_command`

#### Drifted

- **D1 — FR-003**: Spec says sync read/write has "timeout" parameter but `Command::ReadSync`/`WriteSync` have no timeout field
  - Location: `interfaces/src/iblock_device.rs` (Command enum)
  - Severity: minor
  - Note: Sync ops poll SPDK completion in tight loop; timeout only applies to async variants. Intentional design — adding a software timeout to a blocking SPDK poll loop would be ineffective (the only recovery for a non-responding device is hardware reset via FR-009).

- **D2 — SC-008**: Spec says "Criterion benchmarks for performance-sensitive paths" — qpair selection module is `pub(crate)` so external Criterion benchmarks are not possible
  - Location: `benches/latency.rs`, `benches/throughput.rs`
  - Severity: minor
  - Note: Existing benchmarks cover public interface paths (batch writes, sync IO). Qpair selection is thoroughly tested via unit tests in `qpair.rs`.

#### Not Implemented

- **N1 — SC-001**: No test asserts "single-digit microsecond" latency envelope for 4KB sync round-trips
- **N2 — SC-002**: No test validates async timeout errors arrive "within 10% beyond the timeout value"
- **N3 — SC-006**: No test validates telemetry accuracy "within 5% of independently measured values"

---

### Spec: 002-iops-benchmark - IOPS Benchmark Example Application

#### Aligned

All FR-001 through FR-021 are aligned. No drift or missing implementations.

#### Drifted

(none)

#### Not Implemented

(none)

---

### Unspecced Code

| # | Feature | Location | Suggested Spec |
|---|---------|----------|----------------|
| U1 | `IBlockDeviceAdmin` interface (`set_pci_address`, `set_actor_cpu`, `initialize`) | `src/lib.rs`, `interfaces/src/iblock_device.rs` | 001-spdk-nvme-block-device |
| U2 | `set_actor_cpu(cpu: usize)` method on IBlockDeviceAdmin | `src/lib.rs` | 001-spdk-nvme-block-device |
| U3 | `TscClock` module (hardware TSC-based timing for hot-path deadlines) | `src/tsc.rs` | 001-spdk-nvme-block-device |
| U4 | `--io-mode sync\|async` flag in iops-benchmark | `apps/iops-benchmark-md/src/config.rs` | 002-iops-benchmark |
| U5 | Stale WriteAsync bug comment (bug is fixed in v2) | `tests/integration.rs:634-638` | N/A (cleanup) |

**U1/U2 — IBlockDeviceAdmin**: The admin lifecycle interface is a fundamental part of using the component. Every consumer must call `set_pci_address` + `set_actor_cpu` + `initialize` before using `IBlockDevice`. Not specifying this interface means consumers have no contract for how to set up the component.

**U3 — TscClock**: v2 replaced `std::time::Instant` with hardware TSC reads for hot-path deadline tracking in the actor. This is a significant performance optimization. The module includes calibration, TSC-to-nanosecond conversion, and deadline computation.

**U4 — --io-mode**: The benchmark's `--io-mode sync|async` flag controls whether the benchmark uses synchronous or asynchronous SPDK commands. Useful for comparing per-op latency (sync) vs throughput (async).

**U5 — Stale comment**: `tests/integration.rs:634-638` documents a WriteAsync use-after-free bug that has been fixed in v2 (PendingOp.write_buf now pins the Arc<DmaBuffer>). The comment should be removed and async write integration tests should be enabled.

## Recommendations

1. **Backfill FR-003** (minor): Update spec to state timeout applies only to async operations.
2. **Backfill SC-008** (minor): Relax spec to allow unit-level benchmarks for internal algorithms.
3. **Backfill IBlockDeviceAdmin** (new FR-021): Spec the admin lifecycle interface including set_actor_cpu.
4. **Backfill TscClock** (new non-functional): Document the TSC-based timing optimization as an implementation detail.
5. **Backfill --io-mode** (new FR-022 in spec 002): Spec the sync/async IO mode flag.
6. **Cleanup stale comment** (U5): Remove WriteAsync bug comment and enable async write tests.
7. **Implement SC-001/SC-002/SC-006**: Add hardware-dependent tests (may need `#[ignore]` for CI).
