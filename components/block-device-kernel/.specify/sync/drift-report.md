---
spec_sync_component: block-device-kernel
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:28:14Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: cdeb69b5d15589e4625d2a73b54bbb262358ad2ea3b8a042a8eb98eaddc23f73
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report — block-device-kernel

**Generated**: 2026-09-02

Read-only spec↔implementation drift analysis. Sources: `specs/001-block-device-kernel/spec.md` vs `src/{lib.rs,actor.rs,config.rs,telemetry.rs}` and `tests/integration.rs`. Verified against HEAD `2fc1cd3c`.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 44 (FR-001..027, NFR-001..008, SC-001..009) |
| Aligned | 42 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced | 0 |

> Change from prior sync: FlushSync is no longer unspecced — it was backfilled as **FR-027** on 2026-08-20 and is confirmed aligned this run, so requirement count rises 43→44 and unspecced drops 1→0. The two drifted items (FR-021, SC-006) are unchanged: the async-path telemetry-latency defect at `src/actor.rs:776` persists at HEAD.

## Detailed Findings

### Spec 001-block-device-kernel — Block Device Kernel Component

#### Aligned ✓

- **FR-001** IBlockDevice + IBlockDeviceAdmin — `src/lib.rs:198,216`.
- **FR-002** ILogger receptacle; info on init, debug on connect/disconnect; no `warn()` anywhere (matches amended text) — `src/lib.rs:99,117,133,171,229`; `src/actor.rs:881,887`.
- **FR-003** `define_component!` provides [IBlockDevice, IBlockDeviceAdmin], receptacle logger — `src/lib.rs:55-71`.
- **FR-004** open O_DIRECT|O_DSYNC — `src/config.rs:168`.
- **FR-005** io_uring sole IO, no pread/pwrite fallback — all IO via `self.ring` in `src/actor.rs`.
- **FR-006** raw block device (S_IFBLK) enforced, regular files rejected — `src/config.rs:104,117,163`.
- **FR-007** block_size min512/pow2, num_blocks 0 → BLKGETSIZE64 auto-detect — `src/config.rs:34-62,116-153`.
- **FR-008** sync R/W via SQE + `submit_and_wait(1)` — `src/actor.rs:325,390`.
- **FR-009** async R/W with timeout + OpHandle inflight map; caller `tag` NOT propagated (completions emit `tag: 0`, params `_tag`) — matches backfilled FR-009 text — `src/actor.rs:414,494,723-733`.
- **FR-010** WriteZeros posix_memalign 512-aligned buffer, io_uring write, free after — `src/actor.rs:561-645`.
- **FR-011** BatchSubmit recursive `process_command` — `src/actor.rs:222-226`.
- **FR-012** AbortOp AsyncCancel SQE + AbortAck — `src/actor.rs:647-658`.
- **FR-013** NsProbe single NamespaceInfo ns_id=1 — `src/actor.rs:660-672`.
- **FR-014** NotSupported for NsCreate/NsDelete/NsFormat/ControllerReset — `src/actor.rs:248-261`.
- **FR-015** actor model, io_uring loop, `handle()`+`on_idle()` — `src/actor.rs:876-908`.
- **FR-016** per-client SPSC channels capacity 64 — `src/lib.rs:50,232,237`.
- **FR-017** LBA bounds `lba+num<=device` with `checked_add` — `src/actor.rs:167-183`.
- **FR-018** ns_id==1 validation — `src/actor.rs:168`; `src/lib.rs:259,268`.
- **FR-019** posix_fadvise(POSIX_FADV_DONTNEED) on init — `src/config.rs:190`.
- **FR-020** verify O_DIRECT via fcntl(F_GETFL) — `src/config.rs:176-184`.
- **FR-022** Criterion latency + throughput benches — `Cargo.toml:25-31`; `benches/{latency,throughput}.rs`.
- **FR-023** Admin no-ops set_pci_address/set_actor_cpu/signal_stop/detach_controller — `src/lib.rs:199,201,207,213`.
- **FR-024** graceful shutdown via ControlMessage::Shutdown → on_idle false — `src/actor.rs:891-893,898-899`.
- **FR-025** unbounded per-client FIFO backlog, non-blocking delivery, flush oldest-first — `src/actor.rs:38,59-80,850-857`.
- **FR-026** device-info numa_node -1, nvme_version "N/A (kernel block device)", read_write_stats default — `src/lib.rs:292-298,332-334`.
- **FR-027** FlushSync validated no-op: ns_id==1 → `FlushDone{Ok}`, ns_id!=1 → InvalidNamespace; no syscall (O_DIRECT|O_DSYNC durability) — `src/actor.rs:233-247`. *(Backfilled 2026-08-20; confirmed aligned this run.)*
- **NFR-001** 512-byte alignment enforced by O_DIRECT — `src/config.rs:168`; WriteZeros uses posix_memalign 512 — `src/actor.rs:582`.
- **NFR-002** default ring depth 128 — `src/lib.rs:53`.
- **NFR-003** no panic; errors as NvmeBlockError completions — `src/actor.rs` (map_err on all IO paths, e.g. `:335,400,640`).
- **NFR-004** SAFETY comments on unsafe blocks — throughout `src/actor.rs`, `src/config.rs`.
- **NFR-005** Send-safe — `unsafe impl Send for KernelHandler` `src/actor.rs:912`; `ControlMessage` `:91`.
- **NFR-006** on_idle returns true while clients/inflight exist — `src/actor.rs:906`.
- **NFR-007** Instant::now() deadline comparison — `src/actor.rs:466-470,540-544,804-810`.
- **NFR-008** kernel>=5.1 — platform/doc claim, no code contradiction.
- **SC-001** read-after-write integrity, O_DSYNC durability — tested (ignored) `tests/integration.rs`.
- **SC-002** auto-detect via BLKGETSIZE64 — `src/config.rs:41-49,116-153`.
- **SC-003** rejects non-block-device/bad-size/OOR LBA — `src/config.rs:34-39,104-110`; `src/actor.rs:173-181`; unit tests `src/config.rs:201-217`.
- **SC-004** multi-client concurrent IO — `src/lib.rs:217-256` (per-client channels), actor serialization.
- **SC-005** async timeout → Completion::Timeout — `src/actor.rs:799-834`.
- **SC-007** Criterion stable measurements — bench targets.
- **SC-008** drop-in IBlockDevice replacement — full interface surface implemented `src/lib.rs:216-335`.
- **SC-009** unit tests pass without hardware; integration `#[ignore]` — `src/lib.rs:337-410`, `src/config.rs:197-226`.

#### Drifted ⚠️

- **FR-021** — *moderate*.
  - Spec text (FR-021): telemetry tracks "total ops, min/max/mean latency, total bytes, and mean throughput"; "Without the feature, `telemetry()` returns `FeatureNotEnabled`." The `FeatureNotEnabled` behavior is correct (`src/lib.rs:322-327`), and the sync paths + blocking async-completion path record real latency.
  - Actual: the fix is present in the SYNCHRONOUS paths (`handle_read_sync` `src/actor.rs:329-333`, `handle_write_sync` `:394-398`, `handle_write_zeros` `:634-638`) and in `wait_for_cqe`'s blocking async harvest (`:716-718`, `op.start.elapsed()`). But the PRIMARY async-completion path, `harvest_completions()`, still calls `self.telemetry.record_op(0, op.bytes)` — hardcoded `0` latency — even though `InflightOp` carries a populated `start: Instant` (`src/actor.rs:101`, set at `:480` and `:554`). Async ops (ReadAsync/WriteAsync) completing via the normal `on_idle` → `harvest_completions` path therefore record 0 ns latency, driving `min_latency_ns` to 0 (`src/telemetry.rs:41-52`) and skewing the mean.
  - Location: `src/actor.rs:776`.
  - Severity: moderate (telemetry latency is materially wrong for the async IO path — the primary high-throughput path; sibling `block-device-filesys` fixed this same call site).
  - The spec's own header note already documents this residual defect honestly (it does not overstate the fix), so the spec text is NOT drifted; the code is.

- **SC-006** — *moderate* (same root cause as FR-021).
  - Spec text: "Feature-gated telemetry produces accurate `TelemetrySnapshot` values when enabled."
  - Actual: latency values are inaccurate for async operations because `harvest_completions` records 0 ns (`src/actor.rs:776`). Snapshot `min_latency_ns`/`mean_latency_ns` are not accurate when async IO dominates. `total_ops`, `total_bytes`, and `mean_throughput_mbps` remain accurate.
  - Location: `src/actor.rs:776`; `src/telemetry.rs:35-66`.
  - Severity: moderate.

#### Not Implemented ✗

None.

## Unspecced Code

None. The previously-unspecced `Command::FlushSync` handler (`src/actor.rs:233-247`) is now covered by **FR-027** and US2 acceptance scenario 5 (backfilled 2026-08-20). The `process_command` match is exhaustive over `interfaces::Command` (compiles), so no command variant is silently unhandled.

## Recommendations

1. **FR-021 / SC-006 (moderate)**: fix `harvest_completions()` at `src/actor.rs:776` to record real latency — replace `record_op(0, op.bytes)` with `record_op(op.start.elapsed().as_nanos() as u64, op.bytes)` (guarded by `#[cfg(feature = "telemetry")]`), mirroring the already-correct `wait_for_cqe` site at `src/actor.rs:718`. The `InflightOp.start` field already exists and is populated; only the harvest call site was missed by the 2026-08-07 sweep. This is a code change → standing ALIGN task in `.specify/sync/align-tasks.md` (this sync did not edit `.rs`).
2. All other requirements are aligned; the io_uring-only, O_DSYNC-durable design, the anti-head-of-line-blocking delivery backlog (FR-025), the device-info constants (FR-026), and the FlushSync validated no-op (FR-027) all match the spec.
