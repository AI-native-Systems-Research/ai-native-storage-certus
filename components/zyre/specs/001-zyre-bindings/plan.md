# Implementation Plan: Zyre Rust Bindings

**Branch**: `001-zyre-bindings` | **Date**: 2026-07-01 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/001-zyre-bindings/spec.md`

## Summary

Provide safe, idiomatic Rust bindings for the zyre C library (zero-configuration LAN peer discovery and group messaging). The implementation pulls zyre and its dependencies (libzmq, czmq) into a sub-repo at `deps/zyre-build/`, generates FFI bindings via bindgen, and presents a Rust-native API with RAII, typed events, public-field configuration structs (`NodeConfig`/`GossipConfig`, validated at `create_node`), and Result-based errors. The `IZyre` component interface acts as a factory returning `Box<dyn IZyreNode>` handles.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75  
**Primary Dependencies**: zyre v2.0.1 (C), czmq (C), libzmq (C), bindgen (build-time)  
**Storage**: N/A  
**Testing**: cargo test, integration tests with two-node localhost scenarios  
**Target Platform**: Linux (RHEL/Fedora, consistent with Certus)  
**Project Type**: Library (Rust crate within the Certus component framework)  
**Performance Goals**: Peer discovery + round-trip message exchange within 2 seconds on localhost  
**Constraints**: No internal background threads, no async runtime dependency, no system-wide library installation  
**Scale/Scope**: Wraps ~40 zyre C API functions, ~10 zyre_event functions

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

*Constitution inherited from component-framework (v1.0.0, ratified 2026-03-30).*

| Principle | Gate | Status | Notes |
|-----------|------|--------|-------|
| I. Code Correctness First | Unit tests + doc tests on all public APIs; unsafe justified | PASS | Tests planned for all public APIs. Doc tests in all public items. Unsafe confined to FFI with `// SAFETY:` comments. Clippy enforced. |
| II. Comprehensive Testing | Unit + integration + doc tests; TDD preferred | PASS | Unit tests per module, integration tests (two-node localhost), doc tests on all public types. `cargo test -p zyre` must pass with zero failures. |
| III. Performance Accountability | Criterion benchmarks for perf-sensitive code | PASS (deferred) | Justified: Rust bindings are thin FFI — performance is determined by the C library, not the wrapper. FFI overhead is negligible. Benchmarks added if message throughput becomes a concern. |
| IV. Documentation as Contract | Doc comments with runnable examples; `cargo doc --no-deps` clean | PASS | All public types/functions/methods will have `///` doc comments with examples. |
| V. Maintainability and Simplicity | Minimal API surface; fmt+clippy; single responsibility | PASS | Small crate — `lib.rs` (component + `IZyre` impl), `node.rs` (crate-private FFI-owning `ZyreNode` + event parsing), `ffi.rs` (bindgen re-export). Value types and the `IZyre`/`IZyreNode` traits live in the `interfaces` crate (`izyre.rs`) to avoid a crate cycle. Minimal public surface. |
| Platform Constraints | Linux only, Rust stable, no nightly | PASS | Targets Linux. Rust stable, edition 2021, MSRV 1.75. |
| CI Gate | fmt + clippy + test + doc | PASS | Excluded from default-members (like SPDK). CI runs when explicitly targeted with `-p zyre`. |

No gate failures. Proceeding.

## Project Structure

### Documentation (this feature)

```text
specs/001-zyre-bindings/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── izyre.md         # IZyre interface contract
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
components/zyre/
├── Cargo.toml
├── src/
│   ├── lib.rs           # define_component! + IZyre impl; re-exports interface types
│   ├── node.rs          # ZyreNode (crate-private) — safe wrapper over zyre_t + event parsing
│   └── ffi.rs           # include! of bindgen-generated bindings
├── build.rs             # bindgen invocation, link configuration
└── tests/
    ├── integration.rs   # Two-node localhost discovery/shout/whisper/gossip tests
    └── api_safety.rs    # Send / !Sync / no-unsafe compile-time assertions

components/interfaces/src/
└── izyre.rs             # IZyre + IZyreNode traits; NodeConfig, GossipConfig,
                         #   ZyreEvent, PeerId, ZyreError (here to avoid a crate cycle)

deps/
├── build_zyre.sh        # Clone + build libzmq, czmq, zyre → deps/zyre-build/
├── zyre/                # Cloned zyre source (gitignored)
├── zyre-build/          # Installed libs + headers
│   ├── lib/
│   ├── include/
│   └── share/pkgconfig/
└── install_zyre_deps.sh # System package prerequisites (cmake, pkg-config, etc.)
```

**Structure Decision**: Single crate under `components/zyre/` following the existing component layout. The `IZyre`/`IZyreNode` traits and all value types live in the `interfaces` crate so `IZyre::create_node` can name them without a crate cycle; the concrete `ZyreNode` is crate-private in `zyre`. C dependencies built into `deps/zyre-build/` at the workspace root, parallel to `deps/spdk-build/`. The crate is added to workspace `members` but NOT to `default-members` (requires pre-built C libraries, same gating as SPDK crates).

## Complexity Tracking

No constitution violations to justify.
