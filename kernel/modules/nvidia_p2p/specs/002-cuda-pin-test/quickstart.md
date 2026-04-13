# Quickstart: CUDA GPU Memory Allocation Test

**Feature**: 002-cuda-pin-test
**Date**: 2026-04-13

## Prerequisites

- RHEL 9 or RHEL 10 (kernel 5.14+)
- NVIDIA GPU with proprietary driver loaded (`nvidia-smi` works)
- CUDA runtime library installed (`libcudart.so` in library path)
  - Note: Full CUDA SDK is NOT required — only the runtime library
- `nvidia_p2p_pin` kernel module loaded (from feature 001)
- Rust toolchain (stable)
- Root access or `CAP_SYS_RAWIO` capability

## Build & Run

```bash
cd rust/

# Build (no CUDA SDK needed — dlopen at runtime)
cargo build --tests

# Run CUDA pin tests (requires root for kernel module access)
sudo cargo test --test cuda_pin_test -- --nocapture

# Run all tests including CUDA tests
sudo cargo test -- --nocapture
```

## Expected Output (with prerequisites met)

```text
running 5 tests
test test_cuda_alignment ... ok
test test_cuda_pin_1mb ... ok
test test_cuda_pin_64kb_minimum ... ok
test test_cuda_pin_unpin_lifecycle ... ok
test test_cuda_multi_size_alignment ... ok

test result: ok. 5 passed; 0 failed; 0 ignored
```

## Expected Output (without CUDA)

```text
running 5 tests
test test_cuda_alignment ... ok (CUDA runtime not available, skipping)
test test_cuda_pin_1mb ... ok (CUDA runtime not available, skipping)
...

test result: ok. 5 passed; 0 failed; 0 ignored
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| "CUDA runtime not available" | `libcudart.so` not in library path | Install CUDA runtime or set `LD_LIBRARY_PATH` |
| "No CUDA GPU available" | No GPU or driver not loaded | `sudo modprobe nvidia` |
| "nvidia_p2p_pin module not loaded" | Kernel module not loaded | `sudo insmod kernel/nvidia_p2p_pin.ko` |
| Permission denied | Missing CAP_SYS_RAWIO | Run with `sudo` |
| Test panics during cleanup | Unpin before cudaFree ordering violated | Check PinnedMemory Drop ordering |
