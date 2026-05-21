# Implementation Plan: SPDK NVMe Block Device Component

**Branch**: `001-spdk-nvme-block-device` | **Date**: 2026-04-14 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/001-spdk-nvme-block-device/spec.md`

## Summary

Build a Rust component (`block-device-spdk-nvme`) that provides high-performance
NVMe block device access using SPDK. The component follows the actor model with
NUMA-aware thread pinning, exposes an `IBlockDevice` interface for channel-based
client connections, supports synchronous/asynchronous IO with timeout and abort,
namespace management, batch operations, controller reset, device introspection,
and feature-gated telemetry. It integrates with the existing component-framework,
spdk-env, spdk-sys, and interfaces crates.

## Technical Context

**Language/Version**: Rust stable (edition 2021, MSRV 1.75)
**Primary Dependencies**: component-framework (actor, channels, NUMA, IUnknown),
  spdk-sys (FFI bindings), spdk-env (SPDK initialization), interfaces (ILogger,
  ISPDKEnv, DmaBuffer, PciAddress, BlockDeviceError), example-logger (testing),
  crossbeam-channel (benchmarks), criterion (benchmarks)
**Storage**: Direct NVMe via SPDK (userspace, zero-copy)
**Testing**: `cargo test --all` (unit + integration + doc tests),
  `cargo bench` (Criterion benchmarks)
**Target Platform**: Linux (VFIO/UIO + hugepages)
**Project Type**: Library (Rust crate, workspace member)
**Performance Goals**: Single-digit microsecond 4KB read/write latency,
  throughput scaling with batch size and queue depth
**Constraints**: NUMA-local actor thread, SPDK environment must be pre-initialized,
  in-process clients only (Arc references in messages)
**Scale/Scope**: Single NVMe controller per instance, multiple concurrent clients

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Evidence |
|-----------|--------|----------|
| I. Correctness First | PASS | Every public API will have unit tests; unsafe SPDK FFI wrappers will have `// SAFETY:` comments and boundary tests |
| II. Comprehensive Testing | PASS | Unit tests (all public APIs), doc tests (all public types/functions), integration tests (cross-component), Criterion benchmarks (latency/throughput) |
| III. Performance Accountability | PASS | Criterion benchmarks at varying queue depths in `benches/latency.rs` and `benches/throughput.rs` |
| IV. Documentation as Contract | PASS | All public types, functions, and methods will have `///` doc comments with runnable examples; `cargo doc --no-deps` zero warnings |
| V. Maintainability | PASS | `cargo fmt` + `cargo clippy` enforced; minimal public API surface; single-responsibility modules (actor, controller, namespace, qpair, telemetry) |
| VI. Component-Framework Conformance | PASS | Uses `define_component!` macro; actor model with dedicated thread; SPSC channels for client messaging; ILogger and ISPDKEnv via receptacles |

## Project Structure

### Documentation (this feature)

```text
specs/001-spdk-nvme-block-device/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── iblock_device.md # IBlockDevice interface contract
└── tasks.md             # Phase 2 output (/speckit-tasks)
```

### Source Code (repository root)

```text
src/
├── lib.rs               # Component definition (define_component!), IBlockDevice impl
├── actor.rs             # ActorHandler<ControlMessage> — message loop, SPDK polling
├── command.rs           # Command, Completion, ControlMessage enums
├── controller.rs        # Safe wrapper around SPDK NVMe controller FFI
├── namespace.rs         # Namespace probe/create/format/delete operations
├── qpair.rs             # IO queue pair pool, depth-based selection heuristic
├── telemetry.rs         # Feature-gated (cfg(feature = "telemetry")) stats collection
└── error.rs             # Extended BlockDeviceError variants

tests/
└── integration.rs       # Cross-component wiring and basic IO tests

benches/
├── latency.rs           # Criterion: sync/async IO latency at queue depths 1, 4, 16, 64
└── throughput.rs        # Criterion: batch throughput at batch sizes 1, 8, 32, 128
```

**Structure Decision**: Single Rust crate at `components/block-device-spdk-nvme/`
registered as a workspace member. Follows the same layout as `spdk-env` and
`spdk-simple-block-device`. Excluded from `default-members` (requires SPDK).

## Complexity Tracking

> No constitution violations. No complexity justification required.

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| (none)    | —          | —                                   |
