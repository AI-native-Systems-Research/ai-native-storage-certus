# GPU Services v0

A Certus component that wraps the CUDA runtime API to provide safe GPU memory access for DMA operations. Receives CUDA IPC memory handles from remote processes (e.g., a Python inference framework), verifies and pins the memory, and produces DMA-ready buffers for the storage subsystem.

## Summary

- **CUDA FFI**: Hand-written minimal bindings to `libcudart` for IPC handles, memcpy, streams, and host memory registration
- **IPC handles**: Deserializes base64-encoded CUDA IPC handles (64-byte handle + 8-byte LE size) from remote processes
- **DMA operations**: Synchronous and async `cudaMemcpy` between GPU device memory and host DMA buffers (both directions)
- **register_host_memory**: Page-locks existing host memory via `cudaHostRegister` and registers with SPDK for zero-copy NVMe/GPU transfers
- **CUDA streams**: Create/destroy/synchronize streams for overlapping GPU DMA with NVMe I/O
- **Pinned DMA buffers**: Allocates `cudaHostAlloc` + `spdk_mem_register` buffers for pipeline ring use
- **P2P path**: Optional GDRCopy-based GPU-direct-to-NVMe via VFIO DMA mapping (behind `p2p` feature)

## Structure

```
src/
  lib.rs            Component definition (GpuServicesComponentV0), IGpuServices impl
  cuda_ffi.rs       Raw CUDA runtime API FFI bindings (minimal, hand-written)
  device.rs         GPU device enumeration (compute capability 7.0+ filter)
  ipc.rs            IPC handle deserialization and open/close
  memory.rs         Memory verification, pinning, and registration
  dma.rs            DMA buffer creation, SPDK memory registration, P2P VFIO mapping
  gdrcopy_ffi.rs    GDRCopy FFI bindings (behind `p2p` feature)
  bin/              gpu-p2p-server binary (requires `p2p` feature)

benches/
  gpu_services_benchmark.rs    GPU services operation benchmarks
  dma_transfer_benchmark.rs    cudaMemcpy throughput benchmarks (4 KiB - 64 MiB)
```

### Component Wiring

```
GpuServicesComponentV0 --> [IGpuServices provider]
                       <-- [ILogger receptacle] (optional)
```

### Feature Gates

| Feature | Effect |
|---------|--------|
| `gpu` | Enables CUDA FFI, all GPU operations functional |
| `spdk` | Enables `dma_copy_to_host`, `dma_copy_to_device`, `register_host_memory`, `allocate_pinned_dma_buffer` |
| `p2p` | Enables GPU-direct P2P via GDRCopy + VFIO DMA mapping |

Without `gpu`, the crate compiles and links without `libcudart`; every operation returns a descriptive error.

## Build and Test

```bash
# Build without GPU (compiles but operations return errors at runtime)
cargo build -p gpu-services

# Build with GPU support (requires CUDA toolkit)
cargo build -p gpu-services --features gpu

# Build with SPDK DMA support
cargo build -p gpu-services --features gpu,spdk

# Tests
cargo test -p gpu-services
cargo test -p gpu-services --features gpu   # requires CUDA runtime

# Lint and docs
cargo clippy -p gpu-services -- -D warnings
cargo doc -p gpu-services --no-deps

# Benchmarks (requires GPU hardware)
cargo bench -p gpu-services --features gpu
cargo bench -p gpu-services --features gpu --bench dma_transfer_benchmark
```
