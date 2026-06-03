# Implementation Plan: Composable Server with Dynamic Component Loading

**Branch**: `001-composable-server-dylib` | **Date**: 2026-06-03 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/001-composable-server-dylib/spec.md`

## Summary

Build `certus-server-composable`, a runtime-configurable variant of `certus-server` that loads all components as dynamic libraries (`.so` dylibs) based on a JSON configuration file. The configuration declares which dylibs to load, how many instances to create (with variable substitution), and how to bind component receptacles to interfaces. Initialization order is derived from binding dependencies via topological sort, with an optional explicit override. The gRPC service layer remains compiled into the binary and exposes the identical `certus.dispatcher.v1` API.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**: `libloading` (dynamic library loading), `serde`/`serde_json` (JSON parsing), `tonic` 0.12 (gRPC), `tokio` 1.x (async runtime), `clap` 4.x (CLI), `component-core` (ComponentRef, IUnknown)

**Storage**: N/A (components handle their own storage)

**Testing**: `cargo test`, Criterion benchmarks, integration tests with mock dylibs

**Target Platform**: Linux only (RHEL/Fedora), `.so` shared libraries

**Project Type**: Binary application (gRPC server)

**Performance Goals**: Startup within 5 seconds of static certus-server (excluding SPDK init); identical runtime throughput

**Constraints**: Must replicate full certus-server gRPC API; all components loaded dynamically; no compile-time component dependencies

**Scale/Scope**: 6-9 component types in a typical deployment; up to 8+ block-device instances

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Code Quality & Correctness | PASS | clippy -D warnings, rustfmt, SAFETY comments on unsafe dylib FFI |
| II. Extensive Testing | PASS | Unit tests for config parsing/validation/topo-sort; integration tests with mock dylibs; benchmarks for loading path |
| III. Documentation | PASS | Public APIs documented with doc tests; module-level docs on each module |
| IV. Component Architecture | PASS | Uses `create_component()` convention; all components via `ComponentRef` |
| V. Interface Discipline | PASS | Bindings use `connect_receptacle_raw` via IUnknown; no public functions outside interfaces |
| VI. Performance Engineering | PASS | Benchmark startup path; dylib load time profiling |
| VII. Maintainability | PASS | Single-responsibility modules; minimal external dependencies |

No violations.

## Project Structure

### Documentation (this feature)

```text
specs/001-composable-server-dylib/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (/speckit-tasks command)
```

### Source Code (repository root)

```text
apps/certus-server-composable/
├── Cargo.toml
├── build.rs                   # tonic-build for proto
├── proto/
│   └── dispatcher.proto       # Copied from certus-server (identical API)
├── src/
│   ├── main.rs                # CLI parsing, orchestration entry point
│   ├── config.rs              # JSON config parsing and validation (serde)
│   ├── loader.rs              # Dylib loading via libloading, create_component() call
│   ├── resolver.rs            # Search path resolution for dylib files
│   ├── topology.rs            # Topological sort and init_order logic
│   ├── binder.rs              # Component binding orchestration (connect_receptacle_raw)
│   ├── runtime.rs             # Component lifecycle (init, shutdown, fail-fast teardown)
│   └── service.rs             # gRPC service impl (carried from certus-server)
├── configs/
│   ├── example-production.json
│   └── example-dev.json
└── tests/
    ├── config_validation_test.rs
    ├── topology_test.rs
    ├── loader_test.rs
    └── integration_test.rs
```

**Structure Decision**: Single Rust binary crate under `apps/certus-server-composable/`. Source modules follow single-responsibility: config parsing, dylib loading, path resolution, topological ordering, binding orchestration, and runtime lifecycle are each separate modules. The gRPC service layer is carried directly from certus-server with minimal changes (receives dispatcher via `Arc<dyn IDispatcher>`).

## Complexity Tracking

No Constitution Check violations — this section is empty.
