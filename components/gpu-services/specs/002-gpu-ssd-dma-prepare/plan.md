# Implementation Plan: GPU-to-SSD DMA Buffer Preparation

**Branch**: `002-gpu-ssd-dma-prepare` | **Date**: 2026-05-06 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/002-gpu-ssd-dma-prepare/spec.md`

## Summary

Add `prepare_memory_for_spdk` to the `IGpuServices` interface — a single-call convenience method that accepts a base64-encoded CUDA IPC handle and an optional GPU device index, opens the handle with lazy peer access, detects pin state via `cudaPointerGetAttributes`, conditionally pins memory (logging the decision), and returns an SPDK `DmaBuffer` (via `DmaBuffer::from_raw`) with a pin-state-aware custom free function.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75  
**Primary Dependencies**: CUDA runtime API (libcudart via FFI), SPDK (via `interfaces::DmaBuffer`), base64 crate  
**Storage**: N/A (operates on GPU device memory)  
**Testing**: `cargo test -p gpu-services` (unit tests with feature gates `gpu` and `spdk`)  
**Target Platform**: Linux only (RHEL/Fedora)  
**Project Type**: Library (Rust component crate)  
**Performance Goals**: Preparation latency dominated by CUDA IPC open (~100µs); no additional overhead beyond the CUDA calls themselves  
**Constraints**: Requires both `gpu` and `spdk` features enabled simultaneously  
**Scale/Scope**: Single function addition to existing interface; ~150 LOC implementation

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Interface-Only Exposure | PASS | New method added to `IGpuServices` trait in `components/interfaces` |
| II. Component-Framework Conformance | PASS | Uses existing receptacle pattern (logger); no new global state |
| III. Code Correctness Assurance | PASS | All unsafe blocks will have `// SAFETY:` comments; clippy clean |
| IV. Comprehensive Unit Testing | PASS | Tests for both pin paths, error paths, no-logger path |
| V. Rust Documentation Tests | PASS | Doc examples with `no_run` (requires GPU hardware) |
| VI. Criterion Performance Benchmarks | PASS | Will add benchmark for prepare path |
| VII. Maintainability & Engineering Practice | PASS | Composes existing `ipc::` and `memory::` modules; minimal new code |

## Project Structure

### Documentation (this feature)

```text
specs/002-gpu-ssd-dma-prepare/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── igpu_services_prepare.md
└── tasks.md             # Phase 2 output (from /speckit.tasks)
```

### Source Code (repository root)

```text
components/interfaces/src/
└── igpu_services.rs          # Add prepare_memory_for_spdk to define_interface! block

components/gpu-services/v0/src/
├── lib.rs                    # Add impl for prepare_memory_for_spdk
├── dma.rs                    # Add SPDK DmaBuffer creation with pin-aware free fns
├── cuda_ffi.rs               # No changes needed (all FFI already present)
├── ipc.rs                    # No changes needed (reuse existing decode/open)
└── memory.rs                 # Add is_memory_pinned() query function

components/gpu-services/v0/benches/
└── gpu_services_benchmark.rs # Add prepare_memory_for_spdk benchmark
```

**Structure Decision**: Extend existing modules. The new function reuses `ipc::decode_ipc_payload`, `ipc::open_ipc_handle`, and `memory::check_memory_attributes`. New logic (pin-state detection, SPDK DmaBuffer wrapping, pin-aware free functions) goes into `dma.rs` and `memory.rs`.

## Complexity Tracking

No constitution violations — table not needed.
