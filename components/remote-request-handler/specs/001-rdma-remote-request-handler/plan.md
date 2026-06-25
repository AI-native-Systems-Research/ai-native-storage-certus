# Implementation Plan: RDMA Remote Request Handler

**Branch**: `feat/remote-request-handler-component` | **Date**: 2026-06-23 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/001-rdma-remote-request-handler/spec.md`

## Summary

Implement a network endpoint component that accepts RDMA connections from peer Certus nodes, processes batched cache lookups (up to 64 entries per batch, keyed by 64-bit CacheKey), and writes results directly into caller-specified remote memory via RDMA. The component runs an async listener (tokio) on a configurable TCP port (via rdma_cm), manages per-connection sessions, delegates lookups to IDispatcher (initially a logging stub), and provides optional telemetry. Security relies on network-level fabric isolation.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**:
- `component-framework` / `component-core` / `component-macros` (Certus COM framework)
- `interfaces` with `features = ["spdk"]` (IDispatcher, ILogger, IRemoteRequestHandler, CacheKey)
- `tokio` (async runtime for connection listener)
- `prost` / `prost-build` (protobuf message serialization)
- `rdma-core` bindings (rdma_cm for connection management, ibverbs for RC queue pairs and RDMA writes)

**Storage**: N/A (lookup results come from IDispatcher; no local persistence)

**Testing**: `cargo test` + standalone test-client binary

**Target Platform**: Linux with RDMA-capable NICs (InfiniBand or RoCE), rdma-core userspace libraries installed

**Project Type**: Component (library crate) + binary crate (test-client)

**Performance Goals**: 64-entry batch lookup < 500µs handler-side processing; 100 concurrent sessions

**Constraints**: Requires rdma-core userspace libraries; RDMA hardware (or SoftRoCE for testing); single-threaded test execution in CI (hardware-dependent tests gated behind feature flag)

**Scale/Scope**: 100 concurrent sessions; batches of up to 64 entries; internal cluster service (not public-facing)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Constitution file contains only template placeholders — no project-specific gates defined. **PASS** (no violations possible).

## Project Structure

### Documentation (this feature)

```text
specs/001-rdma-remote-request-handler/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output (protobuf definition)
└── tasks.md             # Phase 2 output (/speckit-tasks command)
```

### Source Code (component root)

```text
src/
├── lib.rs               # Component definition, IRemoteRequestHandler impl
├── listener.rs          # Tokio async listener for rdma_cm connection events
├── session.rs           # Per-connection session state machine
├── protocol.rs          # Protobuf message encode/decode (generated + wrappers)
├── rdma.rs              # RDMA resource management (QP, MR, CQ helpers)
└── telemetry.rs         # Optional metrics collection (connection rate, throughput, latency)

proto/
└── remote_request.proto # Protocol definition (protobuf)

tests/
├── unit/
│   ├── session_test.rs
│   ├── protocol_test.rs
│   └── batch_test.rs
└── integration/
    └── loopback_test.rs # SoftRoCE or mock-based integration test

src/bin/
└── test_client.rs       # Standalone test client binary

build.rs                 # prost-build for proto compilation
```

**Structure Decision**: Single component crate with an embedded binary target for the test client. Follows the existing pattern from `block-device-spdk-nvme` (component library + feature-gated tests). Proto file lives alongside the component (not in a shared location) since this protocol is specific to this component.

## Complexity Tracking

No constitution violations to justify.
