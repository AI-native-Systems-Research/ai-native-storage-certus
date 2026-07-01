# gpu-services (v0)

**Crate**: `gpu-services`
**Path**: `components/gpu-services/`
**Version**: 0.1.0
**Features**: `gpu` (CUDA runtime FFI), `spdk` (DMA copy methods and `prepare_memory_for_spdk`)

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
GpuServicesComponent {
    version: "0.1.0",
    provides: [IGpuServices],
    receptacles: {
        logger: ILogger,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IGpuServices {
        fn initialize(&self) -> Result<(), String>;
        fn shutdown(&self) -> Result<(), String>;
        fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String>;
        fn deserialize_ipc_handle(&self, base64_payload: &str) -> Result<GpuIpcHandle, String>;
        fn verify_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>;
        fn pin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>;
        fn unpin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>;
        fn create_dma_buffer(&self, handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String>;
        fn create_stream(&self) -> Result<GpuStream, String>;
        fn destroy_stream(&self, stream: GpuStream) -> Result<(), String>;
        fn stream_query(&self, stream: GpuStream) -> Result<bool, String>;
        fn stream_synchronize(&self, stream: GpuStream) -> Result<(), String>;

        // spdk feature methods:
        fn dma_copy_to_host(&self, src: *const c_void, dst: &DmaBuffer, size: usize) -> Result<(), String>;
        fn dma_copy_to_device(&self, src: &DmaBuffer, dst: *mut c_void, size: usize) -> Result<(), String>;
        fn dma_copy_to_device_async(&self, src: &DmaBuffer, dst: *mut c_void, size: usize, stream: GpuStream) -> Result<(), String>;
        fn memcpy_h2d_async(&self, src: *const c_void, dst: *mut c_void, size: usize, stream: GpuStream) -> Result<(), String>;
        fn dma_copy_to_host_async(&self, src: *const c_void, dst: &DmaBuffer, size: usize, stream: GpuStream) -> Result<(), String>;
        fn memcpy_d2h_async(&self, src: *const c_void, dst: *mut c_void, size: usize, stream: GpuStream) -> Result<(), String>;
        fn prepare_memory_for_spdk(&self, base64_payload: &str, device_index: Option<u32>) -> Result<DmaBuffer, String>;
        fn allocate_pinned_dma_buffer(&self, size: usize) -> Result<DmaBuffer, String>;
        fn register_host_memory(&self, ptr: *mut c_void, size: usize) -> Result<(), String>;
        fn unregister_host_memory(&self, ptr: *mut c_void, size: usize) -> Result<(), String>;
    }
}
```

## Verified Properties

The following invariants are formally proved with Creusot (see `components/gpu-services/verif/`):

| ID | Name | Description |
|----|------|-------------|
| P1 | init-guard | operations fail when not initialized |
| P2 | init-idempotent | calling `initialize()` twice succeeds |
| P3 | shutdown-clears | shutdown sets initialized to false |
| P4 | handle-state-machine | IPC handle transitions: fresh→verified→pinned |
| P5 | verify-ptr-valid | `verify_memory` requires non-null device pointer |
| P6 | pin-requires-verified | `pin_memory` requires verified == true |
| P7 | dma-requires-pinned | `create_dma_buffer` requires pinned == true |
| P8 | copy-size-check | DMA copy rejects size > buffer length |
| P9 | stream-lifecycle | create_stream/destroy_stream paired correctly |
| P10 | register-lifecycle | register/unregister host memory paired |

Total: 10 properties, 19 verification conditions discharged by SMT solvers.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging of initialization, verification, and DMA buffer creation |

## Key Types

- `GpuDeviceInfo` — device index, name, memory size, compute capability (major/minor), PCI bus ID
- `GpuIpcHandle` — opened IPC handle wrapping a device pointer with verification/pinning state
- `GpuDmaBuffer` — owns GPU memory pointer; calls `cudaIpcCloseMemHandle` on drop
- `GpuStream` — opaque handle to a CUDA stream for async GPU operations
