---
spec_sync_component: gpu-services
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-09T20:57:15Z
spec_sync_git_commit: 32539333
spec_sync_inputs_sha256: d8c7ff979e3c0e718c0b982ed7277a956b669eb37367e708bb2e9c1ac8856afc
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: gpu-services

**Generated**: 2026-09-09
**Project**: gpu-services

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 3 |
| Requirements Checked | 78 |
| Aligned | 78 |
| Drifted | 0 (3 resolved by backfill this sync) |
| Not Implemented | 0 |
| Unspecced Features | 0 |

> **Post-apply status (2026-09-09):** the 3 drift items below were resolved in
> this sync by backfilling the spec text (see `apply-report.md` — spec-001
> FR-015 & FR-020, spec-002 FR-019). No actionable drift remains; the findings
> are retained below for the audit trail and marked ✅ RESOLVED.

All three specs remain heavily backfilled from the implementation, so alignment
is high. Since the last sync (spec commit `a93b5620`, 2026-08-20) two commits
touched `components/gpu-services/src/`:

- `6d7ba234` (2026-08-30) — *revert dispatcher GPU experiments to specified
  stream model*: removed ~194 lines of unspecced experimental code from
  `lib.rs`, moving the code **toward** the specified interface. No drift
  introduced.
- `ecf2dbe3` (2026-09-02) — *data-parallel (shared-server) 2-way DP arm*
  (Gate #1): switched the host-pinned allocation/registration sites to CUDA
  **portable** flags so a pool pinned under one device's context stays
  page-locked/zero-copy for transfers to every GPU. This is a deliberate,
  load-bearing behavior for multi-GPU data-parallel serving that the specs do
  not yet describe → the drift below.

Additionally, spec-003 FR-012 (flagged as drifted in the previous report) was
softened on 2026-08-20 to state the MDTS ceiling is an operator responsibility.
The current spec text matches the code, so FR-012 is now **aligned**; the prior
report's FR-012 finding is stale and is cleared here.

## Detailed Findings

### 001-gpu-cuda-services — GPU CUDA Services

FR-001..FR-025 and SC-001..SC-008. Interface `IGpuServices` is defined in
`components/interfaces/src/igpu_services.rs` and implemented in
`components/gpu-services/src/lib.rs`.

- ✓ FR-001..FR-003 (init/shutdown/device discovery): `initialize` lib.rs:98,
  `shutdown` lib.rs:129 (returns `Ok` when gpu feature off),
  `get_devices` lib.rs:150; discovery filters compute major >= 7 in
  `src/device.rs`, collects `GpuDeviceInfo` incl. `pci_bus_id`.
- ✓ FR-004 (memory attribute check via `cudaPointerGetAttributes`):
  `src/memory.rs` `check_memory_attributes`.
- ✓ IPC handling (deserialize/verify/pin/unpin): `deserialize_ipc_handle`
  lib.rs:166, `verify_memory` lib.rs:192, `pin_memory` lib.rs:215,
  `unpin_memory` lib.rs:249 (tracking-removal only, never `cudaHostUnregister`
  — matches backfilled FR-005); payload decode in `src/ipc.rs` (72-byte
  validation), `open_ipc_handle` using `CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS`.
- ✓ DMA buffer + copy ops: `create_dma_buffer` lib.rs:269,
  `dma_copy_to_host` lib.rs:301, `dma_copy_to_device` lib.rs:511;
  async variants lib.rs:725/779/823/877.
- ✓ Stream ops: `create_stream` lib.rs:561, `destroy_stream` lib.rs:657,
  `stream_query` lib.rs:678, `stream_synchronize` lib.rs:702;
  `set_device` lib.rs:588, `device_of_ptr` lib.rs:616. (Post-`6d7ba234` the
  code is back on the specified stream model with no extra unspecced methods.)
- ✓ FR-008 (p2p module helpers as deliberate interface-only exception): matches
  `src/dma.rs` public helpers.
- ✓ FR-014 (`shutdown()` no-op returns `Ok` when gpu feature off): lib.rs:129.
- ✓ SC-008 / US6 demo apps: `apps/gpu-handle-test-server`,
  `apps/gpu-handle-test-client` exist.

- ✅ **FR-015** RESOLVED (was: minor under-documented behavior): `register_host_memory`.
  - Spec: "page-locks the specified host memory region via `cudaHostRegister`
    ... If `cudaHostRegister` succeeds but `spdk_mem_register` fails, the method
    MUST roll back by calling `cudaHostUnregister`."
  - Actual (lib.rs:960-1010): now calls
    `cudaHostRegister(ptr, size, CUDA_HOST_REGISTER_PORTABLE)` (lib.rs:979,
    changed from flag `0` in `ecf2dbe3`) so the shared memory-tier pool stays
    pinned across **all** GPU contexts. Also carries idempotent hardening the
    spec does not mention: `cudaHostRegister` returning
    `CUDA_ERROR_HOST_MEMORY_ALREADY_REGISTERED` (712) is treated as success
    (lib.rs:982-990); SPDK `rc == -16` (EBUSY, already registered) is treated as
    success (lib.rs:1000-1007); and the `cudaHostUnregister` rollback fires only
    when this call actually performed the CUDA registration (`we_registered_cuda`,
    lib.rs:981,1003) rather than unconditionally.
  - Location: `components/gpu-services/src/lib.rs:979`,
    `components/gpu-services/src/cuda_ffi.rs:80`
  - Severity: minor (code behavior is correct and desired; spec under-documented
    the portable-pinning semantics and idempotent paths).
  - Resolution: **backfilled** into spec-001 FR-015 this sync.

- ✅ **FR-020** RESOLVED (was: minor under-documented behavior):
  `allocate_pinned_dma_buffer`.
  - Spec: "allocates page-locked host memory via `cudaHostAlloc`".
  - Actual (lib.rs:937-939): now calls
    `cudaHostAlloc(&mut host_ptr, size, CUDA_HOST_ALLOC_PORTABLE)` (changed from
    `CUDA_HOST_ALLOC_DEFAULT` in `ecf2dbe3`) so the returned buffer is portable
    across GPU contexts for multi-GPU H2D copies. Spec does not mention the
    portable flag.
  - Location: `components/gpu-services/src/lib.rs:938`,
    `components/gpu-services/src/cuda_ffi.rs:74`
  - Severity: minor.
  - Resolution: **backfilled** into spec-001 FR-020 this sync.

Remaining FR-001..FR-014, FR-016..FR-019, FR-021..FR-025 and SC-001..SC-008:
aligned.

### 002-gpu-ssd-dma-prepare — GPU-to-SSD DMA Buffer Preparation

FR-001..FR-024 and SC-001..SC-005.

- ✓ FR-001..FR-015 (`prepare_memory_for_spdk` pipeline): implemented at
  lib.rs:351; opens IPC handle with lazy peer access, checks internal pinned
  `HashSet` (FR-003), conditional pin, returns SPDK `DmaBuffer` with pin-aware
  free function; device-context set/restore (FR-014/FR-018).
- ✓ FR-016/FR-017 (`spdk_mem_register`/rollback): in the prepare path and
  `src/dma.rs`.
- ✓ FR-020 (`unregister_host_memory`): lib.rs:1013.
- ✓ FR-021 (`create_spdk_dma_buffer_from_gpu_bar`, GDRCopy): `src/dma.rs`.
- ✓ FR-022 (`_from_phys`), FR-023 (`_from_bar_direct`): `src/dma.rs`.
- ✓ FR-024 (GDRCopy FFI + `GPU_PAGE_SIZE`): `src/gdrcopy_ffi.rs`.
- ✓ Auxiliary helpers (backfilled 2026-08-07):
  `create_spdk_dma_buffer_from_cuda_malloc`, `get_phys_addr` — spec-tracked.

- ✅ **FR-019** RESOLVED (was: minor under-documented behavior):
  `register_host_memory` (the
  same method surfaced on this spec). Same drift as spec-001 FR-015: the
  `cudaHostRegister` call now passes `CUDA_HOST_REGISTER_PORTABLE` (lib.rs:979),
  and the idempotent `ALREADY_REGISTERED` / SPDK-`EBUSY` / conditional-rollback
  paths are not described. FR-019's rollback clause ("On partial failure (CUDA
  succeeds, SPDK fails), MUST roll back `cudaHostRegister`") still holds for the
  genuine-success case.
  - Location: `components/gpu-services/src/lib.rs:979`
  - Severity: minor.
  - Resolution: **backfilled** into spec-002 FR-019 this sync (kept consistent
    with spec-001 FR-015).

Remaining FR-001..FR-018, FR-020..FR-024 and SC-001..SC-005: aligned.

### 003-gpu-p2p-server — GPU P2P Server

FR-001..FR-012 and SC-001..SC-004. Backfilled from code 2026-07-22
("needs human review"). Implemented in `src/bin/p2p_server.rs`.

- ✓ FR-001 (built only under `p2p` feature): `Cargo.toml` bin
  `gpu-p2p-server` `required-features = ["p2p"]`.
- ✓ FR-002 (CLI args + defaults): `Cli` struct `src/bin/p2p_server.rs:~40-56`.
- ✓ FR-003 (nvidia_peermem + gdrdrv check), FR-004 (SPDK init),
  FR-005 (p2p staging pool): `initialize_stack` / `create_chunk_pool`
  `src/bin/p2p_server.rs:251`.
- ✓ FR-006..FR-010 (UDS listen, per-connection payload read, mode dispatch,
  response line, `--once`): `handle_bounce` :377, `handle_p2p` :451,
  `handle_p2p_cold`, non-blocking accept loop.
- ✓ FR-011 (SIGINT/SIGTERM handlers, cleanup, socket removal):
  `signal_handler` + accept-loop flag check.
- ✓ **FR-012** (now aligned): reads performed in `--chunk-size` increments —
  `do_chunked_read` (`src/bin/p2p_server.rs:273`) issues one async `ReadAsync`
  per chunk and submits via `BatchSubmit`. FR-012 was softened on 2026-08-20 to
  state the MDTS ceiling is an operator responsibility conveyed via CLI help
  ("must not exceed MDTS, typically 128KB", `src/bin/p2p_server.rs:54`) and the
  131072-byte default, not a runtime-validated constraint. Code matches the
  softened spec, so this is no longer drift (the previous report's FR-012 ⚠️
  finding is cleared).

## Unspecced Features

None. `6d7ba234` removed the earlier unspecced GPU stream experiments; the
`src/dma.rs` helpers and `src/gdrcopy_ffi.rs` constants remain backfilled into
spec 002.

## Inter-Spec Conflicts

None. (The historical FR-008-vs-002/003 conflict was resolved in the 2026-08-07
sync and remains resolved.)

## Recommendations

1. ✅ **DONE** — Backfilled the portable host-pinning behavior into the specs
   (spec-001 FR-015 & FR-020, spec-002 FR-019): host memory is
   registered/allocated with the CUDA *portable* flag
   (`CUDA_HOST_REGISTER_PORTABLE` / `CUDA_HOST_ALLOC_PORTABLE`) so the shared
   pool stays pinned/zero-copy across all GPU contexts (multi-GPU DP; Gate #1,
   `ecf2dbe3`).
2. ✅ **DONE** — Documented the idempotent registration paths in FR-015/FR-019:
   `cudaHostRegister` → `ALREADY_REGISTERED` and `spdk_mem_register` → `EBUSY`
   treated as success, and `cudaHostUnregister` rollback only when this call
   performed the CUDA registration.
3. Spec 003 is still marked "needs human review" — a maintainer pass would let
   it graduate from backfilled-draft status. (Not drift; not blocking.)
