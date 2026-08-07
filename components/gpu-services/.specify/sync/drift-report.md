# Spec-vs-Implementation Drift Report — `gpu-services`

Generated: 2026-08-07T15:27:21Z

Component: `components/gpu-services`
Specs analyzed:
- `specs/001-gpu-cuda-services/spec.md` (FR-001..FR-025, SC-001..SC-008)
- `specs/002-gpu-ssd-dma-prepare/spec.md` (FR-001..FR-024, SC-001..SC-005)
- `specs/003-gpu-p2p-server/spec.md` (FR-001..FR-012, SC-001..SC-004)

Sources: `src/lib.rs`, `src/dma.rs`, `src/ipc.rs`, `src/device.rs`, `src/memory.rs`,
`src/gdrcopy_ffi.rs`, `src/bin/p2p_server.rs`, `components/interfaces/src/igpu_services.rs`,
`Cargo.toml`.

## Summary Table

| Spec | Requirements | Aligned | Drifted | Not Implemented |
|------|--------------|---------|---------|-----------------|
| 001-gpu-cuda-services   | 25 FR + 8 SC | 23 FR + 8 SC | 2 FR (minor) | 0 |
| 002-gpu-ssd-dma-prepare | 24 FR + 5 SC | 24 FR + 5 SC | 0            | 0 |
| 003-gpu-p2p-server      | 12 FR + 4 SC | 12 FR + 4 SC | 0            | 0 |

Overall: implementation tracks specs closely. Only spec 001 has drift, both minor.
Specs 002 and 003 are clean. No requirement is unimplemented. Several small
unspecced helpers/fields exist (see Unspecced Code). No spec references a
nonexistent file — demo apps (`apps/gpu-handle-test-client`,
`apps/gpu-handle-test-server`), the benches, and `verif/` all exist.

## Detailed Findings

### Spec 001 — GPU CUDA Services

Aligned (representative locations):
- FR-001 initialize CUDA + report success/failure — `src/lib.rs:98`, `src/device.rs:11`
- FR-002 enumerate GPUs, exclude compute capability < 7.0 — `src/device.rs:39` (`if prop.major < 7 { continue }`)
- FR-003 deserialize base64 IPC handle; low-level `open_ipc_handle` does NOT call `cudaSetDevice` — `src/lib.rs:166`, `src/ipc.rs:42`
- FR-004 verify device memory via `cudaPointerGetAttributes`, track in verified set — `src/lib.rs:192`, `src/memory.rs:8`
- FR-006 create `GpuDmaBuffer` from verified+pinned handle — `src/lib.rs:269`, `src/dma.rs:727`
- FR-007 descriptive errors, no leak — pervasive (rollback paths in `prepare_memory_for_spdk`, `src/lib.rs:407-505`)
- FR-009 gated behind `gpu` feature — `Cargo.toml:9`, `#[cfg(feature = "gpu")]` throughout
- FR-010 unit tests + Criterion benches under `gpu` — `src/lib.rs:1049`, `Cargo.toml:30-38`
- FR-011 `dma_copy_to_host` (spdk) — `src/lib.rs:301`
- FR-012 `dma_copy_to_device` (spdk) — `src/lib.rs:511`
- FR-013 `prepare_memory_for_spdk` — `src/lib.rs:352`
- FR-014 gpu disabled → error; `shutdown()` → `Ok(())` no-op — `src/lib.rs:99-101`, `src/lib.rs:130-133`
- FR-015 `register_host_memory` with CUDA-rollback on SPDK failure — `src/lib.rs:960`
- FR-016 `unregister_host_memory` (SPDK then CUDA) — `src/lib.rs:1010`
- FR-017 stream create/destroy/synchronize — `src/lib.rs:561`, `:657`, `:702`
- FR-018 `dma_copy_to_device_async` — `src/lib.rs:725`
- FR-019 `memcpy_h2d_async` — `src/lib.rs:779`
- FR-020 `allocate_pinned_dma_buffer` (cudaHostAlloc + SPDK register) — `src/lib.rs:920`, `src/dma.rs:253`
- FR-021 `set_device` — `src/lib.rs:588`
- FR-022 `device_of_ptr` returns `-1` for non-device — `src/lib.rs:616-654`
- FR-023 `stream_query` (non-blocking) — `src/lib.rs:678`
- FR-024 `dma_copy_to_host_async` — `src/lib.rs:823`
- FR-025 `memcpy_d2h_async` — `src/lib.rs:877`
- SC-006 tests present (`cargo test --features gpu`), SC-007 benches present, SC-008 demo apps present (`apps/gpu-handle-test-*`). SC-001..005 are runtime performance targets not statically verifiable; supporting code/benches exist.

Drifted:
- **FR-005 (minor)** — Spec states "For locally-pinned memory, full CUDA
  unregistration is performed." `unpin_memory` (`src/lib.rs:249-267`) only removes
  the pointer from the internal `pinned` HashSet and never calls
  `cudaHostUnregister`; there is no locally-pinned code path in
  `pin_memory`/`unpin_memory`. The idempotent-pin and "error if not pinned" clauses
  ARE implemented correctly. The locally-pinned-unregister clause describes behavior
  not present in this interface method (local host un/registration lives only in the
  separate `register_host_memory`/`unregister_host_memory` methods). Spec wording is
  aspirational vs code.
- **FR-008 (minor)** — Spec: "expose all functionality exclusively through the
  `IGpuServices` interface." The P2P/GDRCopy DMA-buffer builders
  (`create_spdk_dma_buffer_from_gpu`, `_from_cuda_malloc`, `_from_cuda_host_alloc`,
  `_from_gpu_bar`, `_from_phys`, `_from_bar_direct`, `get_phys_addr`) are `pub`
  functions in the `dma` module (`src/dma.rs`), not `IGpuServices` methods, and are
  called directly by `src/bin/p2p_server.rs:21`. This is by design per spec 002/003
  but conflicts with the exclusivity wording in spec 001 FR-008 (see Conflicts).

Narrative note (not a scored FR):
- User Story 3 acceptance scenario 2 implies the check distinguishes
  "non-contiguous or unpinned" conditions. `check_memory_attributes`
  (`src/memory.rs:26`) only validates memory `type == device`; it does not separately
  report contiguity vs pin status. The normative FR-004 (device-type check only) is
  aligned; the US3 narrative is broader than the code. Minor/informational.

### Spec 002 — GPU-to-SSD DMA Buffer Preparation

All FR-001..FR-024 aligned:
- FR-001 `prepare_memory_for_spdk(&str, Option<u32>) -> DmaBuffer` — `src/lib.rs:352`
- FR-002 / FR-011 opens handle with `CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS` — `src/ipc.rs:55`
- FR-003 pin state checked via internal `pinned` HashSet (not `cudaPointerGetAttributes`) — `src/lib.rs:427-438`
- FR-004 conditional pin — `src/lib.rs:441-470`
- FR-005 pin/skip decisions logged — `src/lib.rs:465-470`
- FR-006 / FR-007 / FR-008 pin-state-aware free functions, both close IPC handle — `src/dma.rs:69-102`, `:113-162`
- FR-009 error if uninitialized — `src/lib.rs:365-368`
- FR-010 error if IPC open fails — `src/lib.rs:415-421`
- FR-012 no leak on error (rollback pin + close handle) — `src/lib.rs:476-489`
- FR-013 gated behind `spdk` — `#[cfg(feature = "spdk")]` on method
- FR-014 device index sets context, else current — `src/lib.rs:372-394`
- FR-015 returns SPDK `DmaBuffer` — `src/lib.rs:356`
- FR-016 `spdk_mem_register` on GPU device pointer — `src/dma.rs:122`
- FR-017 `spdk_mem_unregister` rollback on error — `src/dma.rs:151-159`
- FR-018 restore original device on success+error — `src/lib.rs:397-404`, invoked on all paths
- FR-019 `register_host_memory` — `src/lib.rs:960`
- FR-020 `unregister_host_memory` — `src/lib.rs:1010`
- FR-021 `create_spdk_dma_buffer_from_gpu_bar` (p2p, GDRCopy pin/map + SPDK register) — `src/dma.rs:353`
- FR-022 `create_spdk_dma_buffer_from_phys` (p2p, mmap + rte_extmem + VFIO) — `src/dma.rs:547`
- FR-023 `create_spdk_dma_buffer_from_bar_direct` (p2p) — `src/dma.rs:635`
- FR-024 GDRCopy FFI (`gdr_open/close/pin_buffer/unpin_buffer/map/unmap`) + `GPU_PAGE_SIZE = 64 KiB` — `src/gdrcopy_ffi.rs:16-33` (`1 << 16 = 65536`)
- SC-001..005 aligned with the single-call pipeline, pin-state-aware cleanup, and logging above.

### Spec 003 — GPU P2P DMA Server (`gpu-p2p-server` binary)

All FR-001..FR-012 aligned:
- FR-001 binary gated behind `p2p` — `Cargo.toml:40-43` (`required-features = ["p2p"]`)
- FR-002 CLI args with correct defaults (`--socket` `/tmp/gpu_p2p_server.sock`, `--pci`, `--mode` default `p2p`, `--staging-size` 4194304, `--chunk-size` 131072, `--once`) — `src/bin/p2p_server.rs:35-61`
- FR-003 verify `nvidia_peermem` + `gdrdrv` via `/proc/modules`, FATAL before bind — `:100-122`
- FR-004 init SPDK env + CUDA + open NVMe + `atexit(_exit(0))` — `:106-223`
- FR-005 pre-allocate GDRCopy-pinned SPDK-registered staging pool for `p2p` — `:251-270`, `:582-604`
- FR-006 listen on Unix socket, remove pre-existing, non-blocking accept poll ~100µs — `:618-668`
- FR-007 read newline-terminated base64 72-byte payload, decode, open IPC — `:325-371`
- FR-008 dispatch bounce/p2p/p2p-cold — `:642-651`, handlers `:374`, `:436`, `:493`
- FR-009 `OK <size> bytes (<mode>, <n> chunks)` / `ERROR: <message>` — `:432`, `:489`, `:563`, `:653-661`
- FR-010 `--once` serves one then exit — `:662-664`
- FR-011 SIGINT/SIGTERM set atomic flag; loop breaks, drops pool, removes socket — `:94-98`, `:613-616`, `:634-638`, `:676-677`
- FR-012 chunked NVMe reads in `--chunk-size` increments — `:273-323`
- SC-001..004 supported by the three-mode dispatch, `--once`, signal-driven shutdown, and error-response paths above.

## Unspecced Code

| Item | Location | Notes | Severity |
|------|----------|-------|----------|
| `create_spdk_dma_buffer_from_cuda_malloc` + `spdk_unregister_and_cuda_free` | `src/dma.rs:189`, `:169` | Public SPDK builder for `cudaMalloc`-backed GPU memory; not referenced by any FR (specs cover IPC-handle, host-alloc, and BAR paths only). | minor |
| `get_phys_addr` | `src/dma.rs:522` | Public p2p helper wrapping `spdk_vtophys`; not mentioned in any spec. | minor |
| `GPU_PAGE_SHIFT` constant | `src/gdrcopy_ffi.rs:16` | Exported alongside spec'd `GPU_PAGE_SIZE`; spec 002 FR-024 only names `GPU_PAGE_SIZE`. | trivial |
| `GpuServicesComponent::initialized` (AtomicBool) + `is_initialized()` | `src/lib.rs:70`, `:92` | Lock-free mirror of `GpuState.initialized` for async copy hot paths; not in the `GpuState` Key Entity description (spec 001). Implementation optimization. | trivial |
| `GpuIpcHandle::{verified, pinned}` fields + `set_verified/set_pinned/is_verified/is_pinned` | `components/interfaces/src/igpu_services.rs:63-118` | Per-handle verified/pinned bools + accessors are defined but the component tracks state via its own `HashSet<usize>` and never reads/writes these fields. Dead API surface. | minor |

## Conflicts

- **Spec 001 FR-008 vs Spec 002 FR-021..FR-024 / Spec 003** — FR-008 mandates exposing
  all functionality "exclusively through the `IGpuServices` interface", but the P2P
  DMA-buffer builders are `pub` module functions in `src/dma.rs` consumed directly by
  the `gpu-p2p-server` binary (`src/bin/p2p_server.rs:21`,
  `gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar`). Spec 003's Assumptions
  acknowledge this direct dependency. The three specs are internally consistent about
  the behavior, but the FR-008 "exclusively" wording is contradicted. Recommend
  softening FR-008 to scope the exclusivity to interface-level operations, or noting
  the p2p module functions as an explicit exception.

## Recommendations

1. **FR-005 (spec 001)** — Remove or reword the "For locally-pinned memory, full CUDA
   unregistration is performed" clause; `unpin_memory` performs tracking-removal only.
   If local unregistration is intended, point it at
   `unregister_host_memory` rather than `unpin_memory`.
2. **FR-008 (spec 001)** — Reword to exempt the intentionally-public `dma` module P2P
   helpers, or move them behind the interface, to resolve the conflict with specs 002/003.
3. **User Story 3 (spec 001)** — Align the acceptance-scenario narrative with the
   implemented check (device-type only) or backfill a contiguity/pin distinction into
   `check_memory_attributes`.
4. **Unspecced code** — Backfill a short requirement for
   `create_spdk_dma_buffer_from_cuda_malloc` and `get_phys_addr` (spec 002), or mark
   them internal. Consider removing the unused `GpuIpcHandle::{verified,pinned}`
   fields/accessors or documenting them as reserved.
5. No action needed for specs 002 and 003 — both are aligned.
