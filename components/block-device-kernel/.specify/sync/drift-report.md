# Drift Report — block-device-kernel

**Generated**: pending

Read-only spec↔implementation drift analysis. Sources: `specs/001-block-device-kernel/spec.md` vs `src/{lib.rs,actor.rs,config.rs,telemetry.rs}` and `tests/integration.rs`.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 43 (FR-001..026, NFR-001..008, SC-001..009) |
| Aligned | 41 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced | 1 |

## Detailed Findings

### Spec 001-block-device-kernel — Block Device Kernel Component

#### Aligned ✓

- **FR-001** IBlockDevice + IBlockDeviceAdmin — `src/lib.rs:198,216`.
- **FR-002** ILogger receptacle; info on init, debug on connect/disconnect; no warn() anywhere (matches amended text) — `src/lib.rs:60,117,172,229`; `src/actor.rs:881,887`.
- **FR-003** `define_component!` provides [IBlockDevice, IBlockDeviceAdmin], receptacle logger — `src/lib.rs:55-70`.
- **FR-004** open O_DIRECT|O_DSYNC — `src/config.rs:168`.
- **FR-005** io_uring sole IO, no pread/pwrite fallback — all IO via `self.ring` in `src/actor.rs`.
- **FR-006** raw block device (S_IFBLK) enforced, regular files rejected — `src/config.rs:104,163`.
- **FR-007** block_size min512/pow2, num_blocks 0 → BLKGETSIZE64 auto-detect — `src/config.rs:33-62,116`.
- **FR-008** sync R/W via SQE + `submit_and_wait(1)` — `src/actor.rs:325,390`.
- **FR-009** async R/W with timeout + OpHandle inflight map; caller `tag` NOT propagated (completions emit `tag: 0`) — matches backfilled FR-009 text — `src/actor.rs:414,494,723,729`.
- **FR-010** WriteZeros posix_memalign 512-aligned buffer, io_uring write, free after — `src/actor.rs:561-645`.
- **FR-011** BatchSubmit recursive `process_command` — `src/actor.rs:222-226`.
- **FR-012** AbortOp AsyncCancel SQE + AbortAck — `src/actor.rs:647-658`.
- **FR-013** NsProbe single NamespaceInfo ns_id=1 — `src/actor.rs:660-672`.
- **FR-014** NotSupported for NsCreate/NsDelete/NsFormat/ControllerReset — `src/actor.rs:248-261`.
- **FR-015** actor model, io_uring loop, `handle()`+`on_idle()` — `src/actor.rs:876-908`.
- **FR-016** per-client SPSC channels capacity 64 — `src/lib.rs:50`, `src/actor.rs` via `SpscChannel`.
- **FR-017** LBA bounds `lba+num<=device` with `checked_add` — `src/actor.rs:167-183`.
- **FR-018** ns_id==1 validation — `src/actor.rs:168`; `src/lib.rs:259,268`.
- **FR-019** posix_fadvise(POSIX_FADV_DONTNEED) on init — `src/config.rs:190`.
- **FR-020** verify O_DIRECT via fcntl(F_GETFL) — `src/config.rs:176-184`.
- **FR-022** Criterion latency + throughput benches — `Cargo.toml` `[[bench]]`; `benches/{latency,throughput}.rs`.
- **FR-023** Admin no-ops set_pci_address/set_actor_cpu/signal_stop/detach_controller — `src/lib.rs:199-213`.
- **FR-024** graceful shutdown via ControlMessage::Shutdown → on_idle false — `src/actor.rs:891,898`.
- **FR-025** unbounded per-client FIFO backlog, non-blocking delivery, flush oldest-first — `src/actor.rs:38,59-80,855`.
- **FR-026** device-info numa_node -1, nvme_version "N/A (kernel block device)", read_write_stats default — `src/lib.rs:292-298,332`.
- **NFR-001** 512-byte alignment enforced by O_DIRECT — `src/config.rs:168`.
- **NFR-002** default ring depth 128 — `src/lib.rs:53`.
- **NFR-003** no panic; errors as NvmeBlockError completions — `src/actor.rs` (map_err on all IO paths).
- **NFR-004** SAFETY comments on unsafe blocks — throughout `src/actor.rs`, `src/config.rs`.
- **NFR-005** Send-safe — `unsafe impl Send for KernelHandler` `src/actor.rs:912`.
- **NFR-006** on_idle returns true while clients/inflight exist — `src/actor.rs:906`.
- **NFR-007** Instant::now() deadline comparison — `src/actor.rs:799-816`.
- **NFR-008** kernel>=5.1 — platform/doc claim, no code contradiction.
- **SC-001** read-after-write integrity, O_DSYNC durability — tested (ignored) `tests/integration.rs:91,441`.
- **SC-002** auto-detect via BLKGETSIZE64 — `tests/integration.rs:64`; `src/config.rs:41`.
- **SC-003** rejects non-block-device/bad-size/OOR LBA — `tests/integration.rs:74,83,251`; `src/config.rs`.
- **SC-004** multi-client concurrent IO — `tests/integration.rs:377`.
- **SC-005** async timeout → Completion::Timeout — `src/actor.rs:799-823`.
- **SC-007** Criterion stable measurements — bench targets.
- **SC-008** drop-in IBlockDevice replacement — interface parity.
- **SC-009** unit tests pass without hardware; integration `#[ignore]` — `src/lib.rs:337`, `tests/integration.rs`.

#### Drifted ⚠️

- **FR-021** — *moderate*.
  - Spec text (FR-021 + "Last Synced" note): telemetry tracks "min/max/mean latency"; the note asserts the "record_op previously passed a hardcoded 0" defect "was **fixed in code** ... per-op start time captured, elapsed ns recorded."
  - Actual: the fix is present in the SYNCHRONOUS paths (`handle_read_sync` `src/actor.rs:322,331`, `handle_write_sync` `:387,396`, `handle_write_zeros` `:624,636`) and in `wait_for_cqe`'s incidental async harvest (`:716-718`). But the PRIMARY async-completion path, `harvest_completions()`, still calls `self.telemetry.record_op(0, op.bytes)` — hardcoded `0` latency — even though `InflightOp` carries a populated `start` timestamp (`src/actor.rs:480,555`). Async ops (ReadAsync/WriteAsync) that complete via the normal `on_idle` → `harvest_completions` path therefore record 0ns latency, driving `min_latency_ns` to 0 and skewing mean.
  - Location: `src/actor.rs:776`.
  - Severity: moderate (telemetry latency is materially wrong for the async IO path — the primary high-throughput path; sibling `block-device-filesys` fixed this same call site at `actor.rs:822-823`).

- **SC-006** — *moderate* (same root cause as FR-021).
  - Spec text: "Feature-gated telemetry produces accurate TelemetrySnapshot values when enabled."
  - Actual: latency values are inaccurate for async operations because `harvest_completions` records 0ns (`src/actor.rs:776`). Snapshot min/mean latency are not accurate when async IO dominates.
  - Location: `src/actor.rs:776`; `src/telemetry.rs:35`.
  - Severity: moderate.

#### Not Implemented ✗

None.

## Unspecced Code

| Feature | Location | Suggested Spec |
|---|---|---|
| `Command::FlushSync { ns_id }` handler — returns `FlushDone{Ok}` as a validated no-op (ns_id==1) since O_DIRECT\|O_DSYNC leaves no volatile write cache; ns_id!=1 → InvalidNamespace | `src/actor.rs:233-247` | Add an FR (parallel to filesys FR-022) documenting FlushSync as a validated no-op for the kernel device, justified by O_DSYNC durability. The shared `IBlockDevice` gained FlushSync after this spec was written and the kernel spec never mentions it. |

## Recommendations

1. **FR-021 / SC-006 (moderate)**: fix `harvest_completions()` at `src/actor.rs:776` to record real latency — replace `record_op(0, op.bytes)` with `record_op(op.start.elapsed().as_nanos() as u64, op.bytes)` (guarded by `#[cfg(feature = "telemetry")]`), matching the sibling filesys implementation at `actor.rs:822-823`. The `InflightOp.start` field already exists and is populated; only the harvest call site was missed by the 2026-08-07 sweep. Until fixed, the spec's "latency defect fixed" claim overstates the code.
2. **Unspecced FlushSync**: backfill an FR for the FlushSync handler (`src/actor.rs:233-247`).
3. All other requirements are aligned; the io_uring-only, O_DSYNC-durable design and the anti-head-of-line-blocking delivery backlog match the spec exactly.
