# Drift Report — block-device-filesys

**Generated**: pending

Read-only spec↔implementation drift analysis. Sources: `specs/001-block-device-filesys/spec.md` vs `src/{lib.rs,actor.rs,config.rs,telemetry.rs}` and `tests/integration.rs`.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 28 (FR-001..022, SC-001..006) |
| Aligned | 27 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced | 1 |

## Detailed Findings

### Spec 001-block-device-filesys — Block Device Filesys Component

#### Aligned ✓

- **FR-001** IBlockDevice implemented — `src/lib.rs:224`.
- **FR-002** ILogger receptacle; warn on io_uring fallback (`src/actor.rs:128`) and fsync-SQE-push failure (`src/actor.rs:603`); SQ-full surfaced as error `Completion` not logged (`src/actor.rs:469`); debug on connect (`src/actor.rs:930`), info on init (`src/lib.rs:130,180`) — receptacle at `src/lib.rs:59`.
- **FR-003** `define_component!` used — `src/lib.rs:54`.
- **FR-004** `config` module public; `create`/`initialize`/`shutdown` public — `src/lib.rs:82,113,187`; `src/config.rs:58,114`.
- **FR-005** regular-file backing store — `src/config.rs:114`.
- **FR-006** block_size/num_blocks via `create()`, pow2 & min 512 validated — `src/config.rs:58-71`.
- **FR-007** sync R/W pread/pwrite, O_DIRECT|O_SYNC + fdatasync, buffered fallback on EINVAL via `eprintln!` — `src/config.rs:173,180-185`; fdatasync `src/actor.rs:402`.
- **FR-008** async R/W via io_uring, write+fsync IO_LINK chain, sync fallback when ring absent — `src/actor.rs:574-583,505,631`.
- **FR-009** WriteZeros zero-fill + fdatasync — `src/actor.rs:680,737`.
- **FR-010** BatchSubmit sequential — `src/actor.rs:238-242`.
- **FR-011** AbortOp via io_uring AsyncCancel — `src/actor.rs:759`.
- **FR-012** NsProbe single namespace — `src/actor.rs:774`.
- **FR-013** actor model / io_uring loop — `src/actor.rs:925-957`.
- **FR-014** Criterion latency + throughput benches — `Cargo.toml` `[[bench]]`; `benches/{latency,throughput}.rs`.
- **FR-016** fallocate-if-absent, exact-size-open, size-mismatch error — `src/config.rs:114-166`.
- **FR-017** direct DmaBuffer slice access — `src/actor.rs` (`as_slice`/`as_mut_slice`, no intermediate copies).
- **FR-018** `io-uring` 0.7 dependency — `Cargo.toml`.
- **FR-019** feature-gated atomics TelemetryStats; latency now recorded from per-op `start.elapsed()` in ALL paths incl. async completion — `src/telemetry.rs:35`; `src/actor.rs:353,418,536,672,753,822-823`.
- **FR-020** non-blocking per-client FIFO backlog `pending` with `deliver`/`flush_pending` — `src/actor.rs:39,60,70,904`.
- **FR-021** device-info surface: numa_node -1, nvme_version "N/A (file-backed)", num_io_queues 1, max_transfer_size block_size*256, read_write_stats default — `src/lib.rs:288-306,340`.
- **FR-022** FlushSync → fdatasync; ns_id!=1 → InvalidNamespace w/o touching file; fdatasync failure → WriteFailed — `src/actor.rs:249-276`.
- **SC-001** read-after-write integrity + durability — tested `tests/integration.rs:85,424`.
- **SC-002** 100 concurrent ops capability — multi-block/multi-client tests `tests/integration.rs:365,424`.
- **SC-003** <1ms 4KB sync latency — design/benchmark target (not unit-asserted).
- **SC-004** tests pass without hardware/root, temp dir only — `tests/integration.rs` uses `tempfile`.
- **SC-005** Criterion CoV<15% — benchmark quality target.
- **SC-006** drop-in IBlockDevice replacement — interface parity with spdk-nvme confirmed.

#### Drifted ⚠️

- **FR-015** — *minor*.
  - Spec text: "The `create()` constructor and `DeviceConfig::new` have runnable doc examples."
  - Actual: `DeviceConfig::new` has a runnable ```` ``` ```` example (`src/config.rs:42-57`), but the `create()` doc example is a ```` ```ignore ```` block (`src/lib.rs:77-81`), so it is NOT compiled/run by `cargo test`. Only one of the two claimed "runnable" examples is actually runnable.
  - Location: `src/lib.rs:77`.
  - Severity: minor (documentation-quality; the DeviceConfig example does exercise validation).

#### Not Implemented ✗

None.

## Unspecced Code

| Feature | Location | Suggested Spec |
|---|---|---|
| `set_file_path` / `set_block_size` / `set_num_blocks` internal setters (`pub(crate)`, `#[allow(dead_code)]`) | `src/lib.rs:95,101,107` | Low priority — internal/unused; either remove or note as reserved config mutators. Not public API, so arguably out of scope for FRs. |

## Recommendations

1. FR-015 (minor): either make the `create()` doc example runnable (drop `ignore`, or use a `tempfile`/`/dev/null`-safe example) or soften FR-015 to state `create()`'s example is illustrative-only (`ignore`) like the module and interface examples. This is the sole real divergence.
2. Consider removing the dead-code `pub(crate)` setters in `src/lib.rs` or documenting their intended role.
3. Otherwise the component is well-aligned; the 2026-08-07 telemetry-latency fix is present in every path including the async harvest path (`src/actor.rs:822-823`).
