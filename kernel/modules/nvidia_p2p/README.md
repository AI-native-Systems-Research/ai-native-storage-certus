# nvidia_p2p - NVIDIA P2P GPU Memory Pinning

Kernel module and Rust library for pinning GPU memory via the NVIDIA persistent
P2P API. Enables GPUDirect Storage workflows where an NVMe SSD can DMA data
directly to/from pinned GPU memory.

## Prerequisites

- RHEL 9 or RHEL 10 (kernel 5.14+)
- NVIDIA proprietary driver with development headers installed
  - `nvidia-smi` works
  - `/usr/src/nvidia-*/nvidia/nv-p2p.h` exists
- Kernel development headers (`kernel-devel` matching running kernel)
- Rust toolchain (stable)
- CUDA runtime library (`libcudart.so`) for tests (full CUDA SDK not required)
- Root access for loading/unloading the kernel module

## Build Kernel Module

```bash
cd kernel/
make
```

The Makefile auto-discovers `nv-p2p.h` and the NVIDIA `Module.symvers` from
DKMS. If the build fails with "NVIDIA driver source not found", install
`nvidia-driver-devel` or verify `/usr/src/nvidia-*/nvidia/nv-p2p.h` exists.

## Load Kernel Module

```bash
sudo insmod kernel/nvidia_p2p_pin.ko

# Verify
lsmod | grep nvidia_p2p_pin
ls -la /dev/nvidia_p2p
dmesg | tail -5   # should show "nvidia_p2p_pin: loaded"
```

## Build Rust Library and Tests

```bash
cd rust/
cargo build
cargo build --tests --benches
```

## Run CUDA Pin Tests

These tests allocate GPU memory via `cudaMalloc` (loaded at runtime via dlopen),
pin it through the kernel module, validate physical addresses, and clean up.
Tests skip gracefully when prerequisites are missing.

The device node `/dev/nvidia_p2p` is created with mode `0666`, so tests can run
as a regular user once the module is loaded. No `sudo` is needed for running
tests or benchmarks.

If `libcudart.so` is not on the default library path, set `LD_LIBRARY_PATH`:

```bash
export LD_LIBRARY_PATH=/path/to/cuda/runtime/lib:$LD_LIBRARY_PATH
```

```bash
cd rust/

# Run all CUDA pin tests
cargo test --test cuda_pin_test -- --nocapture

# Run all integration tests (pin/unpin/query)
cargo test --test integration -- --nocapture

# Run everything
cargo test -- --nocapture
```

### Expected output (with prerequisites)

```
running 6 tests
test test_cuda_alignment ... ok
test test_cuda_pin_1mb ... ok
test test_cuda_pin_64kb_minimum ... ok
test test_cuda_pin_unpin_lifecycle ... ok
test test_cuda_multi_size_alignment ... ok
test test_cuda_skip_no_prerequisites ... ok

test result: ok. 6 passed; 0 failed; 0 ignored
```

### Expected output (without CUDA)

```
running 6 tests
test test_cuda_alignment ... ok (CUDA runtime not available, skipping)
test test_cuda_pin_1mb ... ok (CUDA runtime not available, skipping)
...

test result: ok. 6 passed; 0 failed; 0 ignored
```

## Run Pin/Unpin Benchmarks

Criterion benchmarks measure pin and unpin latency across 64KB, 1MB, and 16MB
regions.

```bash
cargo bench --bench pin_unpin
```

Results are written to `target/criterion/` with HTML reports.

## Unload Kernel Module

```bash
sudo rmmod nvidia_p2p_pin
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `DeviceNotFound` error | Module not loaded | `sudo insmod kernel/nvidia_p2p_pin.ko` |
| `PermissionDenied` error | `/dev/nvidia_p2p` not accessible | Check device permissions (`ls -la /dev/nvidia_p2p`) |
| Module fails to load | NVIDIA driver not loaded | `sudo modprobe nvidia` |
| Build fails: "nv-p2p.h not found" | NVIDIA driver source missing | Install `nvidia-driver-devel` |
| "CUDA runtime not available" | `libcudart.so` not in library path | Install CUDA runtime or set `LD_LIBRARY_PATH` |
| "No CUDA GPU available" | No GPU or driver not loaded | `sudo modprobe nvidia` |
