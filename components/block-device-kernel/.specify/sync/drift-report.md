# Spec Drift Report

Generated: 2026-07-22T22:33:43Z
Project: block-device-kernel

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 41 (24 FR + 9 SC + 8 NFR) |
| ✓ Aligned | 38 (93%) |
| ⚠️ Drifted | 3 (7%) |
| ✗ Not Implemented | 0 (0%) |
| 🆕 Unspecced Code | 1 |

Verification performed: read `specs/001-block-device-kernel/spec.md` against `src/lib.rs`, `src/actor.rs`,
`src/config.rs`, `src/telemetry.rs`, `benches/*.rs`, `tests/integration.rs`, `Cargo.toml`. Also ran
`cargo test -p block-device-kernel --lib` (12/12 pass), `cargo test -p block-device-kernel --test integration`
(2 non-`#[ignore]` tests pass without hardware, 13 hardware-dependent tests correctly `#[ignore]`d), and
`cargo clippy -p block-device-kernel --all-targets -- -D warnings` (clean).

## Detailed Findings

### Spec: 001-block-device-kernel - Block Device Kernel Component

Note: this spec carries a `Status: Backfilled` / "Source: Generated from existing implementation" header,
so most FRs were derived directly from the code and are trivially aligned. The drift below is exactly the
kind of "documents current behavior, not intent" gap the backfill notice warns about — in this case the
spec asserts a capability (accurate latency telemetry) that the code does not actually deliver.

#### Aligned ✓

- FR-001: Implements `IBlockDevice` and `IBlockDeviceAdmin` → `src/lib.rs:198-335`
- FR-003: `define_component!` with `provides: [IBlockDevice, IBlockDeviceAdmin]`, `receptacles: { logger: ILogger }` → `src/lib.rs:55-71`
- FR-004: Opens device with `O_DIRECT | O_DSYNC` → `src/config.rs:168`
- FR-005: io_uring only, no pread/pwrite fallback → `src/actor.rs` (all IO via `opcode::Read`/`opcode::Write`)
- FR-006: `S_IFBLK` check via `stat(2)`, rejects regular files → `src/config.rs:86-113`
- FR-007: block size ≥512 & power-of-2, `num_blocks=0` ⇒ auto-detect → `src/config.rs:33-62`
- FR-008: `ReadSync`/`WriteSync` via SQE push + `submit_and_wait(1)` → `src/actor.rs:250-320,324-381`
- FR-009: `ReadAsync`/`WriteAsync` with timeout + `OpHandle` in `inflight` map → `src/actor.rs:383-535`
- FR-010: `WriteZeros` via `posix_memalign(512, ...)`, io_uring write, `libc::free` after → `src/actor.rs:537-617`
- FR-011: `BatchSubmit` processes each op via recursive `process_command` call → `src/actor.rs:221-225`
- FR-012: `AbortOp` submits `AsyncCancel` SQE + sends `AbortAck` → `src/actor.rs:619-630`
- FR-013: `NsProbe` returns single `NamespaceInfo{ns_id:1,...}` → `src/actor.rs:632-644`
- FR-014: `NsCreate`/`NsDelete`/`NsFormat`/`ControllerReset` → `NotSupported` → `src/actor.rs:232-245`
- FR-015: `KernelHandler` implements `ActorHandler<ControlMessage>` with `handle()`/`on_idle()` → `src/actor.rs:847-879`
- FR-016: Per-client `SpscChannel` pairs, capacity 64 → `src/lib.rs:50,232-241`
- FR-017: `validate_lba` uses `checked_add` for overflow-safe bounds check → `src/actor.rs:166-182`
- FR-018: `ns_id == 1` validated in `sector_size`/`num_sectors`/`validate_lba` → `src/lib.rs:258-274`, `src/actor.rs:167-171`
- FR-019: `posix_fadvise(fd, 0, 0, POSIX_FADV_DONTNEED)` on open → `src/config.rs:189-191`
- FR-020: `fcntl(F_GETFL)` verifies `O_DIRECT` is active → `src/config.rs:174-184`
- FR-022: Criterion benches for latency (`benches/latency.rs`) and throughput at 1/8/32/128 blocks (`benches/throughput.rs`) — both present and match spec exactly
- FR-023: `set_pci_address`/`set_actor_cpu`/`signal_stop`/`detach_controller` are no-ops → `src/lib.rs:199-213`
- FR-024: `ControlMessage::Shutdown` → `shutdown_requested = true` → `on_idle()` returns `false` → `src/actor.rs:862-864,868-871`
- SC-001 to SC-005, SC-007 to SC-009: structurally consistent with implementation; SC-009 explicitly verified by test run (see above)
- NFR-001 through NFR-008: all verified (ring depth=128 constant, `unsafe impl Send`, `Instant`-based deadlines, `// SAFETY:` comments present on every `unsafe` block, no fallback panics on IO error paths — `.ok()` is used to swallow `submit_and_wait` errors rather than panicking)

#### Drifted ⚠️

- **FR-021 / SC-006** — Telemetry latency stats are always zero (functionally broken)
  - Spec says: "Component MUST provide feature-gated telemetry ... that tracks total ops, min/max/mean latency, total bytes, and mean throughput" and "SC-006: Feature-gated telemetry produces accurate `TelemetrySnapshot` values when enabled."
  - Code does: every call to `TelemetryStats::record_op(latency_ns, bytes)` passes a **hardcoded `0`** for `latency_ns` — `src/actor.rs:312, 373, 609, 689, 747`. The `InflightOp.start_ns` field (`src/actor.rs:100`) is populated with `Instant::now().elapsed().as_nanos()` (`src/actor.rs:456, 530`), which measures elapsed time since a *freshly created* `Instant`, i.e. always ≈0ns — it is never used to compute an actual start-to-completion duration. Only `total_ops`, `total_bytes`, and `mean_throughput_mbps` are accurate; `min_latency_ns`, `max_latency_ns`, and `mean_latency_ns` will always report 0.
  - Location: `src/actor.rs:100,312,373,456,530,609,689,747`
  - Severity: **major** — this is the telemetry feature's core value proposition and it silently reports meaningless numbers instead of erroring or omitting the field.

- **FR-002** — No warn-level logging exists anywhere in the crate
  - Spec says: "log initialization at info level, operations at debug level, and channel disconnections at warn level."
  - Code does: `grep -n warn src/*.rs` returns zero matches. Client disconnection is logged at **debug**, not warn (`src/actor.rs:858`: `log.debug(&format!("actor: disconnecting client {client_id}"))`). `ILogger::warn()` exists (`components/interfaces/src/ilogger.rs:6`) but is never called by this component.
  - Location: `src/actor.rs:858` (wrong level); no `warn()` call exists anywhere else in `src/`
  - Severity: **moderate**

- **Edge case: "Client callback channel disconnected"**
  - Spec says: "Client callback channel disconnected: Logged at warn level, completion silently dropped."
  - Code does: nothing is dropped and nothing is logged. `ClientSession::deliver()` buffers the completion into a FIFO backlog (`pending: VecDeque<Completion>`) when the callback ring is full, and `KernelHandler::poll_clients()` retries delivery every idle-loop tick via `flush_pending()` until it succeeds — an intentional non-blocking-delivery design (documented in code comments as avoiding "whole-drive deadlock") that is strictly better than the spec's described drop behavior, but the spec text was not updated to reflect it, and no warn log accompanies either the backlog event or eventual delivery.
  - Location: `src/actor.rs:26-81` (`ClientSession::deliver`/`flush_pending`), `src/actor.rs:815-828` (`send_completion`/`poll_clients`)
  - Severity: **moderate**

#### Not Implemented ✗

None.

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| Non-blocking per-client completion backlog (`ClientSession.pending`/`flush_pending`, FIFO-ordered retry to avoid one slow client head-of-line-blocking the single-threaded actor for all others) | `src/actor.rs:26-81, 815-828` | ~60 | Amend `001-block-device-kernel` FR-002 and the "Client callback channel disconnected" edge case |

## Inter-Spec Conflicts

None — only one spec (`001-block-device-kernel`) exists for this component.

## Recommendations

1. **Fix or re-scope FR-021/SC-006 (major).** Either wire `InflightOp.start_ns` (captured with a monotonic clock at submission, e.g. `Instant::now()` stored and diffed at completion — not `Instant::now().elapsed()`) through to `record_op()`'s `latency_ns` argument for the async and completion-harvest paths, and measure elapsed time around `submit_and_wait` for the sync paths; or, if latency tracking is genuinely out of scope for v0.1.0, update FR-021/SC-006 and `TelemetrySnapshot` docs to state that only throughput/op-count are tracked, dropping the min/max/mean latency claims (or marking them as always-zero placeholders) so `telemetry()` consumers aren't misled.
2. **Add the missing warn-level logging path (moderate), or amend FR-002 / the edge case bullet.** Either add a `log.warn(...)` call at the point a completion is first buffered into `pending` (ring full) and/or on client disconnection, matching the spec as written; or update FR-002 and the edge-case list to describe the actual non-blocking backlog-and-retry behavior instead of "silently dropped."
3. **Backfill a spec entry for the completion backlog mechanism.** It's a deliberate, comment-documented design decision (anti-head-of-line-blocking) that materially changes system behavior under backpressure and deserves its own FR/NFR (e.g., "backlog is unbounded — a permanently stalled client's completions accumulate without limit") so future readers don't have to reverse-engineer it from code comments.
4. **`cargo fmt --check` currently fails** for this crate (diffs in `src/actor.rs` around lines 574 and the `WriteDone` send-completion block, and `src/config.rs:88`). Not spec-related but violates the project-wide `CLAUDE.md` formatting convention and `tasks.md` T029; run `cargo fmt -p block-device-kernel` to fix.
5. Consider working through the still-open `tasks.md` checklist (T006-T012 testing gaps, T017 missing README.md) since they map directly onto real coverage gaps confirmed here (e.g., no unit test exercises the `record_op` latency bug that this analysis found).
