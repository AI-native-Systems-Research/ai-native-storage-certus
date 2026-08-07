# Drift Report: block-device-kernel

Generated: 2026-08-07T15:31:02Z

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (001-block-device-kernel) |
| Requirements Checked | 34 (FR-001..FR-025, SC-001..SC-009) |
| Aligned | 32 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced | 3 |

This spec is a backfill from the implementation and matches the code very closely. The only substantive drift is that the feature-gated telemetry never records real latency (always 0), which contradicts FR-021 and SC-006 — and unlike the sibling `block-device-filesys` spec, this spec does not document it as a known defect. (NFR-001..NFR-008 were also spot-checked and are aligned.)

## Detailed Findings

### Aligned

- **FR-001** — implements `IBlockDevice` + `IBlockDeviceAdmin` (`src/lib.rs:216,198`).
- **FR-002** — `logger` receptacle (`src/lib.rs:59`); info at init (`src/lib.rs:117,172`), debug on connect/disconnect (`src/lib.rs:229`, `src/actor.rs:852,858`). Per the 2026-07-22 amendment, no `warn()` calls exist and none are required.
- **FR-003** — `define_component!` with correct provides/receptacles (`src/lib.rs:55-70`).
- **FR-004** — opens with `O_DIRECT | O_DSYNC` (`src/config.rs:168`).
- **FR-005** — io_uring is the sole IO mechanism, no pread/pwrite anywhere (`src/actor.rs`).
- **FR-006** — rejects non-block-devices via `stat`/`S_IFBLK` check (`src/config.rs:104,163`).
- **FR-007** — block size min-512/power-of-2 (`src/config.rs:34-39`); num_blocks=0 auto-detect via `BLKGETSIZE64` (`src/config.rs:41-50,116`).
- **FR-008** — ReadSync/WriteSync via SQE + `submit_and_wait(1)` (`src/actor.rs:306,367`).
- **FR-009** — ReadAsync/WriteAsync with timeout + inflight map (`src/actor.rs:383,463,448`).
- **FR-010** — WriteZeros via `posix_memalign` 512-aligned buffer, io_uring write, free after (`src/actor.rs:556,577,605`).
- **FR-011** — BatchSubmit via recursive `process_command` (`src/actor.rs:221-225`).
- **FR-012** — AbortOp submits AsyncCancel + AbortAck (`src/actor.rs:619`).
- **FR-013** — NsProbe returns single NamespaceInfo (`src/actor.rs:632`).
- **FR-014** — NotSupported for NsCreate/NsDelete/NsFormat/ControllerReset (`src/actor.rs:232-245`).
- **FR-015** — actor model; `ActorHandler` with `handle()` + `on_idle()` (`src/actor.rs:847,868`).
- **FR-016** — per-client SPSC channels capacity 64 (`src/lib.rs:50,232,237`).
- **FR-017** — LBA bounds validated with `checked_add` (`src/actor.rs:166-182`).
- **FR-018** — ns_id==1 validation in device queries + validate_lba (`src/lib.rs:259,268`; `src/actor.rs:167`).
- **FR-019** — `posix_fadvise(POSIX_FADV_DONTNEED)` on init (`src/config.rs:189`).
- **FR-020** — verifies O_DIRECT via `fcntl(F_GETFL)` (`src/config.rs:176-184`).
- **FR-021** — feature-gated telemetry exists (`src/telemetry.rs`), returns `FeatureNotEnabled` without feature (`src/lib.rs:322-327`). **Partially drifted — see Drifted below re: latency.**
- **FR-022** — Criterion benches present (`benches/latency.rs`, `benches/throughput.rs`).
- **FR-023** — Admin methods are no-ops (`src/lib.rs:199,201,207,213`).
- **FR-024** — graceful shutdown: `ControlMessage::Shutdown` → `on_idle()` returns false (`src/actor.rs:862,869`).
- **FR-025** — non-blocking delivery with unbounded per-client FIFO backlog, flush oldest-first (`src/actor.rs:38,59,69,815,826`).
- **SC-001** — read-after-write, durability via O_DSYNC (`src/actor.rs:367`; `tests/integration.rs`).
- **SC-002** — auto-detect via BLKGETSIZE64 (`src/config.rs:116`).
- **SC-003** — rejects bad paths/sizes/out-of-range LBAs (`src/config.rs:104`; `src/actor.rs:175`).
- **SC-004** — multi-client concurrent IO via actor-serialized loop (`src/actor.rs:830`).
- **SC-005** — async timeout produces `Completion::Timeout` (`src/actor.rs:793`).
- **SC-007** — Criterion benches present.
- **SC-008** — drop-in replacement: full IBlockDevice/IBlockDeviceAdmin surface (`src/lib.rs:198,216`).
- **SC-009** — unit tests use mocked paths (`src/lib.rs:337`, `src/config.rs:197`); hardware integration tests marked `#[ignore]` (`tests/integration.rs:55,64,90,138,196,250,278`).

### Drifted

- **FR-021** (moderate, `src/actor.rs:312,373,609,689,747` + `src/telemetry.rs:35`) — Spec requires telemetry to track "min/max/mean latency", but every `record_op` call passes a hardcoded `0` for latency, so `min/max/mean_latency_ns` are always 0. Op-count, byte-count, and throughput are tracked correctly. Unlike block-device-filesys FR-019, this spec does NOT document the defect.
- **SC-006** (moderate, `src/telemetry.rs:68`) — "Feature-gated telemetry produces accurate `TelemetrySnapshot` values" is false for the latency fields (always 0) due to the FR-021 defect above.

### Not Implemented

- None.

## Unspecced Code

| Item | Location | Note |
|------|----------|------|
| Async `tag` parameter ignored | `src/actor.rs:390,470` (`_tag`) | `ReadAsync`/`WriteAsync` carry a `tag`, but completions are always emitted with `tag: 0` (`src/actor.rs:319,380,697,703`). Spec never mentions tag propagation; sibling filesys component does propagate it. Latent inconsistency. |
| `read_write_stats()` returns zeroed `ReadWriteStats` | `src/lib.rs:332` | IBlockDevice method not mentioned by spec. |
| `numa_node()`=-1, `nvme_version()`="N/A (kernel block device)" | `src/lib.rs:292,296` | Not enumerated in spec's US2 device-info acceptance scenario (which lists sector_size/num_sectors/block_size/max_queue_depth/num_io_queues/max_transfer_size). |

## Recommendations

1. FR-021/SC-006: fix telemetry to record real per-op latency (capture start time and pass elapsed ns to `record_op`), OR amend FR-021/SC-006 to document the latency-always-0 limitation as done for filesys FR-019.
2. Decide whether async `tag` should be propagated to completions for parity with `block-device-filesys`; if intentionally dropped, note it in FR-009.
3. Optionally document the interface-required `read_write_stats`, `numa_node`, and `nvme_version` methods in the spec.
