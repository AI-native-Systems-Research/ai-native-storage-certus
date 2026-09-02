---
spec_sync_component: gpu-services
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-02T21:41:07Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 4bd92194929d13b040f5f14d94fba04a56a6b878203e64af3438fcfef654083c
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report: gpu-services

**Generated**: 2026-09-02
**Project**: gpu-services

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 3 |
| Requirements Checked | 78 |
| Aligned | 78 |
| Drifted | 0 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

All three specs are fully aligned with the implementation. The single minor
drift carried by prior rounds (spec 003 FR-012, MDTS ceiling) was backfilled
into the spec on 2026-08-20 — the current spec 003 FR-012 text already states
the MDTS bound is an operator responsibility documented via CLI help, not a
runtime-validated constraint, which matches the code. This fresh 2026-09-02
re-verification confirms that resolution still holds and found no new drift.

Interface `IGpuServices` is defined in
`components/interfaces/src/igpu_services.rs` and implemented in
`components/gpu-services/src/lib.rs`. Every FR/SC below was checked against
the source with concrete file:line evidence.

## Detailed Findings

### 001-gpu-cuda-services — GPU CUDA Services

FR-001..FR-025 and SC-001..SC-008 (33 items). All Aligned.

- ✓ FR-001..FR-003 (init/shutdown/device discovery): `initialize` lib.rs:98
  (idempotent, sets `initialized` + atomic mirror), `shutdown` lib.rs:129
  (returns `Ok` no-op when gpu feature off), `get_devices` lib.rs:150.
  Discovery filters `prop.major < 7` in `src/device.rs:39`, collects
  `GpuDeviceInfo` incl. `pci_bus_id` (`src/device.rs:60-67`), errors when no
  qualifying GPU (`src/device.rs:70-72`).
- ✓ FR-003 precondition (caller sets device context; `open_ipc_handle` does
  not call `cudaSetDevice`): confirmed — `src/ipc.rs:42-71` has no
  `cudaSetDevice`. `deserialize_ipc_handle` lib.rs:166.
- ✓ FR-004 (device-memory check via `cudaPointerGetAttributes`, verified set):
  `src/memory.rs:8-34` (`CUDA_MEMORY_TYPE_DEVICE` check :26), tracked in
  `state.verified` (lib.rs:205).
- ✓ FR-005 (pin/unpin, idempotent, tracking-only unpin): `pin_memory`
  lib.rs:215 (early-return if already pinned :227; may skip re-verify if in
  verified set :238-240); `unpin_memory` lib.rs:249 removes tracking only,
  errors if not pinned (:261-263), never calls `cudaHostUnregister`.
- ✓ FR-006 (`GpuDmaBuffer` from verified+pinned handle): `create_dma_buffer`
  lib.rs:269 (gates on verified :281 and pinned :284), `create_gpu_dma_buffer`
  `src/dma.rs:727`.
- ✓ FR-007 (descriptive errors, no panic/leak): all methods return
  `Result<_, String>`; rollback paths present in prepare/register flows.
- ✓ FR-008 (interface exposure + documented p2p-module exception): interface
  methods in `components/interfaces/src/igpu_services.rs`; the `pub` `dma`
  module builders are the deliberate exception the spec records.
- ✓ FR-009/FR-010 (feature-gated build, tests + benches): `Cargo.toml`
  `gpu = [...]`; benches `gpu_services_benchmark`/`dma_transfer_benchmark`
  (`required-features = ["gpu"]`); tests in lib.rs:1049-1302.
- ✓ FR-011/FR-012 (`dma_copy_to_host`/`dma_copy_to_device`, spdk-gated):
  lib.rs:301 / lib.rs:511 (`cudaMemcpy` D2H/H2D, size checks).
- ✓ FR-013 (`prepare_memory_for_spdk`): lib.rs:351 (see spec 002).
- ✓ FR-014 (gpu-off methods error; `shutdown` no-op Ok): every method has a
  `#[cfg(not(feature = "gpu"))]` error arm; `shutdown` returns `Ok(())`
  (lib.rs:130-133).
- ✓ FR-015/FR-016 (`register_host_memory`/`unregister_host_memory`, rollback):
  lib.rs:960 (rolls back `cudaHostUnregister` on SPDK failure :999-1001),
  lib.rs:1010 (unregister SPDK then CUDA).
- ✓ FR-017 (stream lifecycle): `create_stream` lib.rs:561, `destroy_stream`
  lib.rs:657, `stream_synchronize` lib.rs:702.
- ✓ FR-018/FR-019 (`dma_copy_to_device_async`/`memcpy_h2d_async`): lib.rs:725
  / lib.rs:779 (`cudaMemcpyAsync` H2D on `GpuStream`).
- ✓ FR-020 (`allocate_pinned_dma_buffer`): lib.rs:920 (`cudaHostAlloc` +
  `create_spdk_dma_buffer_from_cuda_host_alloc`).
- ✓ FR-021/FR-022 (`set_device`/`device_of_ptr`): lib.rs:588 / lib.rs:616
  (returns `-1` for non-device pointer :649-653).
- ✓ FR-023 (`stream_query`, non-blocking): lib.rs:678 (`Ok(true)` on success,
  `Ok(false)` on `CUDA_ERROR_NOT_READY`).
- ✓ FR-024/FR-025 (`dma_copy_to_host_async`/`memcpy_d2h_async`): lib.rs:823
  / lib.rs:877 (`cudaMemcpyAsync` D2H; size check on the buffer variant).
- ✓ SC-001..SC-005 (latency targets): single-call paths present; unenforceable
  offline but no code contradicts them.
- ✓ SC-006/SC-007 (tests/benches): present per FR-010.
- ✓ SC-008 / US6 demo: `apps/gpu-handle-test-server` and
  `apps/gpu-handle-test-client` both exist.

### 002-gpu-ssd-dma-prepare — GPU-to-SSD DMA Buffer Preparation

FR-001..FR-024 and SC-001..SC-005 (29 items). All Aligned.

- ✓ FR-001 (`prepare_memory_for_spdk(&str, Option<u32>) -> DmaBuffer`):
  lib.rs:351; interface signature `igpu_services.rs:499`.
- ✓ FR-002/FR-011 (lazy peer access): `CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS`
  passed to `cudaIpcOpenMemHandle` (`src/ipc.rs:55`).
- ✓ FR-003 (pin-state from internal `HashSet`, not
  `cudaPointerGetAttributes`): `state.pinned.contains(&(ptr as usize))`
  lib.rs:437.
- ✓ FR-004 (conditional pin): lib.rs:441-470.
- ✓ FR-005 (log pin decisions): lib.rs:465-470 (pinned) / :468-470 (already
  pinned).
- ✓ FR-006/FR-007/FR-008 (pin-aware free fn variants, both close IPC handle):
  `create_spdk_dma_buffer_from_gpu` selects `spdk_unregister_unpin_and_ipc_close`
  vs `spdk_unregister_and_ipc_close` (`src/dma.rs:136-140`); both close the
  handle (`src/dma.rs:80,100`).
- ✓ FR-009 (error if uninitialized): lib.rs:366.
- ✓ FR-010 (error if IPC open fails): lib.rs:415-421.
- ✓ FR-012 (no leak on error): rollback closes IPC handle / removes tracking
  on every error arm (lib.rs:409-489).
- ✓ FR-013 (spdk-gated): `#[cfg(feature = "spdk")]` lib.rs:351.
- ✓ FR-014/FR-018 (set device context; restore on success+error): lib.rs:372-404,
  `restore_device` invoked on all paths (:410,:418,:434,:448,:487,:496).
- ✓ FR-015 (returns SPDK `DmaBuffer`): return type lib.rs:356.
- ✓ FR-016/FR-017 (`spdk_mem_register` + rollback `spdk_mem_unregister`):
  `src/dma.rs:122-128` (register), `:151-159` (rollback on `from_raw` failure).
- ✓ FR-019/FR-020 (`register_host_memory`/`unregister_host_memory`): lib.rs:960
  / lib.rs:1010.
- ✓ FR-021 (`create_spdk_dma_buffer_from_gpu_bar`, GDRCopy pin/map/register):
  `src/dma.rs:353` (gdr_open/pin/map + spdk_mem_register; full-cleanup free fn
  `src/dma.rs:318`).
- ✓ FR-022 (`create_spdk_dma_buffer_from_phys`, mmap+extmem+VFIO): `src/dma.rs:547`
  (free fn `vfio_unmap_extmem_munmap` :497).
- ✓ FR-023 (`create_spdk_dma_buffer_from_bar_direct`, identity VA→IOVA, no
  munmap on drop): `src/dma.rs:635` (free fn `vfio_unmap_extmem_only` :708).
- ✓ FR-024 (GDRCopy FFI + `GPU_PAGE_SIZE` 64 KiB): `src/gdrcopy_ffi.rs`
  (`GPU_PAGE_SHIFT=16`, `GPU_PAGE_SIZE=1<<16`, all 6 fns `gdr_open/close/
  pin_buffer/unpin_buffer/map/unmap`).
- ✓ Auxiliary Public Helpers (backfilled 2026-08-07):
  `create_spdk_dma_buffer_from_cuda_malloc` `src/dma.rs:189`, `get_phys_addr`
  `src/dma.rs:522`, `GPU_PAGE_SHIFT` `src/gdrcopy_ffi.rs:16` — all present and
  spec-tracked.
- ✓ SC-001..SC-005: single-call pipeline, pin-state-aware cleanup, logging,
  no-leak error paths, interface-consistent — all satisfied by the above.

### 003-gpu-p2p-server — GPU P2P Server

FR-001..FR-012 and SC-001..SC-004 (16 items). All Aligned.
Implemented in `src/bin/p2p_server.rs`.

- ✓ FR-001 (`p2p`-feature bin): `Cargo.toml` `[[bin]] name = "gpu-p2p-server"`,
  `required-features = ["p2p"]`.
- ✓ FR-002 (CLI args + defaults): `Cli` struct `src/bin/p2p_server.rs:35-61`
  — `--socket` (`/tmp/gpu_p2p_server.sock`), `--pci` (Option), `--mode`
  (default `p2p`), `--staging-size` (4194304), `--chunk-size` (131072),
  `--once`.
- ✓ FR-003 (nvidia_peermem + gdrdrv check before socket bind): `initialize_stack`
  `:117-122` via `/proc/modules`, called from `main` before `UnixListener::bind`.
- ✓ FR-004 (SPDK/GPU/NVMe init + `atexit` `_exit(0)`): `initialize_stack`
  `:106-223` (`atexit(exit_hook)` :112-115).
- ✓ FR-005 (pre-allocate p2p staging pool before accept): `create_chunk_pool`
  `:251` invoked in `main` `:582-604` before the accept loop.
- ✓ FR-006 (UDS listen, remove stale file, non-blocking accept @100us):
  `:619-627` (remove + bind + `set_nonblocking`), `:666-668` (WouldBlock
  sleeps 100us).
- ✓ FR-007 (read one base64 72-byte line, open IPC handle): `parse_client_payload`
  `:325-348`, `open_ipc_handle` `:350-371`.
- ✓ FR-008 (mode dispatch): `main` `:642-652` → `handle_bounce` `:374`,
  `handle_p2p` `:436`, `handle_p2p_cold` `:493`.
- ✓ FR-009 (`OK <size> bytes (<mode>, <n> chunks)` / `ERROR: <msg>` + stderr,
  no crash): handlers return the `OK ...` string (`:432,:489,:563`), `main`
  `:653-661` writes OK or `ERROR:` and logs to stderr.
- ✓ FR-010 (`--once` serves one then exits + removes socket): `:662-664`,
  `:677`.
- ✓ FR-011 (SIGINT/SIGTERM flag, accept loop checks, drop pool + remove
  socket): `signal_handler` `:96-98`, flag check `:635-638`, cleanup
  `:676-677`.
- ✓ FR-012 (chunked reads in `--chunk-size` increments; MDTS = operator
  responsibility): `do_chunked_read` `:273-323` (one `ReadAsync` per chunk,
  `sectors_per_chunk = chunk_size / sector_size`, `BatchSubmit`, await all
  completions). MDTS is conveyed via `--chunk-size` CLI help (`:54`) + 131072
  default only — spec 003 FR-012 (lines 193-204) now states this is an
  operator responsibility, not a runtime constraint, so code and spec agree.
- ✓ SC-001..SC-004: three-mode transfer, `--once`, clean signal shutdown,
  malformed-payload → `ERROR:` (never crash) — all satisfied by FR-006..FR-011
  evidence above.

## Unspecced Features

None. Every `pub` helper in `src/dma.rs` and `src/gdrcopy_ffi.rs` is
spec-tracked: the FR-numbered builders (spec 002 FR-021..024), the documented
p2p-module exception (spec 001 FR-008, which names `_from_gpu`,
`_from_cuda_malloc`, `_from_cuda_host_alloc`, `_from_gpu_bar`, `_from_phys`,
`_from_bar_direct`, `get_phys_addr`), and the Auxiliary Public Helpers section
of spec 002. The free-function symbols are cleanup mechanisms of those
builders, covered by spec 002 FR-006..008 and the DmaBuffer Key Entity.

## Recommendations

- Spec 003 is still marked "Draft (backfilled — needs human review)". A
  maintainer pass would let it graduate from backfilled-draft status; no
  behavioral change is required.
- No spec edits or ALIGN tasks were required this run.
