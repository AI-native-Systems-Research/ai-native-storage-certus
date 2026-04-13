# nvidia_p2p Development Guidelines

Auto-generated from all feature plans. Last updated: 2026-04-13

## Active Technologies
- Rust stable (integration test + helper module) + `nvidia-p2p-pin` (path dep from feature 001), `libloading` or raw `dlopen` for CUDA runtime loading (002-cuda-pin-test)

- C (kernel module, kernel 5.14+); Rust stable (user-space library) + NVIDIA driver (`nv-p2p.h`), Linux kernel headers, `nix` crate (Rust ioctl) (001-nvidia-p2p-gpu-pin)

## Project Structure

```text
src/
tests/
```

## Commands

cargo test && cargo clippy

## Code Style

C (kernel module, kernel 5.14+); Rust stable (user-space library): Follow standard conventions

## Recent Changes
- 002-cuda-pin-test: Added Rust stable (integration test + helper module) + `nvidia-p2p-pin` (path dep from feature 001), `libloading` or raw `dlopen` for CUDA runtime loading

- 001-nvidia-p2p-gpu-pin: Added C (kernel module, kernel 5.14+); Rust stable (user-space library) + NVIDIA driver (`nv-p2p.h`), Linux kernel headers, `nix` crate (Rust ioctl)

<!-- MANUAL ADDITIONS START -->
<!-- MANUAL ADDITIONS END -->
