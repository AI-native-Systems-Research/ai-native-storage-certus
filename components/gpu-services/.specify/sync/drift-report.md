# Spec Drift Report

Generated: 2026-05-21
Project: GPU Services V0
Specs: 001-gpu-cuda-services, 002-gpu-ssd-dma-prepare

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 36 |
| Aligned | 33 (92%) |
| Drifted | 1 (3%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 5 |

## Detailed Findings

### Spec: 001-gpu-cuda-services - GPU CUDA Services

#### Aligned

- **FR-001**: CUDA initialization via `cudaGetDeviceCount` + `cudaGetDeviceProperties`; descriptive errors returned on failure. Idempotent (early returns if already initialized).
- **FR-002**: `discover_devices()` enumerates all GPUs, filters to `prop.major >= 7`, reports model name, `total_global_mem` as bytes, compute major/minor, device index, and `pci_bus_id`.
- **FR-003**: `decode_ipc_payload()` performs base64 decode, validates 72-byte length, splits into 64-byte handle + 8-byte LE u64 size. `open_ipc_handle()` calls `cudaIpcOpenMemHandle`.
- **FR-004**: `check_memory_attributes()` calls `cudaPointerGetAttributes` and verifies `type == CUDA_MEMORY_TYPE_DEVICE`. Verified pointers tracked in `GpuState.verified` HashSet.
- **FR-005**: `pin_memory` is idempotent (no-op if already in pinned set). `unpin_memory` returns error if pointer not in pinned set. State tracked in `HashSet<usize>`. IPC device memory is inherently pinned by CUDA, so tracking is the correct semantic for this use case.
- **FR-006**: `create_dma_buffer()` creates `GpuDmaBuffer` from verified+pinned handle via `cuda_ipc_close_mem_handle` free function. `prepare_memory_for_spdk` returns SPDK `DmaBuffer` (spec 002 path).
- **FR-007**: All functions return `Result<_, String>` with descriptive messages. No panics on error paths. Resource cleanup (IPC close, device restore) on all error branches.
- **FR-008**: All functionality exposed exclusively through `IGpuServices` trait defined in `components/interfaces/src/igpu_services.rs`.
- **FR-009**: All GPU code wrapped in `#[cfg(feature = "gpu")]`. Without feature, returns "GPU support not compiled" error.
- **FR-010**: Criterion benchmarks exist: `benches/gpu_services_benchmark.rs` (init, get_devices, deserialize) and `benches/dma_transfer_benchmark.rs` (H2D/D2H throughput across sizes). Both require `gpu` feature.
- **FR-011**: `dma_copy_to_host` performs `cudaMemcpy` with `CUDA_MEMCPY_DEVICE_TO_HOST`. Validates `size <= dst.len()`. Gated behind `#[cfg(feature = "spdk")]`.
- **FR-012**: `dma_copy_to_device` performs `cudaMemcpy` with `CUDA_MEMCPY_HOST_TO_DEVICE`. Validates `size <= src.len()`. Gated behind `#[cfg(feature = "spdk")]`.
- **FR-013**: `prepare_memory_for_spdk` accepts base64 payload + optional device index, performs full pipeline (decode, open, check pin state, conditionally pin, spdk_mem_register, create DmaBuffer). Gated behind `spdk` feature.
- **FR-014**: All interface methods check `#[cfg(not(feature = "gpu"))]` and return descriptive error "GPU support not compiled (enable --features gpu)".
- **FR-015**: `register_host_memory` calls `cudaHostRegister` then `spdk_mem_register`. On SPDK failure, rolls back via `cudaHostUnregister`. Gated behind `spdk` feature.
- **FR-016**: `unregister_host_memory` calls `spdk_mem_unregister` then `cudaHostUnregister`. Gated behind `spdk` feature.

#### Drifted

- **FR-005** (minor): Spec says "pin verifies device-residency" but the `pin_memory` method calls `check_memory_attributes` only if the pointer is not already in the `verified` set. If already verified, it just inserts into the pinned set without re-verifying device-residency. This is a minor optimization divergence, not a functional bug, since verification is a prerequisite anyway.
  - Location: `src/lib.rs` lines 193-225
  - Severity: minor (correct for the IPC workflow where verify is always called before pin)

#### Not Implemented

(none)

### Success Criteria (001)

- **SC-001**: Aligned. Initialization calls two CUDA APIs (device count + properties per device) which completes well under 5 seconds.
- **SC-002**: Aligned. `discover_devices()` populates all required fields within initialization.
- **SC-003**: Aligned. `decode_ipc_payload` is base64 decode + byte slice operations (sub-millisecond).
- **SC-004**: Aligned. `check_memory_attributes` is a single `cudaPointerGetAttributes` call.
- **SC-005**: Aligned. `create_gpu_dma_buffer` / `create_spdk_dma_buffer_from_gpu` is pointer wrapping + `spdk_mem_register`.
- **SC-006**: Aligned. Tests exist in `src/lib.rs` (unit tests) and `tests/gpu_nvme_p2p.rs` (integration).
- **SC-007**: Aligned. Two benchmark suites exist with `required-features = ["gpu"]`.
- **SC-008**: Aligned. `src/bin/p2p_server.rs` implements Unix socket server; `tests/gpu_client_p2p.py` and `tests/pytorch_alloc_ipc.py` are clients.

---

### Spec: 002-gpu-ssd-dma-prepare - GPU-to-SSD DMA Buffer Preparation

#### Aligned

- **FR-001**: `prepare_memory_for_spdk` accepts `&str` (base64) + `Option<u32>` (device index), returns `interfaces::DmaBuffer`.
- **FR-002**: `open_ipc_handle` passes `CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS` flag (value 1) to `cudaIpcOpenMemHandle`.
- **FR-003**: Pin state checked by `state.pinned.contains(&(ptr as usize))` against internal `HashSet` (not `cudaPointerGetAttributes`).
- **FR-004**: If not already pinned, verifies memory attributes and inserts into pinned set.
- **FR-005**: Logs "pinning GPU memory for DMA" or "memory already pinned -- skipping" via logger receptacle.
- **FR-006**: When `was_already_pinned = false`, free function is `spdk_unregister_unpin_and_ipc_close` which unpins on drop.
- **FR-007**: When `was_already_pinned = true`, free function is `spdk_unregister_and_ipc_close` which does NOT unpin.
- **FR-008**: Both free functions call `cudaIpcCloseMemHandle(ptr)` as final step.
- **FR-009**: Returns error "Not initialized: call initialize() first" if not initialized.
- **FR-010**: Returns error from `ipc::decode_ipc_payload` or `ipc::open_ipc_handle` on invalid payload or IPC open failure.
- **FR-011**: Peer access flag is `CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS` (lazy via flag to `cudaIpcOpenMemHandle`).
- **FR-012**: All error paths roll back: close IPC handle, remove from pinned/verified sets, restore device context.
- **FR-013**: Method has `#[cfg(feature = "spdk")]` attribute.
- **FR-014**: When device index provided, `cudaGetDevice` saves original, `cudaSetDevice` switches, `restore_device` helper restores on both success and error.
- **FR-015**: Returns `interfaces::DmaBuffer` via `dma::create_spdk_dma_buffer_from_gpu`.
- **FR-016**: `create_spdk_dma_buffer_from_gpu` calls `spdk_mem_register(ptr, size)` before creating the DmaBuffer.
- **FR-017**: On error after `spdk_mem_register`, `create_spdk_dma_buffer_from_gpu` calls `spdk_mem_unregister(ptr, size)`.
- **FR-018**: `restore_device` helper called on both success (`restore_device(original_device)` after buffer creation) and all error paths.
- **FR-019**: `register_host_memory` calls `cudaHostRegister` then `spdk_mem_register`. On `spdk_mem_register` failure, rolls back via `cudaHostUnregister`.
- **FR-020**: `unregister_host_memory` calls `spdk_mem_unregister` then `cudaHostUnregister`.

#### Drifted

(none)

#### Not Implemented

(none)

### Success Criteria (002)

- **SC-001**: Aligned. Single function call performs full pipeline (decode, open, pin-check, pin, spdk_register, DmaBuffer creation).
- **SC-002**: Aligned. Two distinct free functions based on pin state.
- **SC-003**: Aligned. Logger messages emitted for both pin cases.
- **SC-004**: Aligned. Comprehensive rollback on all error paths (IPC close, state cleanup, SPDK unregister, device restore).
- **SC-005**: Aligned. Follows same `Result<_, String>` pattern and feature gating as other methods.

---

## Unspecced Code

The following code features exist in the implementation without coverage in either spec:

1. **CUDA Stream API** (`create_stream`, `destroy_stream`, `stream_synchronize`): Interface defines and component implements CUDA stream lifecycle management. Not mentioned in either spec.
   - Location: `src/lib.rs` lines 539-605, `interfaces/src/igpu_services.rs` lines 486-537

2. **Async DMA copy** (`dma_copy_to_device_async`): Async H2D copy using `cudaMemcpyAsync` with a stream. Not covered by any spec requirement.
   - Location: `src/lib.rs` lines 608-660

3. **Raw pointer async copy** (`memcpy_h2d_async`): Raw `*const c_void` to `*mut c_void` async memcpy. Not in any spec.
   - Location: `src/lib.rs` lines 662-704

4. **Pinned DMA buffer allocation** (`allocate_pinned_dma_buffer`): Allocates page-locked host memory via `cudaHostAlloc` and registers with SPDK. Not in any spec.
   - Location: `src/lib.rs` lines 706-744

5. **GDRCopy P2P DMA path** (`create_spdk_dma_buffer_from_gpu_bar`, `create_spdk_dma_buffer_from_phys`, `create_spdk_dma_buffer_from_bar_direct`): Full GDRCopy-based BAR1 mapping for true NVMe-to-GPU P2P DMA. Gated behind `p2p` feature. Not covered by any spec.
   - Location: `src/dma.rs` lines 353-720, `tests/gpu_nvme_p2p.rs`, `src/bin/p2p_server.rs`

## Recommendations

1. **Write spec 003 for GDRCopy P2P DMA**: The `p2p` feature adds substantial functionality (BAR1 mapping, VFIO IOMMU programming, cross-process physical address DMA) that deserves its own spec.

2. **Write spec 004 for async stream operations**: `create_stream`, `destroy_stream`, `stream_synchronize`, `dma_copy_to_device_async`, `memcpy_h2d_async`, and `allocate_pinned_dma_buffer` form a coherent feature set for pipelined NVMe-to-GPU transfers.

3. **Update FR-005 wording** (001): Clarify that pin_memory's verification behavior depends on prior calls to verify_memory. Current code only re-verifies when the pointer is not in the verified set — document this as the intended optimization.
