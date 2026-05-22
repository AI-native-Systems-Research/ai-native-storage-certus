# GPU Services

## Summary

GPU Services is a Certus component that wraps the CUDA runtime API to provide safe GPU memory access for DMA operations. It receives CUDA IPC memory handles from remote processes (e.g., a Python inference framework), verifies and pins the memory, and produces DMA-ready buffers for the storage subsystem.

The component implements the `IGpuServices` interface with operations for CUDA initialization, GPU hardware discovery, IPC handle deserialization, memory verification and pinning, synchronous and asynchronous DMA transfers (both Host-to-Device and Device-to-Host), CUDA stream management, and pinned host memory allocation registered with SPDK. An optional P2P path uses GDRCopy for GPU-direct-to-NVMe transfers via VFIO DMA mapping.

## Architecture

### Source Layout

```
src/
  lib.rs            Component definition (GpuServicesComponent), IGpuServices impl
  cuda_ffi.rs       Raw CUDA runtime API FFI bindings (minimal, hand-written)
  device.rs         GPU device enumeration (compute capability 7.0+ filter)
  ipc.rs            IPC handle deserialization and open/close
  memory.rs         Memory verification (cudaPointerGetAttributes)
  dma.rs            DMA buffer creation, SPDK memory registration, P2P VFIO mapping
  gdrcopy_ffi.rs    GDRCopy FFI bindings (behind `p2p` feature)
  bin/p2p_server.rs GPU P2P DMA server binary (requires `p2p` feature)
```

### CUDA FFI

The `cuda_ffi` module provides hand-written minimal bindings to `libcudart` covering IPC handles (`cudaIpcOpenMemHandle`, `cudaIpcCloseMemHandle`), memcpy (sync and async), streams, device management, pointer attributes, and host memory registration/allocation. Bindings are hand-written rather than bindgen-generated for auditability and to minimize the required CUDA surface area.

### GDRCopy P2P

When the `p2p` feature is enabled, `gdrcopy_ffi` provides bindings to the GDRCopy library (`libgdrapi`) for pinning GPU BAR1 regions. The P2P path maps GPU VRAM into the VFIO address space (via `rte_extmem_register` and `rte_vfio_container_dma_map`) so that NVMe controllers can DMA directly to/from GPU memory without a host bounce buffer.

The `gpu-p2p-server` binary demonstrates three transfer modes:
- **bounce**: NVMe to host DMA buffer to cudaMemcpy H2D to client GPU buffer
- **p2p**: NVMe to pre-pinned GPU staging (GDRCopy, setup amortized) to D2D to client
- **p2p-cold**: NVMe to per-request GDRCopy pin/unpin to D2D to client (baseline)

### Feature Gates

| Feature | Effect |
|---------|--------|
| `gpu`   | Enables CUDA FFI and all GPU operations. Links `libcudart`. |
| `spdk`  | Enables `dma_copy_to_host`, `dma_copy_to_device`, `prepare_memory_for_spdk`, `register_host_memory`, `allocate_pinned_dma_buffer`. Links SPDK. |
| `p2p`   | Implies `gpu` + `spdk`. Enables GDRCopy GPU-direct P2P via VFIO DMA mapping. Links `libgdrapi`. |

Without the `gpu` feature, the crate compiles and links without `libcudart`; every operation returns a descriptive error at runtime.

### Component Wiring

```
GpuServicesComponent --> [IGpuServices provider]
                     <-- [ILogger receptacle] (optional)
```

## Build

The crate name is `gpu-services`.

```bash
# Build without GPU (compiles but operations return errors at runtime)
cargo build -p gpu-services

# Build with GPU support (requires CUDA toolkit installed)
cargo build -p gpu-services --features gpu

# Build with SPDK DMA support (requires SPDK pre-built at deps/spdk-build/)
cargo build -p gpu-services --features gpu,spdk

# Build with P2P support (requires CUDA + SPDK + GDRCopy)
cargo build -p gpu-services --features p2p
```

**Dependencies:**

- **CUDA toolkit** (`libcudart`): Searched in `/usr/local/cuda/lib64`, `/usr/lib64`, the `CUDA_RUNTIME_LIB_PATH` env var, or the pip `nvidia-cuda-runtime-cu12` package location.
- **GDRCopy** (`libgdrapi`, only with `spdk` or `p2p`): Searched in `GDRCOPY_LIB_PATH` env var, a project-local build at `kernel/modules/gdrcopy/src/`, `/usr/local/gdrcopy/lib`, or `/usr/lib64`.
- **SPDK** (only with `spdk` or `p2p`): Requires SPDK pre-built at `deps/spdk-build/`.

## Test

```bash
# Run tests without GPU feature (tests interface availability and graceful errors)
cargo test -p gpu-services

# Run tests with GPU support (requires CUDA runtime and optionally GPU hardware)
cargo test -p gpu-services --features gpu

# Run tests with full SPDK integration
cargo test -p gpu-services --features gpu,spdk

# Lint and docs
cargo clippy -p gpu-services -- -D warnings
cargo doc -p gpu-services --no-deps
```

Tests are structured in tiers: without the `gpu` feature they verify interface discovery and graceful error returns; with `gpu` they exercise CUDA initialization, device enumeration, IPC handle decoding, and a CPU-to-GPU-to-CPU round-trip; with `gpu,spdk` they add `prepare_memory_for_spdk` validation.

## Benchmarks

Two Criterion benchmark suites are available, both requiring `--features gpu` and NVIDIA GPU hardware.

```bash
# Run all gpu-services benchmarks
cargo bench -p gpu-services --features gpu

# GPU services operation benchmarks (initialize, get_devices, IPC deserialization)
cargo bench -p gpu-services --features gpu --bench gpu_services_benchmark

# DMA transfer throughput (4 KiB to 64 MiB, H2D and D2H, pageable vs pinned)
cargo bench -p gpu-services --features gpu --bench dma_transfer_benchmark
```

The DMA transfer benchmark measures `cudaMemcpy` throughput across multiple transfer sizes (4 KiB, 64 KiB, 256 KiB, 1 MiB, 4 MiB, 16 MiB, 64 MiB), both transfer directions, all available GPU devices, and compares pageable versus pinned host memory performance.
