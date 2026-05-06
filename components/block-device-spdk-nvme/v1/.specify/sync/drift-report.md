# Spec Drift Report

Generated: 2026-05-05
Project: block-device-spdk-nvme v1

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 41 |
| Aligned | 36 (88%) |
| Drifted | 2 (5%) |
| Not Implemented | 3 (7%) |
| Unspecced Code | 5 |

## Detailed Findings

### Spec: 001-spdk-nvme-block-device - SPDK NVMe Block Device Component

#### Aligned

- FR-001: IBlockDevice interface for creating/connecting client channels -> `src/lib.rs:334` (`impl IBlockDevice`), `connect_client()` at line 340
- FR-002: Two shared-memory channels per client (ingress + callback) -> `src/lib.rs:352-358` creates SPSC ingress and callback channels
- FR-003 (partial): Sync read/write with ns_id, DmaBuffer, LBA -> `src/actor.rs:339` (ReadSync), `src/actor.rs:364` (WriteSync)
- FR-004: Async read/write with timeout, unique operation handle, handle in completion -> `src/actor.rs:386` (ReadAsync), `src/actor.rs:462` (WriteAsync); handles via monotonic counter; completions include `OpHandle`
- FR-005: Abort in-flight async operation by handle -> `src/actor.rs:568` (AbortOp handler removes from pending, sends AbortAck)
- FR-006: Write-zeros operation -> `src/actor.rs:536`, calls `spdk_nvme_ns_cmd_write_zeroes`
- FR-007: Batch submission of IO operations -> `src/actor.rs:551` (BatchSubmit recursively dispatches each op with qpair selection)
- FR-008: Probe, create, format, delete namespaces -> `src/actor.rs:574-626` (NsProbe, NsCreate, NsFormat, NsDelete)
- FR-010: Device info via IBlockDevice -> `src/lib.rs:383-427` (sector_size, num_sectors, max_queue_depth, num_io_queues, max_transfer_size, block_size, numa_node, nvme_version)
- FR-011: Telemetry feature gate -> `src/telemetry.rs:15-110` (TelemetryStats); `src/telemetry.rs:122` returns error when feature disabled
- FR-012: Single NVMe controller per instance -> `src/lib.rs:140` (set_pci_address), `src/lib.rs:152` (initialize attaches one controller)
- FR-013: Actor pinned to NUMA zone of controller -> `src/lib.rs:209-221` discovers NUMA topology and pins actor
- FR-014: Actor polls all attached client channels -> `src/actor.rs:225` (poll_clients iterates all clients)
- FR-015: Exploits different NVMe IO queues with varying depths -> `src/qpair.rs:132` (QueuePairPool depths 4/16/64/256), `src/qpair.rs:266` (select_index heuristic)
- FR-016: ILogger receptacle -> `src/lib.rs:79` (logger receptacle in define_component!)
- FR-017: Uses spdk-env component -> `src/lib.rs:80` (spdk_env receptacle), `src/lib.rs:153` checks connected
- FR-018: DmaBuffer with Arc references -> `interfaces/src/iblock_device.rs:193` (Arc<Mutex<DmaBuffer>> reads), `:202` (Arc<DmaBuffer> writes)
- FR-019: Client disconnect cancels in-flight ops -> `src/actor.rs:256-258` (swap_remove on channel closed)
- FR-020: Namespace ops serialized through actor thread -> All ns commands in single-threaded `dispatch_command`

#### Drifted

- FR-003: Spec says sync read/write has "timeout" parameter but `Command::ReadSync`/`WriteSync` have no timeout field
  - Location: `interfaces/src/iblock_device.rs:187-203`
  - Severity: minor
  - Note: Sync ops block until SPDK completion callback; timeout only on async variants. Spec wording is ambiguous.

- SC-008: Spec requires "Criterion benchmarks for performance-sensitive paths" — qpair selection benchmarks were removed when modules were made `pub(crate)`
  - Location: `benches/latency.rs`, `benches/throughput.rs`
  - Severity: minor
  - Note: Batch construction, sync IO latency, and batch write throughput benchmarks remain. Internal benchmarks could be added as unit-level bench tests.

#### Not Implemented

- SC-001: No test asserts "single-digit microsecond" latency envelope for 4KB sync round-trips
- SC-002: No test validates async timeout errors arrive "within 10% beyond the timeout value"
- SC-006: No test validates telemetry accuracy "within 5% of independently measured values"

---

### Spec: 002-iops-benchmark - IOPS Benchmark Example Application

#### Aligned

- FR-001: `--op` flag with read/write/rw -> `config.rs:68`
- FR-002: `--block-size` flag, default 4096 -> `config.rs:72`
- FR-003: `--queue-depth` flag, default 32 -> `config.rs:76`
- FR-004: `--threads` flag, default 1 -> `config.rs:80`
- FR-005: `--duration` flag, default 10 -> `config.rs:84`
- FR-006: `--ns-id` flag, default 1 -> `config.rs:88`
- FR-006a: `--pci-addr` flag -> `config.rs:92`, `main.rs:72-95` (device selection)
- FR-006b: `--pattern` random/sequential -> `config.rs:96`
- FR-007: Validation at startup -> `config.rs:112-145` (block_size, threads, duration, queue_depth, namespace)
- FR-008: Queue depth clamping with warning -> `config.rs:148-156`
- FR-009: Each thread connects via IBlockDevice -> `main.rs:224`
- FR-010: Async pipeline kept full -> `worker.rs:92-116` (fills to queue_depth, re-submits on completion)
- FR-011: rw mode 50/50 random -> `worker.rs:163` (`rand::random::<bool>()`)
- FR-012: Config summary at startup -> `report.rs:9-23`
- FR-013: Per-second progress to stderr -> `main.rs:276-298`, `report.rs:28-44` (eprintln!)
- FR-014: Signal threads to stop, collect results -> `main.rs:269-314` (timer thread + join)
- FR-015: Final summary with IOPS, MB/s, latency percentiles -> `report.rs:47-112`
- FR-016: rw mode reports read/write IOPS separately -> `report.rs:87-98`
- FR-017: Random/sequential LBA patterns -> `lba.rs:12-85` (RandomLba uniform, SequentialLba non-overlapping)
- FR-018: Counts and reports IO errors -> `worker.rs:185,195,203,207`, `report.rs:104`
- FR-019: Exit 0 on success, non-zero on failure -> `main.rs:321` (exit(0)), various exit(1)/exit(2)
- FR-020: `--quiet` flag -> `config.rs:104`, `main.rs:276` (guards progress output)
- FR-021: `--help` flag -> provided by clap derive

#### Drifted

(none)

#### Not Implemented

(none)

---

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `--io-mode sync\|async` flag | `apps/iops-benchmark/src/config.rs:28-43` | 16 | 002-iops-benchmark |
| `IBlockDeviceAdmin` interface | `src/lib.rs:77`, `interfaces/src/iblock_device.rs:431-439` | 10 | 001-spdk-nvme-block-device |
| `--driver v1\|v2` flag | `apps/iops-benchmark/src/config.rs` | 15 | 002-iops-benchmark |
| `--batch-size` flag | `apps/iops-benchmark/src/config.rs` | 15 | 002-iops-benchmark |
| Multi-size `--block-size` | `apps/iops-benchmark/src/config.rs` | 20 | 002-iops-benchmark FR-002 |

**IoMode**: The benchmark has a `--io-mode sync|async` flag (default: async) not covered in spec 002. This extends beyond FR-010 which only mentions async commands. Useful feature but unspecced.

**IBlockDeviceAdmin**: The admin lifecycle interface (`set_pci_address`, `initialize`) is implemented, declared in `provides`, and used by the benchmark and integration tests, but is not mentioned in spec 001's requirements.

## Inter-Spec Conflicts

- **Stale WriteAsync bug comment**: `tests/integration.rs:648` claims a WriteAsync buffer lifetime bug, but current code at `src/actor.rs:546-575` correctly stores `write_buf = Some(buf.clone())`. The comment appears stale.

## Recommendations

1. **Backfill FR-003 timeout** (minor): Update spec to clarify timeout applies only to async operations. Sync ops block until SPDK completion by design.
2. **Remove stale test comment**: The WriteAsync bug comment in integration tests is outdated — the fix is already in place.
3. **Add IBlockDeviceAdmin to spec 001**: Document the admin lifecycle interface as a formal requirement (FR-021).
4. **Add `--io-mode` to spec 002**: Add FR-022 covering the sync/async IO submission mode flag.
5. **SC-001/002/006**: Hardware-dependent success criteria — consider marking as "verified during hardware integration" rather than requiring automated tests.
