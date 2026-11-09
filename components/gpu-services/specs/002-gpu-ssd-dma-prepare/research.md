# Research: GPU-to-SSD DMA Buffer Preparation

## R1: DmaBuffer::from_raw for GPU Memory

**Decision**: Use `DmaBuffer::from_raw(ptr, len, free_fn, numa_node)` to wrap GPU device memory as an SPDK DmaBuffer.

**Rationale**: The SPDK `DmaBuffer` type already supports external memory via `from_raw` (added specifically for this use case). It accepts a custom `free_fn` pointer which is called on Drop, allowing us to attach pin-state-aware cleanup logic. The GPU device pointer is valid for SPDK NVMe P2P DMA because GPU BAR memory is PCIe-addressable.

**Alternatives considered**:
- Returning `GpuDmaBuffer` (existing type) — rejected because it requires downstream conversion and the spec explicitly requires SPDK `DmaBuffer`.
- Allocating SPDK hugepage memory and copying GPU→host→SPDK — rejected because it defeats the zero-copy P2P DMA purpose.

## R2: Pin-State-Aware Free Functions

**Decision**: Define two `unsafe extern "C" fn(*mut c_void)` free functions:
1. `cuda_ipc_close_only` — calls `cudaIpcCloseMemHandle(ptr)` only.
2. `cuda_ipc_unpin_and_close` — calls `cudaHostUnregister(ptr)` then `cudaIpcCloseMemHandle(ptr)`.

Select at buffer creation time based on whether memory was already pinned.

**Rationale**: The free function signature `unsafe extern "C" fn(*mut c_void)` is fixed by `DmaBuffer::from_raw`. The decision must be baked in at construction because no additional context (was-pinned flag) can be passed to the free function at drop time. Since `cudaHostUnregister` is safe to call even if the memory was host-registered by another mechanism, the unpin path is idempotent for our pinning.

**Alternatives considered**:
- Single free function with global state lookup — rejected due to complexity and lock contention on drop.
- Storing pin state in DmaBuffer metadata and checking in a wrapper — rejected because DmaBuffer's Drop calls `free_fn` directly; no hook to read metadata.

## R3: Pin State Detection via cudaPointerGetAttributes

**Decision**: Use `cudaPointerGetAttributes` to detect if memory is already host-registered (pinned). If `type == cudaMemoryTypeDevice` and the pointer is valid, the memory is device-allocated. For P2P DMA, device memory is inherently "pinned" (it's physical GPU VRAM). The "pinning" in this context refers to registering the memory for host access or marking it for P2P — which `cudaIpcOpenMemHandle` with `cudaIpcMemLazyEnablePeerAccess` already handles.

**Rationale**: The existing `memory::check_memory_attributes` function already queries `cudaPointerGetAttributes`. The `is_pinned` semantic in this codebase tracks whether we've explicitly registered the memory via the component's `pin_memory()` method. For `prepare_memory_for_spdk`, we query the component's `pinned` HashSet (not a CUDA API) to determine whether the IPC memory was previously pinned through this component.

**Alternatives considered**:
- Querying CUDA directly for host-registration status — CUDA doesn't expose this cleanly for IPC-opened device memory.
- Always pinning unconditionally — rejected because double-pin may cause errors and the spec requires conditional behavior.

## R4: Optional Device Index for Multi-GPU

**Decision**: When `device_index: Option<u32>` is `Some(idx)`, call `cudaSetDevice(idx)` before `cudaIpcOpenMemHandle`. Restore the original device afterward using `cudaSetDevice(original)`.

**Rationale**: CUDA IPC handles are opened in the context of the current device. On multi-GPU systems, the caller needs to specify which GPU to open the handle on (the GPU that holds the memory). `cudaSetDevice` is lightweight and thread-safe.

**Alternatives considered**:
- Not supporting multi-GPU — rejected because multi-GPU inference servers are the primary use case.
- Using CUDA primary context API — rejected as overly complex for this use case.

## R5: Feature Gating

**Decision**: Gate `prepare_memory_for_spdk` behind `#[cfg(feature = "spdk")]` at the interface level (matching `dma_copy_to_host`/`dma_copy_to_device`) and require `gpu` feature for the implementation body.

**Rationale**: The function returns `interfaces::DmaBuffer` which is only available with `features = ["spdk"]`. The implementation uses CUDA FFI which requires `features = ["gpu"]`. Both features must be active.

**Alternatives considered**:
- Single combined feature — rejected to preserve independent feature flag semantics.

## R6: NUMA Node for DmaBuffer

**Decision**: Set `numa_node` to `-1` (unknown) when constructing `DmaBuffer::from_raw` for GPU memory, unless we can determine the GPU's NUMA locality from its PCI bus ID.

**Rationale**: GPU device memory doesn't have a NUMA node in the CPU sense, but the GPU's PCIe attachment point does. For the initial implementation, using `-1` is safe and matches how the existing code handles non-SPDK-allocated buffers. A future enhancement could look up NUMA affinity from the device's PCI topology.

**Alternatives considered**:
- Querying NUMA from PCI bus ID — deferred to future enhancement; not required by spec.
