# Implementation Plan: RDMA Network Test Tool

**Branch**: `001-rdma-network-test` | **Date**: 2026-06-05 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/001-rdma-network-test/spec.md`

## Summary

Build a Rust CLI tool that measures RDMA throughput (via RDMA Write) and latency/jitter (via Send/Recv ping-pong) between two nodes. The tool operates in client/server mode, uses raw ibverbs FFI bindings for zero-overhead benchmarking, auto-detects IB/RoCE devices, and includes an SSH launch script for remote orchestration. Outputs in human-readable or JSON format.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**: libibverbs (FFI), librdmacm (FFI), clap 4 (CLI), tokio 1 (async runtime), serde/serde_json (JSON output)

**Storage**: N/A (in-memory measurements only)

**Testing**: cargo test (unit tests for stats module), integration testing requires RDMA hardware or SoftRoCE

**Target Platform**: Linux (RHEL/Fedora with rdma-core or MLNX OFED)

**Project Type**: CLI tool (standalone binary)

**Performance Goals**: Sub-microsecond latency measurement resolution; throughput limited only by hardware (25-400 Gbps depending on NIC)

**Constraints**: Must link against system libibverbs/librdmacm; cannot use bindgen (system headers incompatible with older bindgen versions); manual FFI bindings required

**Scale/Scope**: Point-to-point single-stream benchmark; 10K-1M iterations per test run

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Constitution is uninitialized (template placeholders only). No project-specific governance constraints to enforce. Gate passes by default.

## Project Structure

### Documentation (this feature)

```text
specs/001-rdma-network-test/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output (CLI interface contract)
└── tasks.md             # Phase 2 output (/speckit-tasks command)
```

### Source Code (repository root)

```text
src/
├── main.rs              # CLI parsing, ibverbs check, dispatch
├── ffi.rs               # Manual FFI bindings to libibverbs/librdmacm
├── rdma.rs              # Safe wrapper layer over FFI (context, QP, MR management)
├── server.rs            # RDMA server (CM listener, passive side)
├── client.rs            # RDMA client (CM connector, active side)
├── throughput.rs        # RDMA Write throughput benchmark
├── latency.rs           # Send/Recv ping-pong latency benchmark
├── stats.rs             # Statistics computation and formatting
└── output.rs            # Human-readable and JSON output formatting

scripts/
└── launch.sh            # SSH-based remote client/server orchestration

build.rs                 # Link flags for libibverbs/librdmacm
Cargo.toml               # Project manifest with [workspace]
```

**Structure Decision**: Single-binary CLI project with manual FFI bindings. No separate `tests/` directory needed since the benchmark requires hardware; unit tests for `stats` module are inline. The `ffi.rs` module provides raw C bindings, `rdma.rs` wraps them in safe Rust abstractions (RAII for PD/MR/QP/CQ lifecycle).

## Complexity Tracking

No constitution violations. No complexity justification needed.
