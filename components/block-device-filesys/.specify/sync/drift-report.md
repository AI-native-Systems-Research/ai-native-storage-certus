# Drift Report: block-device-filesys

Generated: 2026-08-07T15:31:02Z

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (001-block-device-filesys) |
| Requirements Checked | 26 (FR-001..FR-020, SC-001..SC-006) |
| Aligned | 25 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced | 4 |

Overall the implementation closely matches the spec. The spec was clearly kept in sync with the code (several requirements carry explicit "Backfilled from implementation" notes and FR-019 documents a known defect). One minor documentation drift found.

## Detailed Findings

### Aligned

- **FR-001** — implements `IBlockDevice` (`src/lib.rs:224`).
- **FR-002** — `logger: ILogger` receptacle (`src/lib.rs:59`); info at init (`src/lib.rs:130,180`), debug on connect (`src/lib.rs:237`), warn on io_uring ring fallback (`src/actor.rs:127`) and fsync-SQE push failure (`src/actor.rs:563`), SQ-full surfaced as error `Completion` not logged (`src/actor.rs:436,555`).
- **FR-003** — `define_component!` used (`src/lib.rs:54`).
- **FR-004** — `config` module public (`src/lib.rs:27`); `create`/`initialize`/`shutdown` public convenience methods (`src/lib.rs:82,113,187`).
- **FR-005** — regular file backing store via `open_or_create_backing_file` (`src/config.rs:114`).
- **FR-006** — `create(file_path, block_size, num_blocks)` (`src/lib.rs:82`); power-of-2 + min-512 enforced in `DeviceConfig::new` (`src/config.rs:59-64`).
- **FR-007** — ReadSync/WriteSync via pread/pwrite + fdatasync (`src/actor.rs:299,349,366`); O_DIRECT|O_SYNC open with EINVAL→buffered fallback and `eprintln!` warning (`src/config.rs:173,181`).
- **FR-008** — ReadAsync/WriteAsync via io_uring with timeout + OpHandle tracking; write chains IO_LINK write+DATASYNC fsync (`src/actor.rs:534-543`); sync fallback when ring unavailable (`src/actor.rs:468,591`).
- **FR-009** — WriteZeros with aligned zero buffer + fdatasync (`src/actor.rs:636`).
- **FR-010** — BatchSubmit executed sequentially (`src/actor.rs:237-241`).
- **FR-011** — AbortOp submits AsyncCancel + AbortAck (`src/actor.rs:711`).
- **FR-012** — NsProbe returns single namespace (`src/actor.rs:726`).
- **FR-013** — actor model with io_uring event loop in `on_idle` (`src/actor.rs:897`).
- **FR-014** — Criterion benches present (`benches/latency.rs`, `benches/throughput.rs`).
- **FR-016** — fallocate-if-absent / open-if-exists with size-mismatch error (`src/config.rs:118-166`).
- **FR-017** — DmaBuffer slices accessed directly (`src/actor.rs:296-306,345`).
- **FR-018** — depends on `io-uring` 0.7 (`Cargo.toml`).
- **FR-019** — feature-gated atomics-based `TelemetryStats` (`src/telemetry.rs:13`); `telemetry()` returns snapshot or `FeatureNotEnabled` (`src/lib.rs:308-335`). **Documented known defect**: latency always recorded as 0 (`record_op(0, ...)` at `src/actor.rs:320,381,496,629,705,774`) — spec explicitly acknowledges this in FR-019, so classified Aligned.
- **FR-020** — non-blocking completion delivery via per-client FIFO backlog `ClientSession::pending` / `flush_pending` (`src/actor.rs:39,60,70,855`).
- **SC-001** — read-after-write with fdatasync durability (`src/actor.rs:366`); covered by `tests/integration.rs`.
- **SC-002 / SC-003 / SC-005** — performance/concurrency outcomes; capability present, exercised by benches (not statically verifiable).
- **SC-004** — unit tests use `/tmp`/temp files, no hardware/root (`src/lib.rs:345`, `src/config.rs:199`, `tests/integration.rs`).
- **SC-006** — drop-in replacement: full `IBlockDevice`/`IBlockDeviceAdmin` surface implemented (`src/lib.rs:198,224`).

### Drifted

- **FR-015** (minor, `src/config.rs:106-113`) — Spec states "The `create()` constructor, `DeviceConfig`, and `open_or_create_backing_file` have doc examples." `create()` (`src/lib.rs:77`) and `DeviceConfig::new` (`src/config.rs:42`) have doc examples, but `open_or_create_backing_file` has only a prose doc comment with **no** example code block. Spec claim is inaccurate for that function.

### Not Implemented

- None.

## Unspecced Code

| Item | Location | Note |
|------|----------|------|
| `set_file_path` / `set_block_size` / `set_num_blocks` | `src/lib.rs:95,101,107` | `pub(crate)` setters marked `#[allow(dead_code)]`; not referenced in spec (internal/dead). |
| `read_write_stats()` returning zeroed `ReadWriteStats` | `src/lib.rs:340` | IBlockDevice method not mentioned by spec; returns default zeros. |
| Device-info methods `numa_node()`=-1, `nvme_version()`="N/A (file-backed)", `num_io_queues()`=1 | `src/lib.rs:288,300,304` | Not enumerated in spec's device-info scenarios (US2 lists sector_size/num_sectors/block_size/max_queue_depth/max_transfer_size only). |
| `max_transfer_size()` = `block_size * 256` | `src/lib.rs:292` | Specific multiplier not specified anywhere in spec. |

## Recommendations

1. FR-015: either add a runnable doc example to `open_or_create_backing_file`, or amend FR-015 text to reflect that it lacks one (like the interface-method carve-out already present).
2. Consider addressing the FR-019 telemetry-latency defect (record real per-op elapsed time) — tracked in `align-tasks.md`, but leaves min/max/mean latency permanently 0.
3. Optionally document the extra interface-required device-info methods (`numa_node`, `nvme_version`, `num_io_queues`, `read_write_stats`, `max_transfer_size` multiplier) in the spec for completeness.
