---
spec_sync_component: block-device-filesys
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T20:14:00Z
spec_sync_git_commit: 1d8a643d
spec_sync_inputs_sha256: e9bcaa295d50ba2dc0ed67e721132fea0cb520b06e2e1d147def581680fc2314
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report — block-device-filesys

**Generated**: 2026-09-03
**Mode**: Read-only drift analysis, then BACKFILL apply to `spec.md` (code authoritative).

This report supersedes the earlier stale artifact (which read "Generated:
pending", flagged FR-015 as *Drifted*, and listed the `pub(crate)` config
setters as *Unspecced*). Both of those were resolved by the 2026-08-20 Phase B
sync — FR-015 now correctly describes `create()`'s example as ` ```ignore `,
and the setters are specified by FR-023 — so neither is drift against the
**current** spec. This sweep re-verifies the full requirement set against the
current tree and finds one residual drift, resolved by backfill.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (`001-block-device-filesys`) |
| Requirements Checked | 23 FR + 6 SC + 5 Key Entities |
| Aligned | 33 |
| Drifted (this sweep) | 1 → resolved by BACKFILL |
| Not Implemented | 0 |
| Unspecced | 0 |

**Verification runs this sweep** (all green):
- `cargo test -p block-device-filesys -- --test-threads 1` — 13 unit + 15 integration = **28 passed, 0 failed**
- doctests — 2 runnable (`config.rs` `DeviceConfig` line 12, `DeviceConfig::new` line 42), 2 `ignored` (`lib.rs` module example line 17, `create()` line 77) — matches FR-015 exactly
- `cargo clippy -p block-device-filesys -- -D warnings` — clean
- `cargo clippy -p block-device-filesys --features telemetry -- -D warnings` — clean (exercises the FR-019 `record_op` call sites)

## Spec: 001-block-device-filesys — Block Device Filesys Component

### Aligned ✓ (verified this sweep)

| Req | Evidence |
|-----|----------|
| FR-001 implements IBlockDevice | `impl IBlockDevice` `src/lib.rs:224` |
| FR-002 ILogger receptacle + log levels; SQ-full surfaced as error, not logged | receptacle `src/lib.rs:59`; warn on io_uring fallback `src/actor.rs:128`; warn on fsync-SQE-push failure `src/actor.rs:603-605`; SQ-full → error `Completion` (not logged) `src/actor.rs:468-480,588-601`; debug on connect `src/actor.rs:930`, `src/lib.rs:236-238`; info on init `src/lib.rs:130,180` |
| FR-003 `define_component!`/`define_interface!` | `src/lib.rs:54-70` |
| FR-004 config module public; `create`/`initialize`/`shutdown` public | `pub mod config` `src/lib.rs:27`; `src/lib.rs:82,113,187` |
| FR-005 regular-file backing store | `open_or_create_backing_file` `src/config.rs:114` |
| FR-006 `create(path,block_size,num_blocks)`; block_size pow2, min 512 | `src/lib.rs:82`; validated `src/config.rs:59-71` |
| FR-007 sync R/W pread/pwrite, O_DIRECT\|O_SYNC + fdatasync, EINVAL→buffered via `eprintln!` | O_DIRECT\|O_SYNC `src/config.rs:173`; EINVAL fallback `src/config.rs:180-194`; write fdatasync `src/actor.rs:402` |
| FR-008 async R/W via io_uring; write+fsync IO_LINK chain; sync fallback when ring absent | IO_LINK write SQE `src/actor.rs:574-578`, fsync SQE `:580-583`; read fallback `:505-540`; write fallback `:631-677` |
| FR-009 WriteZeros zero-fill + fdatasync | `handle_write_zeros` `src/actor.rs:680`; fdatasync `:737` |
| FR-010 BatchSubmit sequential | `src/actor.rs:238-242` |
| FR-011 AbortOp via io_uring AsyncCancel | `handle_abort` `src/actor.rs:759-772` (AsyncCancel `:761`) |
| FR-012 NsProbe single namespace | `handle_ns_probe` `src/actor.rs:774-786` |
| FR-013 actor model + io_uring event loop | `ActorHandler` impl `src/actor.rs:925-957`; `on_idle` polls/harvests/checks timeouts `:946-956` |
| FR-014 Criterion latency + throughput benches | `Cargo.toml` `[[bench]]`; `benches/{latency,throughput}.rs` |
| FR-015 `DeviceConfig::new` runnable doc example; `create()` illustrative `ignore` | runnable `src/config.rs:42-57`; `ignore` `src/lib.rs:77-81` — confirmed by doctest run (2 pass, 2 ignored) |
| FR-016 fallocate-if-absent, exact-size-open, size-mismatch error | create/fallocate `src/config.rs:151-166`; mismatch error `:126-132` |
| FR-017 direct DmaBuffer slice access, no intermediate copies | `as_slice`/`as_mut_slice` `src/actor.rs:329,378,458,572,634` |
| FR-018 `io-uring` crate dependency (kernel ≥ 5.6) | `Cargo.toml` `io-uring = "0.7"` |
| FR-019 feature-gated atomics TelemetryStats; real per-op latency in ALL paths incl. async harvest | `src/telemetry.rs:13-102`; `start.elapsed()` recorded — sync read `src/actor.rs:352-353`, sync write `:417-418`, read fallback `:535-536`, write fallback `:672-673`, WriteZeros `:752-753`, async completion `:821-823` (start captured at submit `:500,626`) |
| FR-020 non-blocking per-client FIFO backlog | `ClientSession.pending` `src/actor.rs:39`; `deliver` `:60-65`; `flush_pending` `:70-81`; drained in `poll_clients` `:904-906` |
| FR-021 device-info surface (fixed placeholders) | numa_node `-1` `src/lib.rs:300-302`; nvme_version "N/A (file-backed)" `:304-306`; num_io_queues `1` `:288-290`; max_transfer_size `block_size*256` (saturating) `:292-294`; read_write_stats default `:340-342` |
| FR-022 FlushSync → fdatasync; ns_id≠1 → InvalidNamespace w/o touching file; fdatasync fail → WriteFailed | `src/actor.rs:249-276` |
| FR-023 reserved `pub(crate)` config mutators, `#[allow(dead_code)]`, unused | `set_file_path`/`set_block_size`/`set_num_blocks` `src/lib.rs:94-109` |
| SC-001 read-after-write integrity + durability | `tests/integration.rs` `write_sync_read_sync_roundtrip:85`, `data_integrity_multi_block_patterns:424` — pass |
| SC-002 concurrent ops without corruption | multi-block / multi-client `tests/integration.rs:365,424` — pass |
| SC-003 <1ms 4KB sync latency | design/benchmark target (not unit-asserted) |
| SC-004 tests pass w/o hardware/root, temp dir only | 28 tests pass using `tempfile`; no SPDK/hardware |
| SC-005 Criterion CoV < 15% | benchmark quality target |
| SC-006 drop-in IBlockDevice replacement | full `IBlockDevice`/`IBlockDeviceAdmin` surface implemented; `unsupported_operations_return_not_supported` confirms NVMe-admin ops → NotSupported |

### Key Entities — aligned ✓

`BlockDeviceFilesysComponent` (`src/lib.rs:54-70`), `FilesysActor`/`FilesysHandler`
(`src/actor.rs:109`), `DeviceConfig` (`src/config.rs:21-104`), `ClientSession`
with FIFO `pending` (`src/actor.rs:27-40`), and feature-gated `TelemetryStats`
(`src/telemetry.rs:13`) all match their descriptions.

### Drifted ⚠️ → resolved by BACKFILL

- **Edge Cases — "io_uring submission queue is full"** — severity: moderate
  (documentation; spec self-contradiction).
  - Spec (Edge Cases, before this sweep) said: *"The actor MUST back-pressure by
    waiting for completions before submitting new operations."*
  - Actual: on the ReadAsync/WriteAsync hot paths the actor **surfaces the
    condition to the caller** as an error `Completion` — `ReadDone`/`WriteDone`
    carrying `Err(NotInitialized("io_uring submission queue full"))` — and
    returns, never blocking the single-threaded actor
    (`src/actor.rs:468-480` ReadAsync, `src/actor.rs:588-601` WriteAsync).
  - Direction — **code authoritative (BACKFILL)**: this is the shipped, deliberate
    behavior and is *already* specified by FR-002 ("io_uring submission-queue-full
    conditions … are surfaced directly to the caller as an error `Completion` …").
    The Edge Cases bullet was stale prose predating the FR-002 decision, so it
    contradicted another requirement in the same document. Blocking the actor to
    wait would head-of-line-block completion delivery to every other client on the
    device (the FR-020 rationale), so aligning code→spec is not desirable.
  - Backfill: the Edge Cases bullet was rewritten to describe the error-surfacing
    behavior, cross-reference FR-002, and note that `ring.submit()` after each push
    keeps the 128-deep SQ (`DEFAULT_RING_DEPTH`) from filling under normal
    single-op submission (the guard covers a pathological burst).

### Not Implemented ✗

None.

## Unspecced Features

None. The `pub(crate)` config setters (`set_file_path`/`set_block_size`/
`set_num_blocks`) that the previous report listed as unspecced are now covered by
FR-023 (backfilled 2026-08-20) and remain `#[allow(dead_code)]`, reserved,
non-public.

## Recommendations

None outstanding. Commit this stamped `drift-report.md` together with the
`spec.md` Edge Cases backfill so the CI Spec-Sync Gate sees a fresh report whose
input hash matches the tree. No code changes were made or are required this sweep.
