# Implementation Plan: COM-Style Component Framework

**Branch**: `001-com-component-framework` | **Date**: 2026-03-30 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-com-component-framework/spec.md`

## Summary

Build a Microsoft COM-inspired component framework in Rust that provides:
macro-defined interfaces, components with `IUnknown` introspection,
typed receptacles for required-interface wiring, and `Arc`-based
thread-safe composition. Procedural macros (`define_interface!`,
`define_component!`) generate trait definitions, metadata, and
boilerplate so that interface crates can be consumed without access to
implementation crates.

## Technical Context

**Language/Version**: Rust stable (edition 2021, MSRV 1.75+)
**Primary Dependencies**: `proc-macro2`, `quote`, `syn` (proc macros);
`criterion` (benchmarks); no runtime dependencies beyond `std` and `tokio`
**Storage**: N/A
**Testing**: `cargo test` (unit + integration + doc tests); Criterion
benchmarks in `benches/`
**Target Platform**: Linux (x86_64)
**Project Type**: Library (Rust workspace with multiple crates)
**Performance Goals**: Interface query ≤50 ns; receptacle method
dispatch ≤10 ns overhead vs direct trait call; zero heap allocation on
query path after component construction
**Constraints**: Stable Rust only (no nightly); `Send + Sync` on all
public types; minimal dependencies
**Scale/Scope**: Framework crate consumed by downstream component authors;
expected interface count per component: 1–20

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Requirement | Status |
|-----------|-------------|--------|
| I. Code Correctness | Unit tests for all public APIs; doc tests for all public APIs; clippy clean; unsafe justified + tested | ✅ Planned |
| II. Comprehensive Testing | Unit + integration + doc tests; TDD workflow; `cargo test --all` zero failures | ✅ Planned |
| III. Performance Accountability | Criterion benchmarks for query_interface, receptacle connect/disconnect, method dispatch | ✅ Planned |
| IV. Documentation as Contract | Doc comments + doc tests on all public types/functions; `cargo doc --no-deps` zero warnings | ✅ Planned |
| V. Maintainability | Minimal API surface; `cargo fmt` + `cargo clippy`; single-responsibility crates | ✅ Planned |
| Platform Constraints | Linux, stable Rust, no nightly features | ✅ Planned |
| CI Gate | `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps` | ✅ Planned |

**Gate result**: PASS — no violations. All principles addressable within the design.

## Project Structure

### Documentation (this feature)

```text
specs/001-com-component-framework/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md    # Public API contract
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
Cargo.toml                          # Workspace root
crates/
├── component-core/                 # Core traits and types
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                  # Crate root, re-exports
│       ├── interface.rs            # Interface trait, InterfaceInfo
│       ├── iunknown.rs             # IUnknown trait definition
│       ├── receptacle.rs           # Receptacle<T> type
│       ├── component.rs            # Component base types, InterfaceMap
│       └── error.rs                # Framework error types
├── component-macros/               # Procedural macro crate
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                  # Proc macro entry points
│       ├── define_interface.rs     # define_interface! implementation
│       └── define_component.rs     # define_component! implementation
└── component-framework/            # Facade crate (re-exports core + macros)
    ├── Cargo.toml
    └── src/
        └── lib.rs                  # pub use component_core::*; pub use component_macros::*;

benches/
├── query_interface.rs              # Criterion: IUnknown::query_interface
├── receptacle.rs                   # Criterion: connect/disconnect/dispatch
└── method_dispatch.rs              # Criterion: interface method call overhead

tests/
├── interface_definition.rs         # Integration: macro generates valid traits
├── component_iunknown.rs           # Integration: IUnknown query/version/enumerate
├── receptacle_wiring.rs            # Integration: connect/disconnect/invoke
└── cross_crate_isolation.rs        # Integration: interface-only dependency
```

**Structure Decision**: Rust workspace with three crates following the
core/macros/facade pattern. `component-core` contains all runtime types
and traits. `component-macros` is a `proc-macro` crate (required by
Cargo for procedural macros). `component-framework` is the user-facing
facade that re-exports both.

## Complexity Tracking

No constitution violations to justify. The three-crate split is the
minimum required by Rust's proc-macro rules (proc macros must live in
a dedicated crate) and the isolation requirement (interfaces without
implementations).
