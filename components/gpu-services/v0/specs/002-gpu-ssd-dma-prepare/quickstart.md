# Quickstart: prepare_memory_for_spdk

## Usage

```rust
use gpu_services::GpuServicesComponentV0;
use interfaces::IGpuServices;
use component_core::query_interface;

let component = GpuServicesComponentV0::new();
let gpu = query_interface!(component, IGpuServices).unwrap();
gpu.initialize().unwrap();

// base64_payload received from PyTorch via gRPC
let base64_payload = "..."; // 72-byte payload, base64-encoded

// Prepare GPU memory for SPDK DMA (uses current CUDA device)
let dma_buf = gpu.prepare_memory_for_spdk(base64_payload, None).unwrap();

// Or specify a target GPU device index
let dma_buf = gpu.prepare_memory_for_spdk(base64_payload, Some(0)).unwrap();

// dma_buf is now directly usable with SPDK NVMe read/write operations
assert!(dma_buf.len() > 0);

// On drop, dma_buf automatically:
// - Unpins memory (if it was pinned by this call)
// - Closes the CUDA IPC handle
```

## Build

```bash
# Build with both GPU and SPDK support
cargo build -p gpu-services --features "gpu,spdk"

# Run tests
cargo test -p gpu-services --features "gpu,spdk"

# Run benchmarks
cargo bench -p gpu-services --features "gpu,spdk"
```

## Feature Flags

| Flag | Purpose |
|------|---------|
| `gpu` | Enables CUDA FFI bindings and GPU operations |
| `spdk` | Enables SPDK DmaBuffer return type and DMA methods |

Both flags are required for `prepare_memory_for_spdk`.

## Integration with SPDK Block Device

```rust
// After obtaining DmaBuffer from prepare_memory_for_spdk:
// The buffer can be passed directly to IBlockDevice read operations
// for peer-to-peer SSD→GPU DMA.
block_device.read_async(lba, &dma_buf, callback)?;
```
