# Implementation Plan: Channel Backend Benchmarks

**Branch**: `004-channel-benchmarks` | **Date**: 2026-03-31 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/004-channel-benchmarks/spec.md`

## Summary

Implement five third-party channel backends (crossbeam bounded, crossbeam unbounded, kanal, rtrb, tokio) as components in the existing COM-style framework, each providing ISender/IReceiver via IUnknown. Build a Criterion benchmark suite comparing throughput and latency across all backends under SPSC and MPSC topologies, varying message sizes, queue depths, and producer counts.

## Technical Context

**Language/Version**: Rust stable (edition 2021, MSRV 1.75+)
**Primary Dependencies**: crossbeam-channel 0.5, kanal 0.1, rtrb 0.3, tokio (sync feature only) 1.x, criterion 0.5 (existing)
**Storage**: N/A (in-memory channels)
**Testing**: cargo test (unit + integration + doc tests), criterion benchmarks
**Target Platform**: Linux (x86_64)
**Project Type**: Library (component framework extension)
**Performance Goals**: Benchmark suite completes in under 5 minutes; throughput measured in millions of messages/sec; latency measured in nanoseconds per message
**Constraints**: No async runtime required (tokio channels used in sync blocking mode); all backends must implement existing ISender/IReceiver traits
**Scale/Scope**: 5 new channel backend components, 1 benchmark suite with ~20+ benchmark functions

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Code Correctness First | ✓ PASS | Each backend will have unit tests for correctness, boundary conditions, and error paths. Doc tests on all public APIs. |
| II. Comprehensive Testing | ✓ PASS | Unit tests per backend (send/recv, binding enforcement, closure), integration tests, doc tests. TDD approach. |
| III. Performance Accountability | ✓ PASS | This feature IS the performance benchmark suite. Criterion benchmarks in benches/. |
| IV. Documentation as Contract | ✓ PASS | All public types/functions will have doc comments with runnable examples. |
| V. Maintainability and Simplicity | ✓ PASS | Each backend is a single-responsibility module. Dependencies justified (each provides a distinct channel implementation). Minimal public API surface. |
| Platform/Toolchain | ✓ PASS | Linux only, Rust stable, no nightly features. |
| CI Gate | ✓ PASS | `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps` |

**New dependencies justification**: crossbeam-channel, kanal, rtrb, and tokio are well-maintained, widely-used crates with compatible licenses (MIT/Apache-2.0). Each provides a fundamentally different channel implementation needed for meaningful benchmark comparison.

## Project Structure

### Documentation (this feature)

```text
specs/004-channel-benchmarks/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
crates/component-core/
├── Cargo.toml                          # Add crossbeam-channel, kanal, rtrb, tokio deps
└── src/
    └── channel/
        ├── mod.rs                      # Existing — ISender, IReceiver, Sender, Receiver
        ├── spsc.rs                     # Existing — SpscChannel (built-in SPSC)
        ├── mpsc.rs                     # Existing — MpscChannel (built-in MPSC)
        ├── queue.rs                    # Existing — RingBuffer
        ├── crossbeam_bounded.rs        # NEW — CrossbeamBoundedChannel
        ├── crossbeam_unbounded.rs      # NEW — CrossbeamUnboundedChannel
        ├── kanal_bounded.rs            # NEW — KanalChannel
        ├── rtrb_spsc.rs               # NEW — RtrbChannel (SPSC only)
        └── tokio_mpsc.rs              # NEW — TokioMpscChannel

crates/component-framework/
├── Cargo.toml                          # Add benchmark entries
└── benches/
    ├── channel_throughput.rs           # Existing — update to include all backends
    ├── channel_spsc_benchmark.rs       # NEW — SPSC comparison across backends
    ├── channel_mpsc_benchmark.rs       # NEW — MPSC comparison across backends
    └── channel_latency_benchmark.rs    # NEW — Latency measurement across backends

crates/component-framework/tests/
└── channel_backends.rs                 # NEW — Integration tests for all backends
```

**Structure Decision**: Follows existing workspace structure. New channel backends are added as submodules under `crates/component-core/src/channel/` (same pattern as spsc.rs and mpsc.rs). Benchmarks go in `crates/component-framework/benches/` (where existing benchmarks live).

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 4 new external dependencies | Each provides a fundamentally different channel algorithm (lock-free MPSC, bounded/unbounded, SPSC ring buffer, async-compatible) | Cannot benchmark alternatives without the alternatives |
