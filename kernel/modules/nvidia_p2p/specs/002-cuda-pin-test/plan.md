# Implementation Plan: CUDA GPU Memory Allocation Test for P2P Pinning

**Branch**: `002-cuda-pin-test` | **Date**: 2026-04-13 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/002-cuda-pin-test/spec.md`

## Summary

Add a Rust integration test that allocates GPU memory via the CUDA runtime API
(`cudaMalloc` loaded at runtime via `dlopen`), pins it using the
`nvidia-p2p-pin` library, validates the returned physical addresses, and cleans
up correctly. The test builds on systems without CUDA and skips gracefully when
prerequisites (GPU, CUDA runtime, kernel module) are absent.

## Technical Context

**Language/Version**: Rust stable (integration test + helper module)
**Primary Dependencies**: `nvidia-p2p-pin` (path dep from feature 001), `libloading` or raw `dlopen` for CUDA runtime loading
**Storage**: N/A
**Testing**: `cargo test` (integration tests in `rust/tests/`)
**Target Platform**: Linux x86_64, RHEL 9, RHEL 10
**Project Type**: Integration test suite (extends feature 001 Rust crate)
**Performance Goals**: Test completes in < 30 seconds
**Constraints**: Requires root/CAP_SYS_RAWIO at runtime; no CUDA SDK at build time
**Scale/Scope**: Single test file with ~5 test functions

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| Kernel code in C | N/A | No kernel code in this feature (test only) |
| Kernel 5.14+ support | N/A | Test is user-space only |
| RHEL 9 and RHEL 10 | PASS | Rust + dlopen works on both |
| User-level code in Rust | PASS | All test code is Rust |
| Minimize Rust unsafe | PASS | unsafe limited to dlopen FFI calls for cudaMalloc/cudaFree |
| Criterion benchmarks for perf-sensitive code | N/A | Test code is not performance-sensitive |
| Code correctness assurance | PASS | RAII wrappers (CudaMemory), typed errors, assertion coverage |
| Extensive testing | PASS | This feature IS the test |
| Code quality & maintainability | PASS | Small helper module + clean test functions |

**GATE RESULT: PASS** — No violations.

## Project Structure

### Documentation (this feature)

```text
specs/002-cuda-pin-test/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
└── quickstart.md        # Phase 1 output
```

### Source Code (repository root)

```text
rust/
├── Cargo.toml           # Add libloading dependency (or use raw libc::dlopen)
├── src/
│   └── ...              # Existing library code from feature 001
├── tests/
│   ├── integration.rs   # Existing integration tests from feature 001
│   ├── cuda_pin_test.rs # NEW: End-to-end CUDA pin/unpin test
│   └── cuda_helpers.rs  # NEW: CUDA dlopen helper (CudaRuntime, CudaMemory)
└── benches/
    └── ...              # Existing benchmarks from feature 001
```

**Structure Decision**: Tests are added to the existing `rust/` crate from
feature 001. A new `cuda_helpers.rs` module provides the `dlopen`-based CUDA
FFI wrapper and `CudaMemory` RAII type. The test file `cuda_pin_test.rs`
contains the actual test functions. This avoids creating a separate crate for
what is purely a test concern.

## Complexity Tracking

> No Constitution Check violations — table not required.
