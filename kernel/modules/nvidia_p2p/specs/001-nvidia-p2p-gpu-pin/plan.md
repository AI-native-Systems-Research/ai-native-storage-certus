# Implementation Plan: NVIDIA P2P GPU Memory Pinning

**Branch**: `001-nvidia-p2p-gpu-pin` | **Date**: 2026-04-13 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-nvidia-p2p-gpu-pin/spec.md`

## Summary

Implement a Linux kernel module (C) that wraps the NVIDIA persistent P2P API
(`nvidia_p2p_get_pages_persistent` / `nvidia_p2p_put_pages_persistent`) behind
a character device (`/dev/nvidia_p2p`) with an ioctl interface, plus a Rust
user-space library that provides a safe `pin_gpu_memory` / `unpin_gpu_memory`
API. The purpose is to enable GPUDirect Storage workflows where an NVMe SSD
can DMA data directly to/from pinned GPU memory.

## Technical Context

**Language/Version**: C (kernel module, kernel 5.14+); Rust stable (user-space library)
**Primary Dependencies**: NVIDIA driver (`nv-p2p.h`), Linux kernel headers, `nix` crate (Rust ioctl)
**Storage**: N/A (no persistent storage; in-kernel tracking via linked list)
**Testing**: Kernel: custom test harness requiring NVIDIA GPU; Rust: `cargo test` + Criterion benchmarks
**Target Platform**: Linux x86_64, RHEL 9 (kernel 5.14), RHEL 10
**Project Type**: Kernel module + user-space library
**Performance Goals**: Pin latency < 10 ms for 1 MB region; Unpin latency < 5 ms
**Constraints**: CAP_SYS_RAWIO required; 64KB alignment for VA and length
**Scale/Scope**: Single GPU, no MIG support; no artificial limit on concurrent pinned regions

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Constitution principles (from project requirements, constitution not yet formally ratified):

| Principle | Status | Notes |
|-----------|--------|-------|
| Kernel code in C | PASS | Kernel module implemented in C |
| Kernel 5.14+ support | PASS | Using standard kbuild, no APIs newer than 5.14 |
| RHEL 9 and RHEL 10 | PASS | Target platforms for both kernel module and Rust library |
| User-level code in Rust | PASS | User-space library is Rust |
| Minimize Rust unsafe | PASS | unsafe limited to ioctl syscall layer only |
| Criterion benchmarks for perf-sensitive code | PASS | Pin/unpin round-trip benchmarked via Criterion |
| Code correctness assurance | PASS | Typed errors, RAII via Drop, kernel mutex for concurrency |
| Extensive testing | PASS | Unit tests, integration tests, Criterion benchmarks planned |
| Code quality & maintainability | PASS | Clean separation: kernel module / shared ioctl header / Rust library |

**GATE RESULT: PASS** — No violations.

## Project Structure

### Documentation (this feature)

```text
specs/001-nvidia-p2p-gpu-pin/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output (ioctl interface)
│   └── ioctl-interface.md
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
kernel/
├── Makefile             # Kbuild Makefile with nv-p2p.h auto-discovery
├── nvidia_p2p_pin.c     # Kernel module: char device, ioctl handlers, region tracking
├── nvidia_p2p_pin.h     # Shared ioctl definitions (kernel + userspace)
└── Kbuild               # Kbuild integration file

rust/
├── Cargo.toml           # Library crate: nvidia-p2p-pin
├── src/
│   ├── lib.rs           # Public API: pin_gpu_memory, PinnedMemory, Error
│   ├── ioctl.rs         # ioctl definitions and raw syscall wrappers (unsafe boundary)
│   ├── device.rs        # NvP2pDevice: /dev/nvidia_p2p file descriptor management
│   └── error.rs         # Error enum with typed variants
├── tests/
│   └── integration.rs   # Integration tests (require loaded kernel module + GPU)
└── benches/
    └── pin_unpin.rs     # Criterion benchmarks for pin/unpin latency
```

**Structure Decision**: Two-directory layout separating kernel module (C, kbuild)
from user-space library (Rust, cargo). A shared header (`nvidia_p2p_pin.h`)
defines the ioctl command numbers and structures used by both sides. The Rust
library duplicates these definitions in `ioctl.rs` to avoid a C build dependency.

## Complexity Tracking

> No Constitution Check violations — table not required.
