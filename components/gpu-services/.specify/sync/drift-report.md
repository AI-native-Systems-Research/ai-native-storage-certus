# Spec Drift Report

Generated: 2026-05-29
Project: GPU Services V0
Specs: 001-gpu-cuda-services, 002-gpu-ssd-dma-prepare
Previous sync: 2026-05-21 (baseline: 42 aligned, 0 drifted, 0 unspecced)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 44 |
| Aligned | 43 (98%) |
| Drifted | 1 (2%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 0 |

## Detailed Findings

### Spec: 001-gpu-cuda-services - GPU CUDA Services

#### Aligned

- **FR-001**: CUDA initialization via `cudaGetDeviceCount` + `cudaGetDeviceProperties`; descriptive errors returned on failure. Idempotent (early returns if already initialized).
- **FR-002**: `discover_devices()` enumerates all GPUs, filters to `prop.major >= 7`, reports model name, `total_global_mem` as bytes, compute major/minor, device index, and `pci_bus_id`.
- **FR-003**: `decode_ipc_payload()` performs base64 decode, validates 72-byte length, splits into 64-byte handle + 8-byte LE u64 size. `open_ipc_handle()` calls `cudaIpcOpenMemHandle` with `CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS`.
- **FR-004**: `check_memory_attributes()` calls `cudaPointerGetAttributes` and verifies `type == CUDA_MEMORY_TYPE_DEVICE`. Verified pointers tracked in `GpuState.verified` HashSet.
- **FR-005**: `pin_memory` is idempotent (no-op if already in pinned set). `unpin_memory` returns error if pointer not in pinned set. State tracked in `HashSet<usize>`. `pin_memory` MAY skip re-verification for already-verified pointers (documented optimization).
- **FR-006**: `create_dma_buffer()` creates `GpuDmaBuffer` from verified+pinned handle via `cuda_ipc_close_mem_handle` free function. `prepare_memory_for_spdk` returns SPDK `DmaBuffer` (spec 002 path).
- **FR-007**: All functions return `Result<_, String>` with descriptive messages. No panics on error paths. Resource cleanup (IPC close, device restore) on all error branches.
- **FR-008**: All functionality exposed exclusively through `IGpuServices` trait defined in `components/interfaces/src/igpu_services.rs`.
- **FR-009**: All GPU code wrapped in `#[cfg(feature = "gpu")]`. Without feature, returns "GPU support not compiled" error.
- **FR-010**: Criterion benchmarks exist: `benches/gpu_services_benchmark.rs` and `benches/dma_transfer_benchmark.rs`. Both require `gpu` feature.
- **FR-011**: `dma_copy_to_host` performs `cudaMemcpy` with `CUDA_MEMCPY_DEVICE_TO_HOST`. Validates `size <= dst.len()`. Gated behind `#[cfg(feature = "spdk")]`.
- **FR-012**: `dma_copy_to_device` performs `cudaMemcpy` with `CUDA_MEMCPY_HOST_TO_DEVICE`. Validates `size <= src.len()`. Gated behind `#[cfg(feature = "spdk")]`.
- **FR-013**: `prepare_memory_for_spdk` accepts base64 payload + optional device index, performs full pipeline (decode, open, check pin state, conditionally pin, spdk_mem_register, create DmaBuffer). Gated behind `spdk` feature.
- **FR-014**: All interface methods check `#[cfg(not(feature = "gpu"))]` and return descriptive error "GPU support not compiled (enable --features gpu)".
- **FR-015**: `register_host_memory` calls `cudaHostRegister` then `spdk_mem_register`. On SPDK failure, rolls back via `cudaHostUnregister`. Gated behind `spdk` feature.
- **FR-016**: `unregister_host_memory` calls `spdk_mem_unregister` then `cudaHostUnregister`. Gated behind `spdk` feature.
- **FR-017**: `create_stream` returns `GpuStream(cudaStreamCreate())`. `destroy_stream` calls `cudaStreamDestroy`. `stream_synchronize` calls `cudaStreamSynchronize`. All three require initialization and return errors without GPU support.
- **FR-018**: `dma_copy_to_device_async` calls `cudaMemcpyAsync` with `CUDA_MEMCPY_HOST_TO_DEVICE` on the provided stream. Validates `size <= src.len()`. Gated behind `spdk`.
- **FR-019**: `memcpy_h2d_async` calls `cudaMemcpyAsync` from a raw pinned host pointer to a GPU device pointer on the specified stream. Gated behind `spdk`.
- **FR-020**: `allocate_pinned_dma_buffer` calls `cudaHostAlloc` then `spdk_mem_register`; on drop, buffer unregisters from SPDK and frees via `cudaFreeHost`. Gated behind `spdk`.

#### Drifted

- **Assumption (spec 001, last bullet)**: Spec states "GPU device selection for IPC operations is implicit — the IPC handle carries the originating device context and the component follows it automatically." This is inaccurate. `open_ipc_handle()` in `src/ipc.rs` does **not** call `cudaSetDevice`. It relies entirely on the caller having already set the correct CUDA device context before the call. In production, it is the certus-server's `service.rs` that calls `cudaSetDevice` before invoking `deserialize_ipc_handle`. The `open_ipc_handle` function has an implicit precondition — the caller's device context must already match the target GPU for the IPC handle. This precondition is not documented in the spec.
  - Location: `src/ipc.rs` `open_ipc_handle()` (no `cudaSetDevice` call present); spec 001 Assumptions section, final bullet.
  - Severity: **moderate** — incorrect assumption could lead callers of the low-level `open_ipc_handle` function to omit required device context setup. The high-level `prepare_memory_for_spdk` path (spec 002, FR-014) correctly documents and implements `cudaSetDevice` when a device index is provided.

#### Not Implemented

(none)

### Success Criteria (001)

- **SC-001 through SC-008**: All aligned. No changes since 2026-05-21.

---

### Spec: 002-gpu-ssd-dma-prepare - GPU-to-SSD DMA Buffer Preparation

#### Aligned

- **FR-001 through FR-024**: All aligned. No changes since 2026-05-21.
  - FR-014 specifically: `prepare_memory_for_spdk` correctly saves original device via `cudaGetDevice`, switches via `cudaSetDevice(idx)`, and restores via `restore_device` on both success and all error paths. This is the correct documented behavior for the high-level path.

#### Drifted

(none — the `cudaSetDevice` drift is in spec 001's Assumption section, not spec 002's requirements)

#### Not Implemented

(none)

### Success Criteria (002)

- **SC-001 through SC-005**: All aligned.

---

## Unspecced Code

(none — all features have FR coverage from the 2026-05-21 sync)

---

## Recommendations

1. **Backfill spec 001 Assumptions**: Replace the last Assumption bullet with accurate language reflecting that `open_ipc_handle` is a low-level function with a caller precondition: the caller must have set the CUDA device context to the target GPU (via `cudaSetDevice`) before calling this function. Reference `prepare_memory_for_spdk` (spec 002) as the high-level path that handles this automatically.

2. **Add a Preconditions note to FR-003**: Augment FR-003 to state that `open_ipc_handle` (and by extension `deserialize_ipc_handle`) requires the caller to have set the correct CUDA device context. The high-level `prepare_memory_for_spdk` (FR-013) handles device context internally; callers using the lower-level `deserialize_ipc_handle` are responsible for calling `cudaSetDevice` first.
