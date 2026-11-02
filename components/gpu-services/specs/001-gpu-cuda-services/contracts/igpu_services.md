# Interface Contract: IGpuServices

**Location**: `components/interfaces/src/igpu_services.rs`
**Feature gate**: Available unconditionally (methods return error when
`gpu` feature not compiled)

## Trait Definition (target)

```rust
define_interface! {
    pub IGpuServices {
        /// Initialize CUDA libraries and discover qualifying GPUs.
        /// Returns Ok(()) on success or an error describing the failure.
        fn initialize(&self) -> Result<(), String>;

        /// Shut down CUDA context and release all resources.
        fn shutdown(&self) -> Result<(), String>;

        /// Return information about all discovered GPUs (compute >= 7.0).
        /// Must be called after initialize().
        fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String>;

        /// Deserialize a base64-encoded CUDA IPC handle and size.
        /// Input: base64 string encoding 72 bytes (64-byte handle + 8-byte LE size).
        /// Returns the deserialized handle ready for use.
        fn deserialize_ipc_handle(&self, base64_payload: &str)
            -> Result<IpcHandle, String>;

        /// Verify that GPU memory referenced by an IPC handle is
        /// contiguous device memory suitable for DMA.
        fn verify_memory(&self, handle: &IpcHandle) -> Result<(), String>;

        /// Pin GPU memory for DMA operations (idempotent).
        fn pin_memory(&self, handle: &IpcHandle) -> Result<(), String>;

        /// Unpin previously pinned GPU memory.
        fn unpin_memory(&self, handle: &IpcHandle) -> Result<(), String>;

        /// Create a DmaBuffer backed by GPU memory from an IPC handle.
        /// The handle must have been verified and pinned prior to this call.
        fn create_dma_buffer(&self, handle: IpcHandle)
            -> Result<DmaBuffer, String>;

        /// Copy data from GPU device memory to a DMA staging buffer.
        /// Performs synchronous cudaMemcpy DeviceToHost.
        #[cfg(feature = "spdk")]
        fn dma_copy_to_host(&self, src: *const c_void, dst: &DmaBuffer, size: usize)
            -> Result<(), String>;

        /// Copy data from a DMA staging buffer to GPU device memory.
        /// Performs synchronous cudaMemcpy HostToDevice.
        #[cfg(feature = "spdk")]
        fn dma_copy_to_device(&self, src: &DmaBuffer, dst: *mut c_void, size: usize)
            -> Result<(), String>;
    }
}
```

## Supporting Types (in interfaces crate)

```rust
/// GPU device information returned by get_devices().
#[derive(Debug, Clone)]
pub struct GpuDeviceInfo {
    pub device_index: u32,
    pub name: String,
    pub memory_bytes: u64,
    pub compute_major: u32,
    pub compute_minor: u32,
    pub pci_bus_id: String,
}
```

## Method Contracts

### initialize()

- **Preconditions**: None (may be called multiple times; idempotent)
- **Postconditions**: CUDA context active, device list populated
- **Errors**: "CUDA driver not found", "No qualifying GPU (compute
  7.0+) detected", "CUDA initialization failed: {detail}"
- **Performance**: <5 seconds

### shutdown()

- **Preconditions**: None (safe to call even if not initialized)
- **Postconditions**: All IPC handles closed, CUDA context released
- **Errors**: "Shutdown failed: {detail}" (resources leaked)
- **Performance**: <1 second

### get_devices()

- **Preconditions**: `initialize()` called successfully
- **Postconditions**: None (read-only query)
- **Errors**: "Not initialized"
- **Performance**: <1ms (returns cached data)

### deserialize_ipc_handle(base64_payload)

- **Preconditions**: `initialize()` called
- **Postconditions**: Returns valid IpcHandle with opened device pointer
- **Errors**: "Invalid base64", "Payload size != 72 bytes",
  "cudaIpcOpenMemHandle failed: {cuda_error}"
- **Performance**: <1ms

### verify_memory(handle)

- **Preconditions**: Valid IpcHandle from `deserialize_ipc_handle()`
- **Postconditions**: None (validation only)
- **Errors**: "Memory not device type", "Invalid pointer attributes",
  "Null handle"
- **Performance**: <10ms

### pin_memory(handle)

- **Preconditions**: Valid IpcHandle
- **Postconditions**: Memory page-locked for DMA
- **Errors**: "Pin failed: insufficient resources",
  "Invalid handle"
- **Performance**: <10ms

### unpin_memory(handle)

- **Preconditions**: Previously pinned handle
- **Postconditions**: Memory released from page-lock
- **Errors**: "Handle not pinned", "Unpin failed: {detail}"
- **Performance**: <10ms

### create_dma_buffer(handle)

- **Preconditions**: Handle verified AND pinned
- **Postconditions**: Ownership of handle transferred to DmaBuffer;
  IpcHandle consumed
- **Errors**: "Handle not verified", "Handle not pinned",
  "DmaBuffer creation failed: {detail}"
- **Performance**: <50ms

### dma_copy_to_host(src, dst, size)

- **Feature gate**: `spdk`
- **Preconditions**: `initialize()` called; `src` is a valid GPU device
  pointer for at least `size` bytes; `dst` is a valid DmaBuffer with
  `len() >= size`
- **Postconditions**: `size` bytes copied from GPU to DMA buffer
- **Errors**: "size exceeds destination buffer length",
  "Not initialized", "cudaMemcpy D2H failed: {cuda_error}"
- **Performance**: Depends on transfer size; <1ms for 4 KiB

### dma_copy_to_device(src, dst, size)

- **Feature gate**: `spdk`
- **Preconditions**: `initialize()` called; `src` is a valid DmaBuffer
  with `len() >= size`; `dst` is a valid GPU device pointer for at
  least `size` bytes
- **Postconditions**: `size` bytes copied from DMA buffer to GPU
- **Errors**: "size exceeds source buffer length",
  "Not initialized", "cudaMemcpy H2D failed: {cuda_error}"
- **Performance**: Depends on transfer size; <1ms for 4 KiB

## Methods added after this contract *(Sync 2026-10-31)*

The trait listing above is the original target. `IGpuServices` has since grown
(`components/interfaces/src/igpu_services.rs`); those methods are specified in
`spec.md`: `register_host_memory` / `unregister_host_memory` (FR-015/016), the
stream methods `create_stream` / `destroy_stream` / `stream_synchronize`
(FR-017) and `stream_query` (FR-023), `dma_copy_to_device_async` (FR-018),
`memcpy_h2d_async` (FR-019), `allocate_pinned_dma_buffer` (FR-020),
`set_device` / `device_of_ptr` (FR-021/022), `dma_copy_to_host_async` (FR-024),
`memcpy_d2h_async` (FR-025), `memcpy_batch_async` (FR-026), and
`prepare_memory_for_spdk` (FR-013, spec 002).

### memcpy_batch_async(dsts, srcs, sizes, stream)

- **Feature gate**: `spdk` (returns "GPU support not compiled" without `gpu`)
- **Preconditions**: `initialize()` called; `dsts`, `srcs`, `sizes` have equal
  length; entries do not overlap or depend on each other; every pointer valid
  for its size until `stream` is synchronized; host pointers CUDA-pinned;
  `stream` is a created stream, not the legacy NULL stream
- **Postconditions**: one `cudaMemcpyBatchAsync` (CUDA 13.x signature) queued on
  `stream`, all entries with `srcAccessOrder = Stream`; an empty batch returns
  `Ok(())` without a CUDA call. Completion is observed by synchronizing `stream`
- **Errors**: "Not initialized: call initialize() first",
  "memcpy_batch_async: slice lengths differ (...)",
  "cudaMemcpyBatchAsync failed: {cuda_error}"
- **Source**: `src/lib.rs:973-1038`, `src/cuda_ffi.rs:87-106`, `:157-167`
