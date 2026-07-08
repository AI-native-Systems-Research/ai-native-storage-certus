# Feature Specification: GPU Services Component

**Feature Branch**: `001-gpu-services`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

GPU Services is a Certus component that provides safe access to NVIDIA GPU device memory for DMA operations within the storage system. It wraps the CUDA runtime API behind the `IGpuServices` interface, enabling the Certus storage subsystem to receive CUDA IPC memory handles from remote processes (e.g., PyTorch inference frameworks), verify and pin the GPU memory, and produce DMA-ready buffers that can be used by the NVMe block device layer for direct storage-to-GPU data transfer.

The component supports three tiers of functionality gated by Cargo features: basic GPU operations (`gpu` feature) for CUDA initialization, device discovery, IPC handle management, and memory transfers; SPDK integration (`spdk` feature) for NVMe-to-GPU DMA buffer creation with proper IOMMU registration; and true peer-to-peer (`p2p` feature) NVMe-to-GPU DMA via GDRCopy BAR1 mapping that bypasses host memory entirely. The component follows the Certus COM-style architecture with `define_component!`, provides `IGpuServices`, and accepts an optional `ILogger` receptacle.

## User Scenarios & Testing

### User Story 1 - GPU Device Discovery (Priority: P1)

As a storage system operator, I want to discover qualifying NVIDIA GPUs on the system so that I can confirm hardware readiness and select target devices for DMA operations.

**Acceptance Scenarios**:

- Given a system with NVIDIA GPUs with compute capability 7.0+, when `initialize()` is called, then it succeeds and `get_devices()` returns a non-empty list with correct device info (name, memory, PCI address, compute capability).
- Given a system with no GPUs or GPUs below compute capability 7.0, when `initialize()` is called, then it returns an error indicating no qualifying GPUs.
- Given `initialize()` has already succeeded, when `initialize()` is called again, then it returns `Ok(())` (idempotent).
- Given `initialize()` has not been called, when `get_devices()` is called, then it returns an error indicating the component is not initialized.

### User Story 2 - IPC Handle Deserialization and Memory Preparation (Priority: P1)

As a storage server process, I want to receive GPU memory handles from inference clients via IPC so that I can access their GPU memory for direct data transfer from NVMe storage.

**Acceptance Scenarios**:

- Given a valid base64-encoded 72-byte payload (64-byte cudaIpcMemHandle_t + 8-byte LE u64 size), when `deserialize_ipc_handle()` is called, then it returns a `GpuIpcHandle` with the correct size and a non-null device pointer.
- Given an invalid base64 string, when `deserialize_ipc_handle()` is called, then it returns an error containing "base64".
- Given a valid base64 string decoding to != 72 bytes, when `deserialize_ipc_handle()` is called, then it returns an error containing "72 bytes".
- Given the component is not initialized, when `deserialize_ipc_handle()` is called, then it returns "Not initialized" error.

### User Story 3 - Memory Verification and Pinning Lifecycle (Priority: P1)

As the storage DMA subsystem, I want to verify that GPU memory is device-type and pin it for DMA so that NVMe controllers can safely transfer data to/from GPU VRAM.

**Acceptance Scenarios**:

- Given a valid `GpuIpcHandle` pointing to device memory, when `verify_memory()` is called, then it succeeds and the handle is marked verified.
- Given a verified handle, when `pin_memory()` is called, then it succeeds and the handle is marked pinned.
- Given a pinned handle, when `pin_memory()` is called again, then it returns `Ok(())` (idempotent).
- Given a handle that is neither verified nor pinned, when `pin_memory()` is called, then it implicitly verifies first, then pins.
- Given a pinned handle, when `unpin_memory()` is called, then it succeeds.
- Given a non-pinned handle, when `unpin_memory()` is called, then it returns an error.
- Given a verified and pinned handle, when `create_dma_buffer()` is called, then it returns a `GpuDmaBuffer` with the correct size.
- Given a handle that is not verified, when `create_dma_buffer()` is called, then it returns an error.
- Given a handle that is verified but not pinned, when `create_dma_buffer()` is called, then it returns an error.

### User Story 4 - SPDK DMA Buffer Preparation (Priority: P1)

As the Certus dispatch layer, I want a one-call API to prepare GPU IPC memory for NVMe DMA so that I can efficiently serve inference read requests without manual handle lifecycle management.

**Acceptance Scenarios**:

- Given a valid IPC payload and an initialized component, when `prepare_memory_for_spdk()` is called, then it returns a `DmaBuffer` registered with SPDK, ready for NVMe DMA.
- Given memory that is already pinned, when `prepare_memory_for_spdk()` is called, then it skips pinning and the buffer's drop handler only closes the IPC handle (no unpin).
- Given memory that is not yet pinned, when `prepare_memory_for_spdk()` is called, then it pins the memory and the buffer's drop handler unpins then closes the IPC handle.
- Given an optional `device_index`, when `prepare_memory_for_spdk()` is called, then it sets the CUDA device context before opening the IPC handle and restores it afterward.
- Given any error during preparation, when the function returns `Err`, then no GPU resources are leaked (IPC handle closed, pin state rolled back).
- Given nvidia-peermem kernel module is not loaded, when `prepare_memory_for_spdk()` attempts SPDK registration, then it returns an error mentioning nvidia-peermem.

### User Story 5 - Synchronous DMA Transfers (Priority: P1)

As the hot-path data mover, I want to copy data between GPU device memory and SPDK DMA buffers so that NVMe read/write data reaches GPU VRAM for inference.

**Acceptance Scenarios**:

- Given a valid GPU device pointer and a DMA buffer, when `dma_copy_to_host()` is called with `size <= dst.len()`, then it copies `size` bytes from GPU to host DMA buffer.
- Given `size > dst.len()`, when `dma_copy_to_host()` is called, then it returns an error without performing the copy.
- Given a valid DMA buffer and GPU pointer, when `dma_copy_to_device()` is called with `size <= src.len()`, then it copies `size` bytes from host to GPU.
- Given `size > src.len()`, when `dma_copy_to_device()` is called, then it returns an error.
- Given the component is not initialized, when either copy function is called, then it returns "Not initialized" error.

### User Story 6 - Asynchronous DMA Transfers with Streams (Priority: P2)

As the pipelined data path, I want to issue asynchronous GPU memory copies on CUDA streams so that I can overlap NVMe I/O with GPU DMA transfers for higher throughput.

**Acceptance Scenarios**:

- Given an initialized component, when `create_stream()` is called, then it returns a valid `GpuStream`.
- Given a valid stream, when `stream_query()` is called with no pending work, then it returns `Ok(true)`.
- Given a stream with in-flight async work, when `stream_query()` is called, then it returns `Ok(false)`.
- Given a valid stream, when `stream_synchronize()` is called, then it blocks until all queued work completes.
- Given a valid stream, when `destroy_stream()` is called, then the stream resources are released.
- Given a valid stream and valid pointers, when `dma_copy_to_device_async()` is called, then it enqueues a H2D copy on the stream without blocking.
- Given a valid stream and valid pointers, when `dma_copy_to_host_async()` is called, then it enqueues a D2H copy on the stream without blocking.
- Given a valid stream, when `memcpy_h2d_async()` or `memcpy_d2h_async()` is called with raw pointers, then it enqueues the copy without requiring DmaBuffer wrappers.

### User Story 7 - Pinned Host Memory Allocation and Registration (Priority: P2)

As the pipeline ring buffer allocator, I want to allocate page-locked host memory that is registered with both CUDA and SPDK so that NVMe can DMA into it and GPU can async-copy from it without page faults.

**Acceptance Scenarios**:

- Given an initialized component, when `allocate_pinned_dma_buffer(size)` is called, then it returns a `DmaBuffer` backed by `cudaHostAlloc` memory registered with SPDK.
- Given the returned buffer, when it is dropped, then `cudaFreeHost` is called after SPDK unregistration.
- Given a pre-allocated host buffer, when `register_host_memory(ptr, size)` is called, then it registers with both `cudaHostRegister` and `spdk_mem_register`.
- Given already-registered memory, when `register_host_memory()` is called again, then it succeeds (CUDA reports already-registered, which is tolerated).
- Given registered memory, when `unregister_host_memory()` is called, then it unregisters from SPDK first, then from CUDA.

### User Story 8 - GPU-Direct P2P NVMe-to-GPU Transfer (Priority: P2)

As a performance engineer, I want true peer-to-peer NVMe-to-GPU DMA that bypasses host memory so that I can achieve maximum storage-to-GPU bandwidth for large model weight loading.

**Acceptance Scenarios**:

- Given a `cudaMalloc`'d GPU pointer with gdrdrv and nvidia-peermem loaded, when `create_spdk_dma_buffer_from_gpu_bar()` is called, then it pins via GDRCopy, maps BAR1, registers with SPDK, and returns a DMA buffer whose pointer is the BAR1 VA.
- Given the BAR1 DMA buffer, when an NVMe read is issued targeting it, then data lands directly in GPU VRAM via PCIe P2P.
- Given the BAR1 DMA buffer is dropped, then GDRCopy unmap/unpin and SPDK unregister are performed in correct order.
- Given a physical address from a remote GPU's BAR1, when `create_spdk_dma_buffer_from_phys()` is called, then it mmaps anonymous VA, registers with DPDK extmem, and programs VFIO IOMMU DMA mapping.
- Given the P2P server binary in bounce mode, when a client sends an IPC handle, then NVMe reads into host DMA buffers and cudaMemcpy H2D copies to client GPU.
- Given the P2P server binary in p2p mode, when a client sends an IPC handle, then NVMe reads into pre-pinned GPU staging chunks and D2D copies to client GPU.
- Given the P2P server binary in p2p-cold mode, when a client sends an IPC handle, then per-request GDRCopy pin/unpin staging is used (baseline measurement).

### User Story 9 - Graceful Degradation Without GPU Hardware (Priority: P3)

As a developer building on a machine without NVIDIA GPUs, I want the component to compile and return clear errors at runtime so that CI and development environments without GPUs still function.

**Acceptance Scenarios**:

- Given the crate is built without `--features gpu`, when any operation is called, then it returns an error containing "GPU support not compiled".
- Given the `gpu` feature is enabled but no GPU hardware is present, when `initialize()` is called, then it returns a descriptive CUDA error.
- Given the component is never initialized, when `shutdown()` is called, then it returns `Ok(())`.

## Requirements

### Functional Requirements

- **FR-001**: The component SHALL implement `IGpuServices` as defined in `components/interfaces/src/igpu_services.rs`.
- **FR-002**: `initialize()` SHALL enumerate all NVIDIA GPUs via CUDA runtime API and filter to compute capability >= 7.0 (Volta+).
- **FR-003**: `initialize()` SHALL be idempotent -- calling it when already initialized returns `Ok(())`.
- **FR-004**: `shutdown()` SHALL clear all internal state (devices, verified set, pinned set) and mark the component uninitialized.
- **FR-005**: `get_devices()` SHALL return `GpuDeviceInfo` including device index, name, memory bytes, compute major/minor, and PCI bus ID.
- **FR-006**: `deserialize_ipc_handle()` SHALL decode base64 input, validate exactly 72 bytes (64 handle + 8 size), validate size > 0, and call `cudaIpcOpenMemHandle`.
- **FR-007**: `verify_memory()` SHALL call `cudaPointerGetAttributes` and confirm memory type is `cudaMemoryTypeDevice`.
- **FR-008**: `pin_memory()` SHALL implicitly verify if not already verified, then mark as pinned. It SHALL be idempotent.
- **FR-009**: `create_dma_buffer()` SHALL reject handles that are not both verified and pinned.
- **FR-010**: `dma_copy_to_host()` and `dma_copy_to_device()` SHALL validate size against buffer length before issuing `cudaMemcpy`.
- **FR-011**: `prepare_memory_for_spdk()` SHALL perform the full IPC-open, verify, pin, SPDK-register lifecycle in one call with rollback on error.
- **FR-012**: `prepare_memory_for_spdk()` SHALL support optional device context switching via `device_index` parameter.
- **FR-013**: CUDA stream operations (`create_stream`, `destroy_stream`, `stream_query`, `stream_synchronize`) SHALL wrap the corresponding CUDA runtime stream APIs.
- **FR-014**: Async copy operations SHALL issue `cudaMemcpyAsync` on the provided stream without blocking.
- **FR-015**: `allocate_pinned_dma_buffer()` SHALL allocate via `cudaHostAlloc` and register with `spdk_mem_register`.
- **FR-016**: `register_host_memory()` SHALL call `cudaHostRegister` then `spdk_mem_register`, tolerating already-registered CUDA state.
- **FR-017**: `unregister_host_memory()` SHALL call `spdk_mem_unregister` before `cudaHostUnregister` (reverse order of registration).
- **FR-018**: The P2P path SHALL use GDRCopy (`gdr_pin_buffer`, `gdr_map`) to create BAR1 mappings of GPU memory for NVMe DMA.
- **FR-019**: The P2P path SHALL register BAR1 mappings with SPDK for vtophys resolution and VFIO IOMMU programming.
- **FR-020**: All DMA buffer drop handlers SHALL properly clean up resources in reverse acquisition order (SPDK unregister, GDRCopy unmap/unpin, CUDA IPC close/free).
- **FR-021**: The `gpu-p2p-server` binary SHALL accept CUDA IPC handles over a Unix domain socket and support bounce, p2p, and p2p-cold transfer modes.

### Non-Functional Requirements

- **NFR-001**: Without the `gpu` feature enabled, the crate SHALL compile and link without requiring `libcudart`, `libgdrapi`, or any NVIDIA libraries.
- **NFR-002**: All CUDA FFI bindings SHALL be hand-written (not bindgen-generated) covering only the minimal required API surface for auditability.
- **NFR-003**: All unsafe code blocks SHALL include `// SAFETY:` justification comments.
- **NFR-004**: DMA transfer throughput SHALL be benchmarked with Criterion across transfer sizes from 4 KiB to 64 MiB, both directions, pageable vs pinned memory.
- **NFR-005**: The component SHALL support multi-GPU systems by allowing device context selection via `cudaSetDevice`.
- **NFR-006**: GPU memory alignment for GDRCopy operations SHALL be 64 KiB (GPU page size).
- **NFR-007**: The P2P server SHALL handle SIGINT/SIGTERM for graceful shutdown.
- **NFR-008**: The component SHALL log significant operations (init, device count, IPC deserialize, pin, DMA buffer creation) when an `ILogger` receptacle is connected.
- **NFR-009**: Logger absence SHALL NOT cause panics or failures -- all logger access is fallible and gracefully skipped.
- **NFR-010**: Thread safety SHALL be ensured via `Mutex<GpuState>` for component state and `Send + Sync` bounds on all public types.
- **NFR-011**: The P2P server SHALL use chunked I/O (configurable chunk size, default 128 KiB) to respect NVMe MDTS limits.

## Key Entities

| Entity | Description |
|--------|-------------|
| `GpuServicesComponent` | The COM-style component implementing `IGpuServices`. Uses `define_component!` macro. Contains `Mutex<GpuState>` for thread-safe state. |
| `GpuState` | Internal state: initialization flag, discovered devices, verified pointer set, pinned pointer set. |
| `GpuDeviceInfo` | Value object describing a discovered GPU: index, name, memory, compute capability, PCI address. |
| `GpuIpcHandle` | Wrapper around a CUDA IPC-opened device pointer with size, verified, and pinned flags. |
| `GpuDmaBuffer` | Owned GPU device memory buffer with custom drop (calls `cudaIpcCloseMemHandle`). |
| `GpuStream` | Opaque wrapper around `cudaStream_t` for async operations. |
| `DmaBuffer` | SPDK-registered host or device memory buffer (from interfaces crate) with custom free function. |
| `GdrMappingState` | Tracks GDRCopy handle, memory handle, BAR pointer, and size for cleanup on drop. |
| `PhysMappingState` | Tracks mmap'd VA, physical address, and size for DPDK/VFIO cleanup. |
| `REGISTERED_REGIONS` | Global registry mapping pointers to sizes for free functions that lack size parameters. |

## Dependencies

### Internal Dependencies

| Crate | Role |
|-------|------|
| `component-framework` | `define_component!` macro and core traits |
| `component-core` | `IUnknown`, `query_interface!`, receptacle binding |
| `component-macros` | `define_interface!` proc macro |
| `interfaces` | `IGpuServices` trait, `GpuDeviceInfo`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`, `DmaBuffer`, `ILogger` |
| `logger` | Default `ILogger` implementation (dev-dependency for tests) |
| `spdk-sys` | SPDK FFI (optional, `spdk` feature) |
| `spdk-env` | SPDK environment initialization (optional, `spdk` feature) |
| `block-device-spdk-nvme` | NVMe block device driver (optional, `spdk` feature, used by P2P server) |

### External Dependencies

| Library | Feature Gate | Purpose |
|---------|-------------|---------|
| `libcudart` (CUDA Runtime) | `gpu` | Core CUDA API: device management, IPC, memcpy, streams, host memory |
| `libgdrapi` (GDRCopy) | `p2p` | GPU BAR1 pinning and mapping for true P2P DMA |
| `nvidia-peermem` (kernel module) | `spdk`, `p2p` | Enables GPU device memory visibility to IOMMU for NVMe DMA |
| `gdrdrv` (kernel module) | `p2p` | GDRCopy kernel driver for GPU page pinning |
| `base64` | `gpu` | IPC handle payload encoding/decoding |
| `libc` | `gpu` | System calls (mmap, signal handling) |
| `clap` | `gpu` (binary) | CLI argument parsing for P2P server |

### Kernel Module Requirements (Runtime)

- `nvidia-peermem`: Required for SPDK to register GPU device memory (`spdk_mem_register` on GPU pointers)
- `gdrdrv`: Required for GDRCopy BAR1 mapping (`p2p` feature only)
- VFIO with IOMMU: Required for NVMe DMA to GPU memory

## Success Criteria

1. **Interface compliance**: Component passes `query_interface!(component, IGpuServices)` and all interface methods are callable.
2. **Graceful degradation**: Without `gpu` feature, all operations return descriptive errors; the crate compiles without CUDA libraries.
3. **Memory safety**: No resource leaks on error paths; all GPU pointers are properly closed/freed via RAII drop handlers.
4. **Data integrity**: CPU-to-GPU-to-CPU round-trip produces identical data (verified in `test_dma_cpu_to_gpu_roundtrip`).
5. **P2P correctness**: NVMe-to-GPU P2P DMA via GDRCopy BAR1 produces data verifiable via `cudaMemcpy D2H` (verified in `test_nvme_to_gpu_p2p_gdrcopy`).
6. **Cross-process P2P**: Data written by NVMe P2P is accessible from a separate Python process via CUDA IPC (verified in `test_nvme_to_gpu_p2p_python_client`).
7. **Idempotency**: `initialize()` and `pin_memory()` are safe to call multiple times.
8. **Error rollback**: `prepare_memory_for_spdk()` rolls back all state (IPC close, unpin, device restore) on any error.
9. **Benchmarks pass**: Criterion DMA transfer benchmarks run without error on GPU-equipped systems.
10. **Lint clean**: `cargo clippy -- -D warnings` and `cargo doc --no-deps` produce no warnings.

## Implementation Notes

- The IPC handle payload format is: 64 bytes of `cudaIpcMemHandle_t.reserved` followed by 8 bytes of little-endian u64 buffer size, totaling 72 bytes, base64-encoded into a single string.
- GDRCopy requires memory allocated by the current process (`cudaMalloc`). IPC-opened memory (`cudaIpcOpenMemHandle`) cannot be GDRCopy-pinned directly. For cross-process P2P, the allocating process must perform GDRCopy pin+map and share BAR mapping info.
- The `REGISTERED_REGIONS` static is necessary because SPDK's `DmaBuffer` free function signature is `fn(*mut c_void)` with no size parameter, so sizes must be looked up at free time.
- GPU page alignment is 64 KiB (`GPU_PAGE_SIZE = 1 << 16`). GDRCopy operations align allocations up to this boundary.
- The P2P server uses `atexit` to hook `_exit(0)` to prevent SPDK's atexit teardown from crashing when the test harness exits.
- `cudaDeviceProp` struct includes a 4096-byte padding field because the actual CUDA 13.0 struct is over 1 KiB and we only read `name`, `total_global_mem`, `major`, and `minor`.
- The `create_spdk_dma_buffer_from_bar_direct` function uses identity IOVA mapping (VA = IOVA) for VFIO container DMA programming, suitable for BAR regions already mapped in the process page table.
- Feature gate hierarchy: `p2p` implies `gpu` + `spdk`. The `spdk` feature alone does not imply `gpu`.
