# gpu-services (v0)

**Crate**: `gpu-services`
**Path**: `components/gpu-services/v0/`
**Version**: 0.1.0
**Features**: `gpu` (CUDA runtime FFI)

## Description

Wraps the CUDA runtime API to provide safe GPU memory access for DMA operations. Receives CUDA IPC memory handles from remote processes (e.g., a Python inference framework), verifies and pins the memory, and produces DMA-ready buffers that can be used by the storage subsystem.

In AI-native storage workloads, inference engines (PyTorch, TensorRT) hold model weights and activations in GPU memory. This component bridges that GPU memory into the Certus storage pipeline by:

1. Discovering NVIDIA GPUs with compute capability 7.0+ (Volta and newer)
2. Deserializing CUDA IPC handles exported by another process
3. Verifying the memory is device-allocated and contiguous
4. Pinning the memory for DMA transfer
5. Producing a `GpuDmaBuffer` that owns the IPC handle lifetime

All CUDA FFI calls are behind `#[cfg(feature = "gpu")]`. Without the feature, the crate compiles and links without `libcudart`; every operation returns a descriptive error.

## Component Definition

```
GpuServicesComponentV0 {
    version: "0.1.0",
    provides: [IGpuServices],
    receptacles: { logger: ILogger },
}
```

## Interfaces Provided

| Interface | Key Methods |
|-----------|------------|
| `IGpuServices` | `initialize()` -- load CUDA runtime, enumerate GPUs (idempotent) |
|                | `shutdown()` -- release all state and close handles |
|                | `get_devices() -> Result<Vec<GpuDeviceInfo>, String>` -- list qualifying GPUs |
|                | `deserialize_ipc_handle(base64) -> Result<GpuIpcHandle, String>` -- decode 72-byte base64 payload (64B handle + 8B LE size), open CUDA IPC handle |
|                | `verify_memory(handle) -> Result<(), String>` -- confirm pointer refers to device memory via `cudaPointerGetAttributes` |
|                | `pin_memory(handle) -> Result<(), String>` -- pin for DMA (idempotent, auto-verifies) |
|                | `unpin_memory(handle) -> Result<(), String>` -- unpin previously pinned memory |
|                | `create_dma_buffer(handle) -> Result<GpuDmaBuffer, String>` -- consume verified+pinned handle, return DMA buffer |

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging of initialization, verification, and DMA buffer creation |

## Key Types

- `GpuDeviceInfo` -- device index, name, memory size, compute capability (major/minor), PCI bus ID
- `GpuIpcHandle` -- opened IPC handle wrapping a device pointer with verification/pinning state
- `GpuDmaBuffer` -- owns GPU memory pointer; calls `cudaIpcCloseMemHandle` on drop

## Internal Modules

- `cuda_ffi` -- hand-written minimal CUDA runtime API FFI bindings (public for benchmark access)
- `device` -- GPU hardware discovery and filtering (compute capability >= 7.0)
- `ipc` -- base64 payload decoding and `cudaIpcOpenMemHandle` wrapper
- `memory` -- `cudaPointerGetAttributes` verification
- `dma` -- `GpuDmaBuffer` construction from verified+pinned handles

## Benchmarks

Criterion benchmarks measuring `cudaMemcpy` throughput across:
- Transfer sizes: 4 KiB to 64 MiB
- Directions: Host-to-Device and Device-to-Host
- All available GPU devices
- Pageable vs pinned (page-locked) host memory
