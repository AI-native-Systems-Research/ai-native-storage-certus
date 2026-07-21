# Spec Drift Report
Generated: 2026-07-21
Project: gpu-services

Scope: focused re-analysis after two methods were added to the `IGpuServices`
interface on this branch (base commit `833e9f36`): `set_device` and
`device_of_ptr` in `components/gpu-services/src/lib.rs` and
`components/interfaces/src/igpu_services.rs`.

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 44 |
| Aligned | 44 (100%) |
| Drifted | 0 (0%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 2 |

## Detailed Findings
### Spec: 001-gpu-cuda-services - GPU CUDA Services
#### Aligned
All previously-verified functional requirements remain aligned; none of the two
new methods contradict or alter existing behavior.
- FR-001: CUDA initialization with success/failure reporting → src/lib.rs
- FR-002: Enumerate GPUs with CC 7.0+ reporting model, memory, compute arch → src/device.rs
- FR-003: Deserialize base64 IPC handle into native Rust structures → src/lib.rs, src/ipc.rs
- FR-004: Verify GPU memory is device type via cudaPointerGetAttributes, track in verified set → src/lib.rs, src/memory.rs
- FR-005: Pin/unpin operations with idempotent pin, error on unpin of non-pinned → src/lib.rs
- FR-006: Create GpuDmaBuffer from verified+pinned handle with custom free → src/lib.rs, src/dma.rs
- FR-007: All operations return descriptive errors without panicking → all methods use Result<_, String>
- FR-008: Expose functionality through IGpuServices interface → src/lib.rs
- FR-009: Build gated behind --features gpu → Cargo.toml features
- FR-010: Unit tests and Criterion benchmarks with gpu feature → src/lib.rs tests, benches/
- FR-011: dma_copy_to_host using cudaMemcpy D2H, gated behind spdk → src/lib.rs
- FR-012: dma_copy_to_device using cudaMemcpy H2D, gated behind spdk → src/lib.rs
- FR-013: prepare_memory_for_spdk full pipeline, gated behind spdk → src/lib.rs
- FR-014: Return error when gpu feature disabled → cfg(not(feature = "gpu")) blocks
- FR-015: register_host_memory with cudaHostRegister + spdk_mem_register, rollback → src/lib.rs
- FR-016: unregister_host_memory with spdk_mem_unregister + cudaHostUnregister → src/lib.rs
- FR-017: CUDA stream lifecycle (create_stream, destroy_stream, stream_synchronize) → src/lib.rs
- FR-018: dma_copy_to_device_async using cudaMemcpyAsync H2D on a stream → src/lib.rs
- FR-019: memcpy_h2d_async from raw pinned host pointer → src/lib.rs
- FR-020: allocate_pinned_dma_buffer via cudaHostAlloc + SPDK register → src/lib.rs

#### Drifted
(none)

#### Not Implemented
(none)

---

### Spec: 002-gpu-ssd-dma-prepare - GPU-to-SSD DMA Buffer Preparation
#### Aligned
All FR-001 through FR-024 remain aligned (see prior full drift report of
2026-07-15). The two new methods are core-CUDA-services capabilities and belong
to spec 001; they do not affect any requirement in this spec.

#### Drifted
(none)

#### Not Implemented
(none)

---

## Unspecced Code
| Feature | Location | Suggested FR |
|---------|----------|--------------|
| `set_device(device)` — bind calling thread's CUDA context to a GPU ordinal via `cudaSetDevice`; prerequisite for multi-GPU stream creation and cross-device memcpy routing | src/lib.rs:566-592, interfaces/src/igpu_services.rs:555 | 001-gpu-cuda-services / FR-021 |
| `device_of_ptr(ptr)` — return CUDA device ordinal owning a device pointer via `cudaPointerGetAttributes`; returns -1 when no device association; used to route transfers and reject cross-device pointers | src/lib.rs:594-633, interfaces/src/igpu_services.rs:577 | 001-gpu-cuda-services / FR-022 |

## Inter-Spec Conflicts
(none)

## Recommendations
Backfill spec 001-gpu-cuda-services with two new functional requirements (next
available numbers after FR-020). Both are new capabilities that do not
contradict any existing requirement.

- **FR-021**: Component MUST provide a `set_device(device)` method that binds
  the calling thread's current CUDA device context to the specified GPU ordinal
  via `cudaSetDevice`, so that subsequently-created streams and issued transfers
  target that GPU. This is required for multi-GPU / tensor-parallel operation,
  where each device must be selected before a stream is created on it or a
  `cudaMemcpyAsync` is issued to a pointer resident on it (a stream is bound to
  the device that was current at creation, and `cudaMemcpyAsync` rejects a
  destination pointer on a different device). CUDA tracks the current device per
  OS thread. MUST return an error if GPU support is not compiled, the component
  is not initialized, or the device ordinal is invalid.
- **FR-022**: Component MUST provide a `device_of_ptr(ptr)` method that returns
  the CUDA device ordinal owning a given device pointer via
  `cudaPointerGetAttributes`. It MUST return `-1` for a pointer with no device
  association (e.g. plain host or unregistered memory). This is used to route a
  transfer to a stream on the pointer's own device and to reject cross-device
  pointers. MUST return an error if GPU support is not compiled, the component
  is not initialized, or the attribute query fails.
</content>
