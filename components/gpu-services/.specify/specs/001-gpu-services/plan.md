# Implementation Plan: GPU Services

**Branch**: `001-gpu-services` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

GPU Services provides safe CUDA runtime access for NVMe-to-GPU DMA within the Certus storage system. It wraps the CUDA runtime API behind an `IGpuServices` COM-style interface, enabling IPC handle deserialization from inference clients, GPU memory verification/pinning, synchronous and asynchronous DMA transfers, and true peer-to-peer NVMe-to-GPU data movement via GDRCopy BAR1 mapping. The implementation is feature-gated into three tiers: basic GPU operations (`gpu`), SPDK integration (`spdk`), and P2P DMA (`p2p`).

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**: CUDA Runtime API (libcudart, hand-written FFI), GDRCopy (libgdrapi, hand-written FFI), SPDK (via spdk-sys crate), DPDK (rte_extmem/VFIO APIs), base64, libc, clap
**Storage**: NVMe block devices via SPDK userspace driver (block-device-spdk-nvme)
**Testing**: `cargo test -p gpu-services` (unit), `cargo test -p gpu-services --features p2p` (integration, requires hardware), Criterion benchmarks
**Target Platform**: Linux only (RHEL/Fedora), NVIDIA GPUs with compute capability 7.0+ (Volta and newer)
**Project Type**: Library component + binary (`gpu-p2p-server`)
**Performance Goals**: Saturate PCIe bandwidth for NVMe-to-GPU transfers; benchmarked across 4 KiB to 64 MiB transfer sizes
**Constraints**: 64 KiB GPU page alignment for GDRCopy, 128 KiB default NVMe chunk size (MDTS), `nvidia-peermem` and `gdrdrv` kernel modules required at runtime for P2P
**Scale/Scope**: Single-node, multi-GPU; designed for inference serving with multiple concurrent IPC handles

## Architecture

### Component Layer

```
+---------------------------------------------------------------------+
|                       Certus Dispatch Layer                          |
|  (issues prepare_memory_for_spdk / dma_copy_to_device_async calls)  |
+---------------------------------------------------------------------+
        |                                              |
        v                                              v
+---------------------------+      +------------------------------------+
|    IGpuServices           |      |     IBlockDevice (NVMe)            |
|  (gpu-services component) |      |  (block-device-spdk-nvme)          |
+---------------------------+      +------------------------------------+
        |                                              |
        |  Receptacle: ILogger (optional)               |  Receptacle: ISPDKEnv
        |                                              |
        v                                              v
+---------------------------+      +------------------------------------+
|   CUDA Runtime API        |      |     SPDK / DPDK / VFIO             |
|  (libcudart.so)           |      |   (hugepages, IOMMU, NVMe driver)  |
+---------------------------+      +------------------------------------+
        |                                              |
        v                                              v
+---------------------------+      +------------------------------------+
|   NVIDIA GPU Hardware     | <--- |     NVMe SSD (PCIe P2P DMA)        |
|  (VRAM, BAR1 aperture)   |      |                                    |
+---------------------------+      +------------------------------------+
        |
        v (P2P feature only)
+---------------------------+
|   GDRCopy (libgdrapi)     |
|  (gdrdrv kernel module)   |
|  BAR1 pin + map           |
+---------------------------+
```

### Internal Module Structure

```
components/gpu-services/
├── Cargo.toml                     # Feature gates: gpu, spdk, p2p
├── build.rs                       # Link-time: libcudart, libgdrapi search paths
├── src/
│   ├── lib.rs                     # Component definition (define_component!), IGpuServices impl
│   ├── cuda_ffi.rs                # Hand-written CUDA runtime FFI bindings (minimal API surface)
│   ├── device.rs                  # GPU discovery: enumerate + filter by compute cap >= 7.0
│   ├── dma.rs                     # DMA buffer creation (5 variants), free functions, REGISTERED_REGIONS
│   ├── gdrcopy_ffi.rs             # Hand-written GDRCopy FFI bindings (pin/map/unpin/unmap)
│   ├── ipc.rs                     # IPC handle decode (base64 → 64+8 bytes) and open
│   ├── memory.rs                  # GPU pointer attribute verification (device type check)
│   └── bin/
│       └── p2p_server.rs          # Unix socket server: bounce/p2p/p2p-cold transfer modes
├── benches/
│   ├── dma_transfer_benchmark.rs  # Criterion: H2D/D2H, pageable vs pinned, 4K-64M
│   └── gpu_services_benchmark.rs  # Component-level benchmarks
├── tests/
│   └── gpu_nvme_p2p.rs            # Integration: true NVMe→GPU P2P via GDRCopy + Python cross-process
└── info/
    └── FUNCTIONAL-DESIGN.md       # Design documentation
```

### Data Flow / Key Paths

**Path 1: IPC Handle Lifecycle (Bounce Mode)**

```
Client (PyTorch) → base64(IPC handle + size) → gRPC/socket
    → deserialize_ipc_handle() → cudaIpcOpenMemHandle → GpuIpcHandle
    → verify_memory() → cudaPointerGetAttributes → mark verified
    → pin_memory() → mark pinned
    → create_dma_buffer() → GpuDmaBuffer (owns pointer, closes on drop)
```

**Path 2: One-Shot SPDK Preparation (Hot Path)**

```
base64 payload → prepare_memory_for_spdk(payload, device_index)
    → [optional] cudaSetDevice(idx) / cudaGetDevice(save original)
    → decode_ipc_payload → open_ipc_handle → cudaIpcOpenMemHandle
    → check pin state → [if unpinned] check_memory_attributes + mark pinned
    → create_spdk_dma_buffer_from_gpu(ptr, size, was_already_pinned)
        → spdk_mem_register(ptr, size) [requires nvidia-peermem]
        → DmaBuffer::from_raw(ptr, size, free_fn, -1)
    → [restore original device]
    → return DmaBuffer (NVMe can DMA directly to GPU VRAM)
```

**Path 3: True P2P NVMe→GPU (GDRCopy BAR1)**

```
cudaMalloc(dev_ptr, size)
    → create_spdk_dma_buffer_from_gpu_bar(dev_ptr, size)
        → align up to 64 KiB GPU page boundary
        → gdr_open() → gdr_pin_buffer(dev_ptr) → nvidia_p2p_get_pages
        → gdr_map() → BAR1 VA (has valid pagemap → physical GPU BAR)
        → spdk_mem_register(bar_ptr) → IOMMU DMA mapping
        → DmaBuffer::from_raw(effective_bar_ptr, size, cleanup_fn)
    → NVMe ReadSync into DmaBuffer → data lands in GPU VRAM via PCIe P2P
    → Drop: SPDK unregister → GDRCopy unmap → unpin → close
```

**Path 4: Async Pipeline (Stream-Based)**

```
allocate_pinned_dma_buffer(size) → cudaHostAlloc + spdk_mem_register → DmaBuffer
    → NVMe ReadAsync into DmaBuffer (NVMe DMA engine)
    → dma_copy_to_device_async(buf, gpu_ptr, size, stream) → cudaMemcpyAsync H2D
    → [overlap: next NVMe read into another buffer]
    → stream_synchronize(stream) → GPU data ready
```

**Path 5: P2P Server (Unix Socket)**

```
Client connects → sends base64(IPC handle + size) via newline-delimited text
Server:
    bounce mode: NVMe → host DMA bufs (chunked) → cudaMemcpy H2D → client GPU
    p2p mode:    NVMe → pre-pinned GPU staging (GDRCopy pool) → cudaMemcpy D2D → client GPU
    p2p-cold:    NVMe → per-request GDRCopy pin → cudaMemcpy D2D → client GPU (measures overhead)
Server responds: "OK N bytes (mode, K chunks)" or "ERROR: ..."
```

### Key Design Decisions

1. **Hand-written FFI over bindgen**: CUDA and GDRCopy bindings are manually written to minimize API surface for auditability (NFR-002). Only the ~25 functions actually called are declared.

2. **Feature-gate hierarchy**: `p2p` implies `gpu` + `spdk`. The `spdk` feature does NOT imply `gpu`. Without `gpu`, the crate compiles without any NVIDIA library dependency, enabling CI on GPU-less machines.

3. **REGISTERED_REGIONS static**: SPDK's `DmaBuffer` free function signature is `fn(*mut c_void)` without size. A global `OnceLock<Mutex<HashMap<usize, usize>>>` maps pointer addresses to their sizes so free functions can call `spdk_mem_unregister(ptr, size)` correctly.

4. **GDR_MAPPINGS / PHYS_MAPPINGS statics**: Same pattern for GDRCopy BAR state and physical/VFIO mapping state, enabling proper reverse-order cleanup on drop.

5. **Three DMA buffer free functions**: `spdk_unregister_and_ipc_close` (already-pinned), `spdk_unregister_unpin_and_ipc_close` (we-pinned), `spdk_unregister_gdr_unmap_and_close` (GDRCopy P2P). Each performs cleanup in reverse acquisition order.

6. **64 KiB GPU page alignment**: GDRCopy requires allocations aligned to GPU page boundaries. `create_spdk_dma_buffer_from_gpu_bar` aligns up and applies an offset to the returned BAR pointer.

7. **`atexit` hook in P2P server/tests**: SPDK's own atexit teardown crashes when components outlive it. The workaround calls `_exit(0)` in an atexit hook to prevent SPDK's handler from running.

8. **Chunked NVMe I/O**: The P2P server respects NVMe MDTS (Maximum Data Transfer Size) by breaking large transfers into configurable chunks (default 128 KiB), issuing concurrent async reads via `BatchSubmit`.

9. **Thread safety via `Mutex<GpuState>`**: Component state (devices, verified set, pinned set) is protected by a single mutex. GPU pointers themselves are safe across threads once opened.

10. **Identity IOVA mapping for BAR direct**: `create_spdk_dma_buffer_from_bar_direct` uses VA = IOVA for VFIO container DMA programming, suitable for GDRCopy BAR regions already mapped in the process page table.

## Dependencies

### Internal Crate Dependencies

| Crate | Role | Feature Gate |
|-------|------|-------------|
| `component-framework` | `define_component!` macro, facade re-export | always |
| `component-core` | `IUnknown`, `query_interface!`, receptacle binding | always |
| `component-macros` | `define_interface!` proc macro | always |
| `interfaces` | `IGpuServices` trait, types (`GpuDeviceInfo`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`, `DmaBuffer`, `ILogger`) | always (with `gpu`/`spdk` sub-features) |
| `logger` | Default `ILogger` implementation | dev-dependency |
| `spdk-sys` | SPDK FFI bindings | `spdk` |
| `spdk-env` | SPDK environment init, device enumeration | `spdk` (binary) |
| `block-device-spdk-nvme` | NVMe block device driver | `spdk` (binary) |

### External Library Dependencies

| Library | Feature Gate | Link Type | Purpose |
|---------|-------------|-----------|---------|
| `libcudart` | `gpu` | dynamic | CUDA runtime: device mgmt, IPC, memcpy, streams, host alloc |
| `libgdrapi` | `p2p` | dynamic | GDRCopy: GPU BAR1 pin + map for true P2P DMA |
| `base64` | `gpu` | Rust crate | IPC handle payload encoding/decoding |
| `libc` | `gpu` | Rust crate | mmap, signal handling (P2P server) |
| `clap` | `gpu` | Rust crate | CLI argument parsing (P2P server binary) |

### Kernel Module Requirements (Runtime)

| Module | Feature | Purpose |
|--------|---------|---------|
| `nvidia-peermem` | `spdk`, `p2p` | GPU device memory visibility to IOMMU for NVMe DMA |
| `gdrdrv` | `p2p` | GDRCopy kernel driver for GPU page pinning (nvidia_p2p_get_pages) |
| VFIO + IOMMU | `spdk`, `p2p` | NVMe DMA to GPU memory via IOMMU programming |

## Testing

### Unit Tests (no hardware required)

| Test | Feature | Validates |
|------|---------|-----------|
| `test_provides_igpu_services` | none | Component exposes IGpuServices via query_interface |
| `test_initialize_without_logger` | none | Graceful error without GPU hardware |
| `test_shutdown_without_logger` | none | shutdown() always succeeds |
| `test_get_devices_before_init_fails` | none | Pre-init guard on get_devices() |
| `test_initialize_with_logger` | none | Logger receptacle does not interfere |
| `test_initialize_idempotent` | `gpu` | Second initialize() call succeeds |
| `test_shutdown_releases_state` | `gpu` | State cleared after shutdown |
| `test_deserialize_invalid_base64` | `gpu` | Error contains "base64" |
| `test_deserialize_wrong_payload_size` | `gpu` | Error contains "72 bytes" |
| `test_dma_cpu_to_gpu_roundtrip` | `gpu` | CPU→GPU→CPU data integrity (cudaMemcpy) |
| `test_prepare_memory_not_initialized` | `gpu+spdk` | Pre-init guard on prepare_memory_for_spdk |
| `test_prepare_memory_invalid_base64` | `gpu+spdk` | Error propagation for bad base64 |
| `test_prepare_memory_wrong_payload_size` | `gpu+spdk` | Error propagation for bad payload length |
| `test_prepare_memory_succeeds_without_logger` | `gpu+spdk` | No panic when logger disconnected |
| `test_prepare_memory_logs_with_logger` | `gpu+spdk` | Logger path does not interfere with error handling |

### Integration Tests (requires hardware)

| Test | Feature | Validates |
|------|---------|-----------|
| `test_nvme_to_gpu_p2p_gdrcopy` | `p2p` | End-to-end NVMe→GPU P2P via GDRCopy BAR1 |
| `test_nvme_to_gpu_p2p_python_client` | `p2p` | Cross-process verification via Python CUDA IPC |
| `test_nvme_to_gpu_p2p_explicit_iommu` | `p2p` | Decomposed GDRCopy+SPDK registration path |

### Benchmarks (Criterion)

| Benchmark | Feature | Measures |
|-----------|---------|----------|
| `dma_h2d_pageable` | `gpu` | Host→Device throughput with pageable memory (4K-64M) |
| `dma_d2h_pageable` | `gpu` | Device→Host throughput with pageable memory (4K-64M) |
| `dma_h2d_pinned` | `gpu` | Host→Device throughput with pinned memory (4K-64M) |
| `dma_d2h_pinned` | `gpu` | Device→Host throughput with pinned memory (4K-64M) |

### Test Execution

```bash
# Unit tests (no GPU required)
cargo test -p gpu-services

# GPU-dependent unit tests
cargo test -p gpu-services --features gpu

# Full integration (requires GPU + NVMe + kernel modules)
cargo test -p gpu-services --features p2p

# Benchmarks
cargo bench -p gpu-services --features gpu --bench dma_transfer_benchmark
```

## Future Considerations

1. **Multi-stream pipeline**: The current async API provides building blocks; a higher-level pipeline abstraction (double/triple buffering with stream overlap) is not yet implemented in this component.

2. **GPU memory pool / arena**: Currently each `prepare_memory_for_spdk` call opens a new IPC handle. A pooled approach could amortize IPC open/close overhead for repeated accesses to the same GPU buffer.

3. **Cross-node P2P via RDMA**: The `remote-lookup` component placeholder suggests future multi-node GPU-direct-RDMA paths. GPU Services would need to integrate with `ibverbs` or `UCX` for RDMA registration of GPU memory.

4. **CUDA events for fine-grained sync**: The current stream API offers stream-level synchronization. Adding CUDA event support would enable finer-grained dependency tracking between individual DMA operations.

5. **Error recovery and health monitoring**: Currently errors are propagated as strings. A future version could implement retry logic for transient CUDA errors and expose GPU health metrics (ECC errors, thermal throttling).

6. **GDS (GPU Direct Storage)**: NVIDIA's `cuFile` API provides a kernel-bypass path for filesystem I/O directly to GPU memory. This could complement or replace the current GDRCopy approach for certain workloads.

7. **Handle tracking and leak detection**: The spec notes that shutdown "claims to close open handles, but handle tracking depends on runtime HashSet correctness." A dedicated leak detector test or debug-mode reference counting would strengthen this guarantee.
