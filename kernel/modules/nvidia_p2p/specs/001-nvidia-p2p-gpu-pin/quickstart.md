# Quickstart: NVIDIA P2P GPU Memory Pinning

**Feature**: 001-nvidia-p2p-gpu-pin
**Date**: 2026-04-13

## Prerequisites

- RHEL 9 or RHEL 10 (kernel 5.14+)
- NVIDIA proprietary driver installed (580.x+ recommended)
  - Verify: `nvidia-smi` shows GPU information
  - Verify: `/usr/src/nvidia-*/nvidia/nv-p2p.h` exists
- CUDA toolkit installed (for test programs that allocate GPU memory)
- Kernel development headers: `kernel-devel` package matching running kernel
- Rust toolchain (stable): `rustup`, `cargo`
- Root access or `CAP_SYS_RAWIO` capability

## Build & Install

### Kernel Module

```bash
cd kernel/

# Build (auto-discovers nv-p2p.h location)
make

# Load the module
sudo insmod nvidia_p2p_pin.ko

# Verify
lsmod | grep nvidia_p2p_pin
ls -la /dev/nvidia_p2p
dmesg | tail -5   # should show "nvidia_p2p_pin: loaded"
```

### Rust Library

```bash
cd rust/

# Build
cargo build

# Run tests (requires loaded kernel module + NVIDIA GPU)
sudo cargo test

# Run benchmarks
sudo cargo bench
```

## Usage Example (Rust)

```rust
use nvidia_p2p_pin::{NvP2pDevice, PinnedMemory, Error};

fn main() -> Result<(), Error> {
    // Open the device
    let device = NvP2pDevice::open()?;

    // Assume gpu_va and gpu_len come from CUDA allocation
    // (must be 64KB-aligned, length must be multiple of 64KB)
    let gpu_va: u64 = /* from cudaMalloc */ ;
    let gpu_len: u64 = 1024 * 1024;  // 1 MB

    // Pin GPU memory
    let pinned = device.pin_gpu_memory(gpu_va, gpu_len)?;

    println!("Pinned {} pages (page size: {:?})",
        pinned.page_count(), pinned.page_size());

    // Access physical addresses (for DMA programming)
    for (i, phys_addr) in pinned.physical_addresses().iter().enumerate() {
        println!("  page {}: phys_addr = 0x{:x}", i, phys_addr);
    }

    // Use physical addresses to program NVMe SSD DMA...

    // Explicit unpin (or let Drop handle it)
    pinned.unpin()?;

    Ok(())
}
```

## Unload

```bash
# Unload kernel module (releases all remaining pinned regions)
sudo rmmod nvidia_p2p_pin
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `DeviceNotFound` error | Module not loaded | `sudo insmod nvidia_p2p_pin.ko` |
| `PermissionDenied` error | Missing CAP_SYS_RAWIO | Run as root or add capability |
| Module fails to load | NVIDIA driver not loaded | `sudo modprobe nvidia` |
| Build fails: "nv-p2p.h not found" | NVIDIA driver source missing | Install `nvidia-driver-devel` package |
| `InvalidAlignment` error | GPU VA not 64KB-aligned | Ensure CUDA allocation is aligned |
| `OutOfMemory` error | GPU memory exhaustion | Free unused GPU allocations |
