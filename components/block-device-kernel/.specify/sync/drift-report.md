---
spec_sync_component: block-device-kernel
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T22:45:24Z
spec_sync_git_commit: e3aef85a
spec_sync_inputs_sha256: d8c42984c817f4d6def16af807f64d14299957c7604d093afb6f44febb4fc1ee
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report — block-device-kernel

**Generated**: 2026-09-03
**Spec**: `specs/001-block-device-kernel/spec.md` (Status: Backfilled)
**Mode**: Read-only drift analysis, then **ALIGN** apply to code (spec authoritative for the telemetry contract), + freshness stamp.

This sweep supersedes the earlier stale artifact (which read "Generated:
pending", listed **2 Drifted** — FR-021 + SC-006 async latency — and **1
Unspecced** — the `FlushSync` handler). Both of the stale findings are now
resolved:

- **`FlushSync` is no longer unspecced** — it was documented as **FR-027**
  (backfilled 2026-08-20): a validated no-op returning `Ok(())` for `ns_id == 1`
  and `InvalidNamespace` otherwise, because `O_DIRECT | O_DSYNC` (FR-004) leaves
  no volatile write cache to drain. Verified against `src/actor.rs`
  `Command::FlushSync` arm (`handle_ns_probe`/dispatch in `process_command`).
- **FR-021 / SC-006 async latency is fixed in code this sweep** (see below).

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (`001-block-device-kernel`) |
| Requirements Checked | 27 FR + 8 NFR + 9 SC + 7 Key Entities |
| Aligned | 51 |
| Drifted (this sweep) | 1 → resolved by **ALIGN** (code fix) |
| Not Implemented | 0 |
| Unspecced | 0 |

**Verification runs this sweep** (all green):
- `cargo build -p block-device-kernel` — clean
- `cargo build -p block-device-kernel --features telemetry` — clean
- `cargo clippy -p block-device-kernel --all-targets -- -D warnings` — clean
- `cargo clippy -p block-device-kernel --all-targets --features telemetry -- -D warnings` — clean
- `cargo test -p block-device-kernel -- --test-threads 1` (default + `--features telemetry`) — 2 unit tests pass per set; 13 IO/async integration tests + 2 doctests are `#[ignore]`/`ignore` (require a real block device), matching SC-009.

## Spec: 001-block-device-kernel — Block Device Kernel Component

### Drifted ⚠️ → resolved by ALIGN (code fix)

- **FR-021 / SC-006 — async telemetry records 0 ns latency** — severity: moderate
  (code defect; spec authoritative — HARD RULE against backfilling specs to match
  bugs, see `.specify/sync/align-tasks.md`).
  - Spec: FR-021 requires feature-gated telemetry tracking "min/max/mean latency";
    SC-006 requires "accurate `TelemetrySnapshot` values when enabled". Both are
    correct as written and were **not** backfilled to describe the defect.
  - Actual (before this sweep): the primary async-completion path
    `harvest_completions()` called `self.telemetry.record_op(0, op.bytes)` with a
    hardcoded `0` (`src/actor.rs`, in the success arm of the harvested-CQE loop),
    even though `InflightOp.start: Instant` is populated at both async insert
    sites (`handle_read_async`, `handle_write_async`). Async `ReadAsync`/
    `WriteAsync` ops harvested on the idle loop therefore recorded 0 ns, pinning
    `min_latency_ns` to 0 and skewing the mean whenever async IO was present.
  - Direction — **code authoritative (ALIGN)**: the spec states the desired
    contract; the code had a defect. The correct completion site already existed:
    `wait_for_cqe()` records `op.start.elapsed().as_nanos() as u64` for the CQEs
    it drains while blocking on a sync op. The fix mirrors that exactly.
  - Fix applied: `harvest_completions()` now records
    `op.start.elapsed().as_nanos() as u64` for the async completion path (no new
    `InflightOp` field; the existing `start: Instant` is reused). All five
    telemetry recording sites — `handle_read_sync`, `handle_write_sync`,
    `write_zeros`, `wait_for_cqe`, and `harvest_completions` — now record real
    per-op latency, so FR-021/SC-006 hold for both sync and async IO. Tracked as
    ✅ RESOLVED in `.specify/sync/align-tasks.md`.
  - **Follow-up (not blocking):** the dedicated accuracy test asserting non-zero
    `min/max/mean_latency_ns` for async ops under `--features telemetry` remains
    deferred to a hardware/loopback run — the async harvest path requires a real
    io_uring completion against a real device (all async integration tests are
    `#[ignore]`). The fix mirrors a site already exercised by the same
    hardware-gated path, and both feature sets build/clippy clean.

### Aligned ✓ (verified this sweep)

| Req | Evidence |
|-----|----------|
| FR-001 implements IBlockDevice + IBlockDeviceAdmin | `impl IBlockDevice` `src/lib.rs:216`; `impl IBlockDeviceAdmin` `src/lib.rs:198` |
| FR-002 ILogger receptacle; info on init, debug on connect/disconnect; no warn (amended) | receptacle `src/lib.rs:59-61`; info `src/lib.rs:117,172`; debug connect `src/actor.rs:881`, disconnect `:887`, `src/lib.rs:229`; no `warn()` in crate |
| FR-003 `define_component!` provides IBlockDevice+IBlockDeviceAdmin, logger receptacle | `src/lib.rs:55-71` |
| FR-004 open with O_DIRECT\|O_DSYNC | `custom_flags(libc::O_DIRECT \| libc::O_DSYNC)` `src/config.rs:168` |
| FR-005 io_uring sole IO mechanism, no pread/pwrite fallback | `src/actor.rs` uses only `opcode::{Read,Write,AsyncCancel}`; no pread/pwrite in crate |
| FR-006 rejects regular files (S_IFBLK only) | `assert_block_device` `src/config.rs:86-113` (S_IFBLK check `:104`) |
| FR-007 block_size ≥512 pow2; num_blocks=0 → BLKGETSIZE64 auto-detect | validation `src/config.rs:34-39`; auto-detect `:41-50`, `query_device_size` `:116-153` |
| FR-008 sync R/W via SQE + submit_and_wait(1) | `handle_read_sync` `src/actor.rs:266`, `handle_write_sync` `:344`; `submit_and_wait(1)` `:325,390` |
| FR-009 async R/W with timeout + OpHandle inflight map; tag NOT propagated (tag:0) | `handle_read_async` `src/actor.rs:407`, `handle_write_async` `:487`; inflight insert `:472,546`; completions emit `tag: 0` (`_tag` unused) — documented current behavior |
| FR-010 WriteZeros posix_memalign 512-aligned, io_uring, free after | `handle_write_zeros` `src/actor.rs:561`; `posix_memalign(…,512,…)` `:582`; `libc::free` `:632` |
| FR-011 BatchSubmit sequential recursive dispatch | `Command::BatchSubmit` `src/actor.rs:222-226` (recursive `process_command`) |
| FR-012 AbortOp → AsyncCancel SQE + AbortAck | `handle_abort` `src/actor.rs:647-658` (AsyncCancel `:648`) |
| FR-013 NsProbe → single NamespaceInfo ns_id=1 | `handle_ns_probe` `src/actor.rs:660-672` |
| FR-014 NsCreate/NsDelete/NsFormat/ControllerReset → NotSupported | `src/actor.rs:248-261` |
| FR-015 actor model, dedicated thread, io_uring loop; ActorHandler handle()+on_idle() | `impl ActorHandler<ControlMessage>` `src/actor.rs:876`; `on_idle` polls/harvests/checks `:897-907` |
| FR-016 per-client SPSC channels capacity 64 via SpscChannel | `CLIENT_CHANNEL_CAPACITY = 64` `src/lib.rs:50`; `SpscChannel::new` `src/lib.rs:232,237` |
| FR-017 LBA bounds validate lba+num_blocks ≤ device, checked_add | `validate_lba` `src/actor.rs:167-183` (`checked_add` `:173`) |
| FR-018 ns_id==1 validation for IO + sector/num queries | `validate_lba` `:168`; `sector_size`/`num_sectors` ns check `src/lib.rs:259,268` |
| FR-019 posix_fadvise POSIX_FADV_DONTNEED on init | `src/config.rs:189-191` |
| FR-020 verify O_DIRECT active via fcntl(F_GETFL) | `src/config.rs:176-184` |
| FR-021 feature-gated telemetry min/max/mean latency, bytes, throughput; FeatureNotEnabled without | `TelemetryStats` `src/telemetry.rs:13-103`; **real latency now recorded at all 5 sites** incl. async harvest `src/actor.rs` (fixed this sweep); `FeatureNotEnabled` `src/lib.rs:323-326` |
| FR-022 Criterion latency + throughput benches | `Cargo.toml` `[[bench]]`; `benches/{latency,throughput}.rs` |
| FR-023 IBlockDeviceAdmin set_pci_address/set_actor_cpu/signal_stop/detach_controller no-ops | `src/lib.rs:199,201,207,213` |
| FR-024 graceful shutdown via Shutdown → on_idle returns false | `ControlMessage::Shutdown` `src/actor.rs:891-893`; `on_idle` `:898-900` |
| FR-025 non-blocking delivery, unbounded per-client FIFO backlog, oldest-first retry | `ClientSession.pending` `src/actor.rs:38`; `deliver` `:59-64`; `flush_pending` `:69-80`; drained in `poll_clients` `:855-857` |
| FR-026 device-info surface: numa_node -1, nvme_version "N/A (kernel block device)", read_write_stats zeroed | `numa_node` `src/lib.rs:292-294`; `nvme_version` `:296-298`; `read_write_stats` default `:332-334` |
| FR-027 FlushSync validated no-op (ns_id==1 → Ok, else InvalidNamespace); no syscall (O_DSYNC) | `Command::FlushSync` arm `src/actor.rs:233-247` |
| SC-001 read-after-write integrity, O_DSYNC durability | `tests/integration.rs` `write_sync_read_sync_roundtrip:91`, `data_integrity_multi_block_patterns:441` (`#[ignore]`, hardware) |
| SC-002 auto-detect size via BLKGETSIZE64 | `initialize_auto_detects_size` `tests/integration.rs:65`; `query_device_size` `src/config.rs:116` |
| SC-003 reject non-block-device / bad block size / OOR LBA | `initialize_rejects_non_block_device` + `block_size_*` + `lba_out_of_range_error` tests |
| SC-004 multi-client concurrent independent IO | `multiple_clients_independent_channels` `tests/integration.rs:377` (hardware) |
| SC-005 async timeout → Completion::Timeout | `check_timeouts` `src/actor.rs:799-835` (emits `Completion::Timeout` `:822`) |
| SC-006 telemetry produces accurate TelemetrySnapshot | **now holds for sync + async** after this sweep's fix; `snapshot()` `src/telemetry.rs:68-102` |
| SC-007 Criterion benches stable | `benches/{latency,throughput}.rs` (measurement quality target) |
| SC-008 drop-in IBlockDevice replacement | full IBlockDevice/IBlockDeviceAdmin surface; `unsupported_operations_return_not_supported` confirms NVMe-admin ops → NotSupported |
| SC-009 unit tests pass w/o hardware; integration `#[ignore]` | 2 unit tests/set pass; 13 integration + 2 doctests `#[ignore]`/`ignore` |
| NFR-001..008 | 512-align via O_DIRECT (`NFR-001`); ring depth 128 `DEFAULT_RING_DEPTH` `src/lib.rs:53` (`NFR-002`); errors as `NvmeBlockError` completions, no panic on IO error (`NFR-003`); `// SAFETY:` on all unsafe (`NFR-004`); `unsafe impl Send for KernelHandler` `src/actor.rs:912` (`NFR-005`); `on_idle` true while clients/inflight `:906` (`NFR-006`); `Instant::now()` deadline compare `:804-810` (`NFR-007`); Linux ≥5.1 io_uring (`NFR-008`) |

### Key Entities — aligned ✓

`BlockDeviceKernelComponent` (`src/lib.rs:55-71`), `KernelHandler`
(`src/actor.rs:107-118`), `DeviceConfig` (`src/config.rs:12-83`), `ClientSession`
with unbounded FIFO `pending` (`src/actor.rs:26-39`), `ControlMessage`
(`src/actor.rs:84-88`), `InflightOp` with populated `start: Instant`
(`src/actor.rs:93-104`), and feature-gated `TelemetryStats`
(`src/telemetry.rs:13`) all match their descriptions.

### Not Implemented ✗

None.

## Unspecced Features

None. The `FlushSync` handler that the previous report listed as unspecced is now
covered by FR-027 (backfilled 2026-08-20).

## Recommendations

None outstanding. The one code drift found this sweep (async telemetry latency)
was resolved by the ALIGN fix in `harvest_completions()`; the spec, code, and
`align-tasks.md` are now consistent. Commit this stamped `drift-report.md`
together with the `src/actor.rs` fix and the `spec.md` "Last Synced" update so
the CI Spec-Sync Gate sees a fresh report whose input hash matches the tree.

Deferred (non-blocking, tracked in `align-tasks.md`): (1) the hardware/loopback
telemetry-accuracy test for the async path; (2) low-severity async `tag`
propagation parity with `block-device-filesys` (documented as current behavior in
FR-009).
