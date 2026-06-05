# Implementation Plan: Block Device Filesys Component

**Branch**: `001-block-device-filesys` | **Date**: 2026-06-04 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/001-block-device-filesys/spec.md`

## Summary

A file-backed block device component that implements the IBlockDevice interface using a regular Linux file as the backing store. Synchronous IO uses pread/pwrite with fdatasync for durability. Asynchronous IO uses io_uring via the `io-uring` crate for kernel-level async operations. The component follows the actor model with a dedicated thread running an io_uring event loop, conforming to the component-framework methodology.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**: component-core, component-macros, component-framework, interfaces, io-uring (tokio-rs/io-uring), crossbeam-channel

**Storage**: File on local Linux filesystem (ext4/XFS), pre-allocated via fallocate

**Testing**: cargo test (unit + doc tests), criterion (benchmarks)

**Target Platform**: Linux x86_64 (kernel >= 5.6 for io_uring)

**Project Type**: Library (Rust component crate)

**Performance Goals**: <1ms sync IO latency for 4KB blocks on SSD, stable Criterion benchmarks (CV < 15%)

**Constraints**: No SPDK dependency, fdatasync after each write, single-namespace model (ns_id=1)

**Scale/Scope**: Single component crate, ~1000-1500 SLOC estimated

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Component Architecture Conformance | ✅ PASS | Uses define_component!, provides IBlockDevice, receptacle for ILogger, interfaces in components/interfaces crate |
| II. Code Quality & Maintainability | ✅ PASS | Will enforce fmt, clippy -D warnings, unsafe with SAFETY comments |
| III. Comprehensive Testing | ✅ PASS | Unit tests for all public APIs, doc tests planned |
| IV. Performance Validation | ✅ PASS | Criterion benchmarks for latency and throughput planned |
| V. Documentation | ✅ PASS | Doc comments with examples on all public items |
| VI. Correctness Assurance | ✅ PASS | Checked arithmetic for LBA calculations, typed errors at boundaries |
| VII. Platform Constraints | ✅ PASS | Linux only, Rust 1.75+, edition 2021 |

All gates pass. No violations to justify.

## Project Structure

### Documentation (this feature)

```text
specs/001-block-device-filesys/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (/speckit-tasks)
```

### Source Code (repository root)

```text
src/
├── lib.rs               # Component definition (define_component!), IBlockDevice impl
├── actor.rs             # FilesysActor — io_uring event loop, command dispatch
├── config.rs            # DeviceConfig struct, validation
└── telemetry.rs         # Feature-gated telemetry (mirrors block-device-spdk-nvme)

benches/
├── latency.rs           # Criterion: command construction + sync/async IO latency
└── throughput.rs        # Criterion: batch construction + IO throughput

tests/
└── integration.rs       # End-to-end tests with temporary backing files
```

**Structure Decision**: Single Rust crate following the same layout as `block-device-spdk-nvme` — `src/` for implementation, `benches/` for Criterion benchmarks, `tests/` for integration tests. Unit tests are inline in each module.

## Complexity Tracking

No constitution violations. No complexity justification needed.
