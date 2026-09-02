---
spec_sync_component: block-device-filesys
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-02T21:29:01Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 77e3ac263848b9b84203516b88da09854d79810744c3d273643c3cbfd88f506a
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report — block-device-filesys

**Generated**: 2026-09-02 (spec-sync re-run)

Read-only spec↔implementation drift analysis. Sources: `specs/001-block-device-filesys/{spec.md,plan.md,data-model.md,contracts/}` vs `src/{lib.rs,actor.rs,config.rs,telemetry.rs}`, `Cargo.toml`, and `tests/integration.rs`; interface context from `components/interfaces/src/iblock_device.rs` (read-only).

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 29 (FR-001..023, SC-001..006) |
| Aligned | 29 |
| Drifted (FR/SC) | 0 |
| Not Implemented | 0 |
| Unspecced | 0 |
| Doc-drift found & backfilled | 6 |

## Detailed Findings

### Spec 001-block-device-filesys — Block Device Filesys Component

#### Aligned ✓ (all 29 FR/SC)

- **FR-001** IBlockDevice implemented — `src/lib.rs:224`.
- **FR-002** ILogger receptacle (`src/lib.rs:59`); warn on io_uring fallback (`src/actor.rs:128`) and fsync-SQE-push failure (`src/actor.rs:603`); SQ-full surfaced as error `Completion` not logged (`src/actor.rs:469,595`); debug on connect (`src/lib.rs:237`, `src/actor.rs:930`) / disconnect (`src/actor.rs:936`); info on init (`src/lib.rs:130,180`).
- **FR-003** `define_component!` used — `src/lib.rs:54`.
- **FR-004** `config` module public; `create`/`initialize`/`shutdown` public — `src/lib.rs:27,82,113,187`.
- **FR-005** regular-file backing store — `src/config.rs:114`.
- **FR-006** block_size/num_blocks via `create()`, pow2 & min 512 validated (num_blocks>0 too) — `src/config.rs:58-71`.
- **FR-007** sync R/W pread/pwrite, O_DIRECT|O_SYNC + fdatasync, buffered fallback (no O_SYNC) on EINVAL via `eprintln!` — `src/config.rs:173,180-193`; fdatasync `src/actor.rs:402`.
- **FR-008** async R/W via io_uring, write+fsync IO_LINK chain, sync fallback when ring `None` — `src/actor.rs:570-583,631`.
- **FR-009** WriteZeros zero-fill + fdatasync — `src/actor.rs:680,724,737`.
- **FR-010** BatchSubmit sequential — `src/actor.rs:238-242`.
- **FR-011** AbortOp via io_uring AsyncCancel — `src/actor.rs:759-772`.
- **FR-012** NsProbe single namespace — `src/actor.rs:774-786`.
- **FR-013** actor model / io_uring `on_idle` loop — `src/actor.rs:925-957`.
- **FR-014** Criterion latency + throughput benches — `Cargo.toml [[bench]]`; `benches/{latency,throughput}.rs`.
- **FR-015** `DeviceConfig::new` runnable ` ``` ` example (`src/config.rs:42-57`); `create()` intentionally ` ```ignore ` (`src/lib.rs:77-81`). **Now aligned** — was the sole drift in the 2026-08-20 report; the spec text now matches the intentional `ignore` example.
- **FR-016** fallocate-if-absent, exact-size-open, size-mismatch error — `src/config.rs:114-166`.
- **FR-017** direct DmaBuffer slice access (`as_slice`/`as_mut_slice`, no intermediate copies) — `src/actor.rs`.
- **FR-018** `io-uring` 0.7 dependency — `Cargo.toml:19`.
- **FR-019** feature-gated atomics TelemetryStats; latency recorded from per-op `start.elapsed()` in ALL paths incl. async completion — `src/telemetry.rs:35`; `src/actor.rs:353,418,536,672,753,822-823`.
- **FR-020** non-blocking per-client FIFO backlog `pending` with `deliver`/`flush_pending` — `src/actor.rs:39,60,70,904`.
- **FR-021** device-info surface: numa_node -1, nvme_version "N/A (file-backed)", num_io_queues 1, max_transfer_size block_size*256, read_write_stats default — `src/lib.rs:288-306,340`.
- **FR-022** FlushSync → fdatasync; ns_id!=1 → InvalidNamespace w/o touching file; fdatasync failure → WriteFailed — `src/actor.rs:249-276`. Interface has `FlushSync`/`FlushDone` — `components/interfaces/src/iblock_device.rs:411,501`.
- **FR-023** reserved `pub(crate)` `#[allow(dead_code)]` setters present, unused — `src/lib.rs:95-109`. Documented as intentional; no longer "unspecced".
- **SC-001** read-after-write integrity + durability — `tests/integration.rs:85,424`.
- **SC-002** 100 concurrent ops — multi-block/multi-client tests `tests/integration.rs:365,424`.
- **SC-003** <1ms 4KB sync latency — design/benchmark target (not unit-asserted).
- **SC-004** tests pass without hardware/root, temp dir only — `tests/integration.rs` uses `tempfile`.
- **SC-005** Criterion CoV<15% — benchmark quality target.
- **SC-006** drop-in IBlockDevice replacement — interface parity with spdk-nvme confirmed (`components/interfaces/src/iblock_device.rs:569-589`).

#### Drifted ⚠️ (FR/SC)

None. FR-015 (the only drift in the 2026-08-20 report) is resolved: the spec now matches the intentional ` ```ignore ` `create()` example (`src/lib.rs:77`).

#### Not Implemented ✗

None.

## Doc-Drift (supporting artifacts) — found & resolved via BACKFILL

These are stale-documentation divergences in the supporting spec artifacts against
working, tested code. All were backfilled this run (docs updated to match code); none
required a code change.

| ID | Artifact / location | Spec said | Code does | Evidence | Sev |
|---|---|---|---|---|---|
| EDGE-SQ-FULL | spec.md — io_uring SQ-full edge case | actor MUST back-pressure by waiting | surfaces error `Completion` (`Err(NotInitialized("io_uring submission queue full"))`) to caller (matches FR-002) | `src/actor.rs:469-480,588-601` | minor |
| DM-FILE-PATH-TYPE | data-model.md — component field | `file_path: RwLock<..>` | `Mutex<Option<PathBuf>>` | `src/lib.rs:62` | minor |
| DM-PROVIDES | data-model.md — Provides | `[IBlockDevice]` | `[IBlockDevice, IBlockDeviceAdmin]` | `src/lib.rs:57` | minor |
| DM-RING-TYPE | data-model.md — FilesysActor | `ring: IoUring`; struct FilesysActor; no shutdown/telemetry fields | `ring: Option<IoUring>`; struct `FilesysHandler`; `shutdown_requested`, feature-gated `telemetry` | `src/actor.rs:109-120` | minor |
| DM-INFLIGHT-START | data-model.md — InflightOp | `start_ns: u64` ~0, never read, "see align-tasks (latency defect)" | `start: Instant`, `start.elapsed()` recorded on completion; defect fixed (FR-019) | `src/actor.rs:103,822-823` | moderate |
| DM-CONFIGURED-STATE | data-model.md — Configured state | `set_file_path/set_block_size/set_num_blocks` called | config supplied at construction via `create(...)`; `set_*` are reserved dead code (FR-023) | `src/lib.rs:82-109` | minor |

## Unspecced Code

None. The former `set_*` internal setters are now documented as FR-023.

## Recommendations

1. **Optional code cleanup (not required)**: the `set_*` mutators (`src/lib.rs:95-109`) remain dead `pub(crate)` `#[allow(dead_code)]` code. FR-023 records them as intentional/reserved; a future decision could remove them or wire them into a reconfiguration path (with set-time validation, which they currently lack). Out of scope for this sync (no `.rs` edits).
2. Contracts doc (`contracts/iblock-device-contract.md`) predates `FlushSync` and `read_write_stats`; these are covered by FR-022/FR-021 in spec.md. Left as-is (design-time contract snapshot); could be refreshed in a future doc pass.
3. Otherwise the component is fully aligned; all documentation now matches the tested implementation.
