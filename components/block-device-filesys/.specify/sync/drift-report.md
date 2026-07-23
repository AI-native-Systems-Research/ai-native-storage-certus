# Spec Drift Report

Generated: 2026-07-22
Project: block-device-filesys
Spec: 001-block-device-filesys

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 26 |
| Aligned | 18 (69%) |
| Drifted | 5 (19%) |
| Not Implemented | 3 (12%) |
| Unspecced Code | 3 |

## Detailed Findings

### Spec: 001-block-device-filesys — Block Device Filesys Component

#### Aligned

- FR-001: `IBlockDevice` implemented — `src/lib.rs:224`
- FR-003: `define_component!` macro used to build the component; the component consumes the pre-existing `define_interface!`-defined `IBlockDevice`/`IBlockDeviceAdmin` traits rather than defining new ones — `src/lib.rs:54-70`
- FR-004: `create()`, `initialize()`, `shutdown()` are public convenience methods mirroring `IBlockDeviceAdmin`; `config` module (`DeviceConfig`, `open_or_create_backing_file`) is public per spec — `src/lib.rs:82-203`, `src/config.rs`
- FR-005: Uses a regular file on a Linux filesystem via `std::fs::OpenOptions` — `src/config.rs:114-197`
- FR-007: `ReadSync`/`WriteSync` via `pread`/`pwrite`, opened O_DIRECT|O_SYNC with fdatasync belt-and-suspenders, buffered-IO fallback on `EINVAL` — `src/actor.rs:265-385`, `src/config.rs:171-197`
- FR-008: `ReadAsync`/`WriteAsync` via io_uring, `IO_LINK` write+fdatasync chaining, timeout/`OpHandle` tracking, sync fallback when io_uring unavailable — `src/actor.rs:387-634`
- FR-009: `WriteZeros` writes zero-filled blocks with fdatasync — `src/actor.rs:636-709`
- FR-010: `BatchSubmit` executes ops sequentially via recursive `process_command` — `src/actor.rs:237-241`
- FR-011: `AbortOp` issues an io_uring `AsyncCancel` and removes the op from in-flight tracking — `src/actor.rs:711-724`
- FR-012: `NsProbe` returns a single namespace with configured geometry — `src/actor.rs:726-738`
- FR-013: Actor model (dedicated thread, `Actor`/`ActorHandler`) running an io_uring event loop in `on_idle()` — `src/actor.rs:876-908`, `src/lib.rs:168-172`
- FR-014: Criterion benchmarks present — `benches/latency.rs`, `benches/throughput.rs`, `Cargo.toml:19-24`
- FR-016: Backing file created via fallocate if absent, opened with size check if present, mismatch errors — `src/config.rs:114-167`
- FR-017: `DmaBuffer` byte slices accessed directly (`as_slice`/`as_mut_slice`), no intermediate copies — `src/actor.rs` throughout
- FR-018: Depends on `io-uring` crate 0.7 (tokio-rs/io-uring) — `Cargo.toml:16`
- SC-001: Read-after-write integrity verified across patterns/block sizes with durable writes — `tests/integration.rs` (`write_sync_read_sync_roundtrip`, `data_integrity_multi_block_patterns`)
- SC-004: Tests use only `tempfile::tempdir()`, no hardware/root required — `tests/integration.rs:34-51`
- SC-006: `IBlockDevice` fully, compiler-checked implemented for drop-in interchangeability — `src/lib.rs:224-343`

#### Drifted

- **FR-002** (logging levels for queue-full and disconnection events) — severity: moderate
  - Spec: "Queue-full and disconnection events are logged at warn level."
  - Actual: client disconnection is logged at **debug**, not warn (`src/actor.rs:887`). The two "io_uring submission queue full" paths (async read/write SQE push failure) don't log at all — they only return an error `Completion`. Only ring-creation-fallback and fsync-SQE-push-failure actually use warn.
  - Location: `src/actor.rs:432-444`, `src/actor.rs:548-567`, `src/actor.rs:885-889`

- **FR-006** (configurable block size, default 512 bytes) — severity: minor
  - Spec: "Component MUST support configurable block size (default 512 bytes)..."
  - Actual: `block_size` is a required, always-explicit constructor parameter with no default-value mechanism; `DeviceConfig::new` only enforces a *minimum* of 512, not a default.
  - Location: `src/lib.rs:82-91`, `src/config.rs:58-79`

- **FR-015** (doc examples on `create()`, `DeviceConfig`, `open_or_create_backing_file`) — severity: moderate
  - Spec: claims all three have doc examples.
  - Actual: `open_or_create_backing_file` has **no doc example** (prose only). `create()`'s example is fenced ` ```ignore ` so it is never compiled/run as a doctest — unlike `DeviceConfig::new`'s example, which is a real runnable doctest. Conflicts with the project-wide CLAUDE.md convention requiring runnable doc examples.
  - Location: `src/config.rs:106-114`, `src/lib.rs:74-81`

- **Edge Case: io_uring submission queue full** — severity: major
  - Spec: "The actor MUST back-pressure by waiting for completions before submitting new operations."
  - Actual: there is no backpressure/wait logic anywhere. On SQE push failure the actor immediately returns an error completion (`NvmeBlockError::NotInitialized("io_uring submission queue full")`) instead of waiting for completions and retrying.
  - Location: `src/actor.rs:430-444` (read), `src/actor.rs:546-561` (write)

- **Assumptions: telemetry feature parity with block-device-spdk-nvme** — severity: major
  - Spec: "The `telemetry` feature gate behavior matches block-device-spdk-nvme."
  - Actual: only the `FeatureNotEnabled` gating matches. The telemetry *data* does not: every call site passes a hard-coded latency of `0` to `record_op(0, bytes)` (both sync/async paths and `harvest_completions`), so `min/max/mean_latency_ns` in `TelemetrySnapshot` are always 0 regardless of real IO latency. `InflightOp.start_ns` is computed as `Instant::now().elapsed().as_nanos()` — `elapsed()` on a just-created `Instant` is ~0 — and this field is never read when a completion is harvested. By contrast, `block-device-spdk-nvme` measures real per-op latency via TSC timestamps (`components/block-device-spdk-nvme/src/actor.rs:613,636`). Filesys telemetry latency figures are therefore always zero/meaningless while appearing valid — this is the most significant drift found.
  - Location: `src/actor.rs:318-321,379-382,494-497,627-630` (record_op(0,...) calls), `src/actor.rs:101-104,462-466,585-589` (unused/buggy `start_ns`), `src/actor.rs:773-776` (harvest_completions)

#### Not Implemented

- **SC-002**: "100 concurrent operations per second from a single client without data corruption" — no test/benchmark asserts this throughput threshold; only functional correctness is exercised.
- **SC-003**: "IO latency for 4KB blocks ... under 1ms for synchronous operations" — no test/CI check enforces a latency bound; `benches/latency.rs` produces measurements but nothing asserts `<1ms`.
- **SC-005**: "Criterion benchmarks produce ... coefficient of variation under 15%" — no automated check parses/enforces Criterion's CV output; left to manual inspection.

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `TelemetryStats` latency/throughput tracking (min/max/mean latency, mean throughput, atomic counters) | `src/telemetry.rs:1-104` | 104 | Add an FR describing exact counters/units/semantics and require real op-timed latency (not a placeholder), mirroring block-device-spdk-nvme's contract. |
| O_DIRECT-unsupported fallback warning printed via `eprintln!` instead of `ILogger` | `src/config.rs:171-197` | 27 | Clarify in FR-002/FR-007 whether config-module functions (no logger access) may bypass `ILogger`, or require threading the warning through the actor/component. |
| Non-blocking per-client completion delivery with FIFO backlog (`ClientSession::deliver`/`flush_pending`) to avoid cross-client head-of-line blocking | `src/actor.rs:26-82,850-873` | 57 | Add a requirement/edge case describing multi-client completion delivery semantics (a full callback channel for one client must not block delivery to others; FIFO retry ordering). |

## Inter-Spec Conflicts

None detected (single spec directory for this component).

## Recommendations

1. **Fix or scope down the telemetry latency claim** (major): either wire real timestamps (capture the real submission `Instant`/TSC and compute elapsed at completion) into `record_op`, or update the spec's Assumptions section to state that filesys telemetry only tracks op counts/bytes, not latency, until implemented.
2. **Implement or descope the io_uring backpressure edge case** (major): either add a pending-submission queue that retries when `on_idle()` sees ring space (matching the `ClientSession` backlog pattern already used for completion delivery), or soften the spec's Edge Cases section to describe the current fail-fast behavior.
3. **Align FR-015 with reality**: add a real doc example for `open_or_create_backing_file`, and drop the ` ```ignore ` fence on `create()`'s example (using a real temp path) so it participates in `cargo test --doc`.
4. **Tighten FR-002's warn-level claim**: either log queue-full events at warn (as already done for the fsync-SQE-push-failure case) and promote disconnection to warn, or narrow the spec text to describe only the levels actually used.
5. **Clarify FR-006's "default 512 bytes"**: either add an actual default (e.g., a `DeviceConfig::with_defaults` or a `Default` block size constant) or reword the FR to describe 512 as the documented minimum rather than a default.
6. **Backfill specs for unspecced telemetry, logging-bypass, and completion-backlog behaviors** listed above so future drift analyses have a requirement to check against.
