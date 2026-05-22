# Implementation Plan: GPU CUDA Services

**Branch**: `001-gpu-cuda-services` | **Date**: 2026-04-29 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/001-gpu-cuda-services/spec.md`

## Summary

Implement a GPU services component that wraps CUDA C libraries via FFI
to provide GPU hardware discovery, IPC handle deserialization from
Python base64 payloads, GPU memory pinning/verification, and DMA buffer
creation compatible with the existing `DmaBuffer` type. All
functionality is exposed through an expanded `IGpuServices` interface
and gated behind `--features gpu`. A Python test client and Rust test
server demonstrate the end-to-end Unix domain socket handoff.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**: CUDA runtime API (libcudart via FFI),
  component-framework, interfaces crate, base64 crate, serde (for
  IPC message framing)
**Storage**: N/A (operates on GPU device memory)
**Testing**: `cargo test -p gpu-services --features gpu`, Criterion
  benchmarks via `cargo bench -p gpu-services --features gpu`
**Target Platform**: Linux (RHEL/Fedora) with NVIDIA GPU (compute
  capability 7.0+)
**Project Type**: Component library (Certus component-framework)
**Performance Goals**: Initialize <5s, IPC deserialize <1ms, pin
  verify <10ms, DMA buffer create <50ms
**Constraints**: Feature-gated (`gpu`), interface-only exposure, no
  public functions outside traits, Linux-only
**Scale/Scope**: Single component crate + test apps, 6 interface
  methods + supporting types

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1
design.*

| Principle | Status | Evidence |
|-----------|--------|----------|
| I. Interface-Only Exposure | PASS | All methods on `IGpuServices` trait in `components/interfaces`; no pub fns outside trait |
| II. Component-Framework Conformance | PASS | Uses `define_component!`, receptacles for ILogger, lifecycle via initialize/shutdown |
| III. Code Correctness Assurance | PASS | clippy -D warnings, fmt, doc warnings, SAFETY comments on all unsafe FFI |
| IV. Comprehensive Unit Testing | PASS | Tests for each interface method (success + error), with/without receptacles |
| V. Rust Documentation Tests | PASS | Doc examples on all public trait methods in interface definition |
| VI. Criterion Performance Benchmarks | PASS | Benchmarks for initialize, deserialize, pin, verify, DMA buffer create |
| VII. Maintainability & Engineering Practice | PASS | Minimal API surface, justified deps (CUDA FFI necessary), no premature abstractions |

No violations. Gate passes.

## Project Structure

### Documentation (this feature)

```text
specs/001-gpu-cuda-services/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
components/gpu-services/v0/
├── Cargo.toml                    # Feature gate: [features] gpu = [...]
├── src/
│   ├── lib.rs                    # Component definition, IGpuServices impl
│   ├── cuda_ffi.rs               # Raw CUDA FFI bindings (cfg(feature="gpu"))
│   ├── device.rs                 # GpuDevice discovery logic
│   ├── ipc.rs                    # IPC handle deserialization from base64
│   ├── memory.rs                 # Pin/unpin and contiguity verification
│   └── dma.rs                    # DMA buffer creation from IPC handle
├── benches/
│   └── gpu_services_benchmark.rs # Criterion benchmarks (cfg gpu)
└── tests/
    └── integration.rs            # Integration tests (cfg gpu)

components/interfaces/src/
└── igpu_services.rs              # Expanded IGpuServices trait definition

apps/gpu-handle-test-client/      # Python Unix domain socket client
├── client.py
└── requirements.txt

apps/gpu-handle-test-server/      # Rust server using gpu-services component
├── Cargo.toml
└── src/
    └── main.rs
```

**Structure Decision**: Single-component Rust crate following the
existing v0 skeleton layout, with internal modules for logical
separation. Test apps live in the workspace `apps/` directory per
project convention. The component exposes nothing except `IGpuServices`
and the component struct needed for `query_interface!`.

## Complexity Tracking

> No constitution violations to justify.
