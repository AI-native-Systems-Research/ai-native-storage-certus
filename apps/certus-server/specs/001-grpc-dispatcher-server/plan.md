# Implementation Plan: gRPC Dispatcher Server

**Branch**: `dispatcher` | **Date**: 2026-05-05 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-grpc-dispatcher-server/spec.md`

## Summary

Build a Rust gRPC server (`certus-server`) that exposes the IDispatcher interface (populate, lookup, check, remove) to Python clients via tonic/gRPC. The server auto-initializes the full Certus component stack (SPDK, GPU services, dispatch-map, dispatcher) on startup using CLI-provided PCI addresses. All gRPC methods accept batched requests (list of entries) and iterate server-side, returning per-entry results. A Python test client validates all operations.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75  
**Primary Dependencies**: tonic (gRPC), prost (protobuf), tokio (async runtime), clap (CLI)  
**Storage**: NVMe via SPDK (managed by dispatcher component)  
**Testing**: Python test client (`grpcio`/`grpcio-tools`), cargo build verification  
**Target Platform**: Linux (RHEL/Fedora)  
**Project Type**: CLI server application  
**Performance Goals**: 1000-entry batches without timeout; <10s startup (excluding SPDK)  
**Constraints**: Serialized request processing; same-machine client/server (GPU DMA)  
**Scale/Scope**: Single dispatcher instance, single server process

## Constitution Check

*GATE: No constitution gates defined (template only). Proceeding.*

No violations — constitution file contains only placeholders.

Post-Phase 1 re-check: No new violations introduced by design decisions.

## Project Structure

### Documentation (this feature)

```text
specs/001-grpc-dispatcher-server/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── dispatcher.proto # gRPC service definition
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
apps/certus-server/
├── Cargo.toml           # Package manifest (tonic, prost, clap, tokio, dispatcher deps)
├── build.rs             # Prost/tonic codegen from proto/dispatcher.proto
├── proto/
│   └── dispatcher.proto # Protobuf service definition
├── src/
│   ├── main.rs          # CLI parsing, component wiring, gRPC server startup, signal handling
│   └── service.rs       # gRPC service implementation (batch iteration, error mapping)
└── python-client/
    ├── requirements.txt # grpcio, grpcio-tools, protobuf
    ├── generate_pb.sh   # Script to generate Python stubs from proto
    └── test_client.py   # Test client exercising all batch operations
```

**Structure Decision**: Single crate in `apps/certus-server/` following the established workspace app pattern (see `apps/gpu-handle-test-server/`, `apps/iops-benchmark/`). The proto file lives alongside the Rust source for build.rs codegen. Python client lives as a subdirectory with its own requirements.txt, matching `apps/gpu-handle-test-client/` pattern.

## Key Design Decisions

### Component Wiring (from research.md R6)

Follow `certus-connector/src/engine.rs` initialization sequence:
1. SPDKEnvComponent → `ISPDKEnv::init()`
2. GpuServicesComponentV0 → `IGpuServices::initialize()`
3. DispatchMapComponentV0 → `IDispatchMap::initialize()`
4. DispatcherComponentV0 → bind receptacles → `IDispatcher::initialize(config)`

### Request Serialization (from research.md R7)

Wrap the `Arc<dyn IDispatcher>` in a `std::sync::Mutex`. Each gRPC handler acquires the lock inside `spawn_blocking`, processes all batch entries, then releases. This ensures one-at-a-time semantics (FR-013) while keeping the tokio event loop responsive.

### Batch Duplicate Pre-validation (FR-015)

Before acquiring the dispatcher lock, collect all keys from the batch into a `HashSet`. If `set.len() != entries.len()`, reject the entire request with `ErrorCode::DUPLICATE_KEY` at the gRPC level (not per-entry).

### Error Mapping

Direct 1:1 mapping from `DispatcherError` variants to `ErrorCode` enum values:
- `NotInitialized` → `NOT_INITIALIZED`
- `KeyNotFound` → `KEY_NOT_FOUND`
- `AlreadyExists` → `ALREADY_EXISTS`
- `AllocationFailed` → `ALLOCATION_FAILED`
- `IoError` → `IO_ERROR`
- `Timeout` → `TIMEOUT`
- `InvalidParameter` → `INVALID_PARAMETER`

## Complexity Tracking

No constitution violations to justify.
