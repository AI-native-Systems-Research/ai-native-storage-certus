# Drift Report: gpu-services

**Generated**: pending
**Project**: gpu-services

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 3 |
| Requirements Checked | 78 |
| Aligned | 77 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

All three specs are heavily backfilled from the implementation (dated
2026-07-22 / 2026-08-07 notes throughout), so alignment is high. A single
minor drift remains: the MDTS ceiling in spec 003 FR-012 is documented in CLI
help text but not enforced at runtime.

## Detailed Findings

### 001-gpu-cuda-services — GPU CUDA Services

FR-001..FR-025 and SC-001..SC-008. All Aligned. Interface `IGpuServices` is
defined in `components/interfaces/src/igpu_services.rs` and implemented in
`components/gpu-services/src/lib.rs`.

- ✓ FR-001..FR-003 (init/shutdown/device discovery): `initialize` lib.rs:98,
  `shutdown` lib.rs:129 (returns `Ok` when gpu feature off),
  `get_devices` lib.rs:150; discovery filters compute major >= 7 in
  `src/device.rs:39`, collects `GpuDeviceInfo` incl. `pci_bus_id`.
- ✓ FR-004 (memory attribute check via `cudaPointerGetAttributes`):
  `src/memory.rs:8` `check_memory_attributes`, device-type check at
  `src/memory.rs:26`.
- ✓ IPC handling (deserialize/verify/pin/unpin): `deserialize_ipc_handle`
  lib.rs:166, `verify_memory` lib.rs:192, `pin_memory` lib.rs:215,
  `unpin_memory` lib.rs:249; payload decode in `src/ipc.rs:11` (72-byte
  validation), `open_ipc_handle` `src/ipc.rs:42` using
  `CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS`.
- ✓ DMA buffer + copy ops: `create_dma_buffer` lib.rs:269,
  `dma_copy_to_host` lib.rs:301, `dma_copy_to_device` lib.rs:511;
  async variants lib.rs:725/779/823/877.
- ✓ Stream ops: `create_stream` lib.rs:561, `destroy_stream` lib.rs:657,
  `stream_query` lib.rs:678, `stream_synchronize` lib.rs:702;
  `set_device` lib.rs:588, `device_of_ptr` lib.rs:616.
- ✓ Pinned buffer + host registration: `allocate_pinned_dma_buffer`
  lib.rs:920, `register_host_memory` lib.rs:960, `unregister_host_memory`
  lib.rs:1010.
- ✓ FR-008 (p2p module helpers as deliberate interface-only exception): noted
  in spec, matches `src/dma.rs` public helpers.
- ✓ SC-008 / US6 demo apps: `apps/gpu-handle-test-server`,
  `apps/gpu-handle-test-client` exist.

### 002-gpu-ssd-dma-prepare — GPU-to-SSD DMA Buffer Preparation

FR-001..FR-024 and SC-001..SC-005. All Aligned.

- ✓ FR-001..FR-015 (`prepare_memory_for_spdk` pipeline): implemented at
  lib.rs:351; opens IPC handle with lazy peer access, checks internal pinned
  `HashSet` (FR-003), conditional pin, returns SPDK `DmaBuffer` with pin-aware
  free function; device-context set/restore (FR-014/FR-018).
- ✓ FR-016/FR-017 (`spdk_mem_register`/rollback): in the prepare path and
  `src/dma.rs`.
- ✓ FR-019/FR-020 (host memory register/unregister): `register_host_memory`
  lib.rs:960, `unregister_host_memory` lib.rs:1010.
- ✓ FR-021 (`create_spdk_dma_buffer_from_gpu_bar`, GDRCopy): `src/dma.rs:353`.
- ✓ FR-022 (`_from_phys`): `src/dma.rs:547`; FR-023 (`_from_bar_direct`):
  `src/dma.rs:635`.
- ✓ FR-024 (GDRCopy FFI + `GPU_PAGE_SIZE`): `src/gdrcopy_ffi.rs`
  (`GPU_PAGE_SHIFT=16`, `GPU_PAGE_SIZE=1<<16`, all 6 GDRCopy fns declared).
- ✓ Auxiliary helpers (backfilled 2026-08-07):
  `create_spdk_dma_buffer_from_cuda_malloc` `src/dma.rs:189`, `get_phys_addr`
  `src/dma.rs:522` — spec-tracked, not unspecced.

### 003-gpu-p2p-server — GPU P2P Server

FR-001..FR-012 and SC-001..SC-004. Backfilled from code 2026-07-22
("needs human review"). Implemented in `src/bin/p2p_server.rs`.

- ✓ FR-001 (built only under `p2p` feature): `Cargo.toml` bin
  `gpu-p2p-server` `required-features = ["p2p"]`.
- ✓ FR-002 (CLI args + defaults): `Cli` struct `src/bin/p2p_server.rs:~40`.
- ✓ FR-003 (nvidia_peermem + gdrdrv check), FR-004 (SPDK init),
  FR-005 (p2p staging pool): `initialize_stack` / `create_chunk_pool`
  `src/bin/p2p_server.rs:251`.
- ✓ FR-006..FR-010 (UDS listen, per-connection payload read, mode dispatch,
  response line, `--once`): `handle_bounce` :373, `handle_p2p` :435,
  `handle_p2p_cold`, non-blocking accept loop.
- ✓ FR-011 (SIGINT/SIGTERM handlers, cleanup, socket removal):
  `signal_handler` + accept-loop flag check.

- ⚠️ **FR-012** (minor): NVMe reads in `--chunk-size` increments *not exceeding
  the NVMe controller's MDTS*.
  - Spec: reads MUST be performed in `--chunk-size` increments not exceeding
    the controller's MDTS.
  - Actual: chunking is implemented (`do_chunked_read`
    `src/bin/p2p_server.rs:273`, `num_chunks` loops), but the MDTS ceiling is
    only conveyed via the CLI doc comment ("must not exceed MDTS, typically
    128KB", `src/bin/p2p_server.rs:54`); there is no runtime validation of
    `chunk_size` against the device MDTS.
  - Location: `components/gpu-services/src/bin/p2p_server.rs:54,273`
  - Severity: minor

## Unspecced Features

None. Auxiliary `src/dma.rs` helpers and `src/gdrcopy_ffi.rs` constants are
already backfilled into spec 002.

## Recommendations

- FR-012: either add a runtime `chunk_size <= MDTS` check that queries the
  controller and errors/clamps, or soften FR-012 to state the MDTS bound is a
  caller responsibility documented via CLI help (matching current behavior).
- Spec 003 is still marked "needs human review" — a maintainer pass would let
  it graduate from backfilled-draft status.
