# Quickstart: GPU CUDA Services

## Prerequisites

- Linux system (RHEL/Fedora tested)
- NVIDIA GPU with compute capability 7.0+ (Volta or newer)
- NVIDIA CUDA drivers and runtime installed
- Rust stable toolchain (MSRV 1.75)
- Python 3.8+ with `cupy` installed (for test client)
- SPDK environment configured (for DMA transfer testing)

## Build

```bash
# Build with GPU support
cargo build -p gpu-services --features gpu

# Build the test server
cargo build -p gpu-handle-test-server --features gpu
```

## Run Tests

```bash
# Unit tests + doc tests
cargo test -p gpu-services --features gpu

# Benchmarks
cargo bench -p gpu-services --features gpu
```

## Basic Usage (Rust)

```rust
use gpu_services::GpuServicesComponentV0;
use interfaces::{IGpuServices, IpcHandle};
use component_core::query_interface;

// Create and initialize component
let component = GpuServicesComponentV0::new();
let gpu = query_interface!(component, IGpuServices).unwrap();
gpu.initialize().unwrap();

// Discover GPUs
let devices = gpu.get_devices().unwrap();
for dev in &devices {
    println!("{}: {} ({}.{}), {} MB",
        dev.device_index, dev.name,
        dev.compute_major, dev.compute_minor,
        dev.memory_bytes / (1024 * 1024));
}

// Deserialize an IPC handle from base64 (received from Python)
let handle = gpu.deserialize_ipc_handle(&base64_payload).unwrap();

// Verify and pin
gpu.verify_memory(&handle).unwrap();
gpu.pin_memory(&handle).unwrap();

// Create DMA buffer for SPDK operations
let dma_buf = gpu.create_dma_buffer(handle).unwrap();

// Use dma_buf with block-device-spdk-nvme for SSD→GPU transfer...

// Shutdown
gpu.shutdown().unwrap();
```

## End-to-End Demo

Terminal 1 — Start the Rust server:
```bash
cargo run -p gpu-handle-test-server --features gpu
# Listens on /tmp/gpu-services-ipc.sock
```

Terminal 2 — Run the Python client:
```bash
cd apps/gpu-handle-test-client
pip install -r requirements.txt
python client.py
# Allocates GPU memory, sends IPC handle, waits for ACK
```

## Verify

The server will log:
```
GpuServices initialized
Found 1 GPU(s): NVIDIA A100 (8.0), 40960 MB
IPC handle received: 4096 bytes
Memory verified: contiguous, device type
DMA buffer created successfully
DMA write complete: 4096 bytes transferred
ACK sent to client
```
