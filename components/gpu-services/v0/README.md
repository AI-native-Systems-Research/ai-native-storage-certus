# GPU Services Component (v0)

A Certus component that wraps the CUDA runtime API to provide safe GPU memory access for DMA operations. It receives CUDA IPC memory handles from remote processes (e.g., a Python inference framework), verifies and pins the memory, and produces DMA-ready buffers that can be used by the storage subsystem.

## Purpose

In AI-native storage workloads, inference engines (PyTorch, TensorRT) hold model weights and activations in GPU memory. This component bridges that GPU memory into the Certus storage pipeline by:

1. Discovering NVIDIA GPUs with compute capability 7.0+ (Volta and newer)
2. Deserializing CUDA IPC handles exported by another process
3. Verifying the memory is device-allocated and contiguous
4. Pinning the memory for DMA transfer
5. Producing a `GpuDmaBuffer` that owns the IPC handle lifetime

## Interface: `IGpuServices`

Defined in `components/interfaces/src/igpu_services.rs`. All methods return `Result<T, String>`.

| Method | Description |
|--------|-------------|
| `initialize()` | Load CUDA runtime, enumerate GPUs. Idempotent. |
| `shutdown()` | Release all state and close handles. |
| `get_devices()` | Return `Vec<GpuDeviceInfo>` for qualifying GPUs. |
| `deserialize_ipc_handle(base64)` | Decode a 72-byte base64 payload (64B handle + 8B LE size), open the CUDA IPC handle, return `GpuIpcHandle`. |
| `verify_memory(handle)` | Confirm the pointer refers to device memory via `cudaPointerGetAttributes`. |
| `pin_memory(handle)` | Pin the memory for DMA (idempotent, auto-verifies if needed). |
| `unpin_memory(handle)` | Unpin previously pinned memory. |
| `create_dma_buffer(handle)` | Consume a verified+pinned handle, return `GpuDmaBuffer`. Dropping the buffer closes the IPC handle. |

## Key Types

- **`GpuDeviceInfo`** — device index, name, memory size, compute capability, PCI bus ID
- **`GpuIpcHandle`** — opened IPC handle with verification/pinning state
- **`GpuDmaBuffer`** — owns GPU memory pointer; calls `cudaIpcCloseMemHandle` on drop

## Build

```bash
# Without GPU (compiles but operations return errors at runtime)
cargo build -p gpu-services

# With GPU support (requires CUDA toolkit)
cargo build -p gpu-services --features gpu

# Tests
cargo test -p gpu-services
cargo test -p gpu-services --features gpu  # requires CUDA

# Benchmarks
cargo bench -p gpu-services --features gpu
```

## Architecture

```
Python client                    Rust (this component)
─────────────                    ─────────────────────
cupy.cuda.alloc()
  → IPC handle export
  → base64(handle[64] + size[8])
  → Unix socket send ──────────→ deserialize_ipc_handle()
                                   → verify_memory()
                                   → pin_memory()
                                   → create_dma_buffer()
                                   → DMA to/from NVMe via SPDK
```

## DMA Transfer Benchmarks

Criterion benchmarks measure `cudaMemcpy` throughput across transfer sizes (4 KiB – 64 MiB), both directions, all available GPUs, and pageable vs pinned host memory.

```bash
cargo bench -p gpu-services --features gpu --bench dma_transfer_benchmark
```

Example results on Tesla V100-PCIE-16GB:

| Transfer | Memory | 4 KiB | 64 KiB | 1 MiB | 16 MiB | 64 MiB |
|----------|--------|-------|--------|-------|--------|--------|
| H→D | Pageable | 567 MiB/s | 1.68 GiB/s | 2.41 GiB/s | 2.98 GiB/s | 2.94 GiB/s |
| D→H | Pageable | 426 MiB/s | 1.86 GiB/s | 2.22 GiB/s | 2.69 GiB/s | 2.67 GiB/s |
| H→D | Pinned | 480 MiB/s | 4.75 GiB/s | 10.5 GiB/s | 11.4 GiB/s | 11.5 GiB/s |
| D→H | Pinned | 577 MiB/s | 5.47 GiB/s | 11.3 GiB/s | 12.2 GiB/s | 12.2 GiB/s |

Pinned (page-locked) host memory provides ~4x higher throughput than pageable memory for large transfers, saturating the PCIe Gen3 x16 link at ~12 GiB/s.

## Feature Gate

All CUDA FFI calls are behind `#[cfg(feature = "gpu")]`. Without the feature, the crate compiles and links without `libcudart`; every operation returns a descriptive error. This allows the workspace to build on CI machines without GPU hardware.

## Component Model

Uses `define_component!` from the Certus component framework:

- **Provides**: `IGpuServices`
- **Receptacles**: `logger: ILogger` (optional; operations succeed silently without it)

```rust
use gpu_services::GpuServicesComponentV0;
use interfaces::IGpuServices;
use component_core::query_interface;

let component = GpuServicesComponentV0::new();
let gpu = query_interface!(component, IGpuServices).unwrap();
gpu.initialize().unwrap();
let devices = gpu.get_devices().unwrap();
gpu.shutdown().unwrap();
```
