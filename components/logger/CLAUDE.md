# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Logger v1 — a logging component for the Certus storage system. Provides console and file-based logging with configurable log levels via the `RUST_LOG` environment variable (matching `env_logger` conventions). Built with the component-framework using `define_component!` and `define_interface!` macros.

This component is fully implemented. The feature spec lives at `specs/001-logger-component/spec.md`. All functional requirements (FR-001 through FR-011) and success criteria (SC-001 through SC-007) are satisfied with unit, integration, doc tests, and Criterion benchmarks.

## Build and Test Commands

This crate must be added to the workspace `Cargo.toml` (both `members` and `default-members`) before building. Once added:

```bash
cargo build -p logger                            # Build
cargo test -p logger                             # All tests (unit + integration + doc)
cargo test -p logger test_name                   # Run a single test by name
cargo fmt -p logger --check                      # Check formatting
cargo clippy -p logger -- -D warnings            # Lint (warnings are errors)
cargo doc -p logger --no-deps                    # Build documentation
cargo bench -p logger                            # Run Criterion benchmarks
cargo bench -p logger --no-run                   # Verify benchmarks compile
```

**CI gate** (all must pass before merge):
```bash
cargo fmt -p logger --check \
  && cargo clippy -p logger -- -D warnings \
  && cargo test -p logger \
  && cargo doc -p logger --no-deps \
  && cargo bench -p logger --no-run
```

## Architecture

### Component Wiring

```
LoggerComponent --> [ILogger provider]
```

**Lifecycle**: `new_default()` → optionally configure file output → use `ILogger` methods.

### ILogger Interface

The `ILogger` interface must be defined in the shared `interfaces` crate (at `../../interfaces/`) using `define_interface!`, following the same pattern as `IGreeter` and `IExtentManager`. It should support log methods at different levels (error, warn, info, debug, trace).

### Design Requirements (from PROMPT.md)

- Log levels controlled by `RUST_LOG` environment variable (same semantics as `env_logger`)
- Console (stderr) output by default
- Optional file output as an alternative to console
- Log format includes: timestamp, log level, and message
- Interface named `ILogger`, added to the `interfaces` crate

### Key Internal Dependencies

- `component-framework`, `component-core`, `component-macros` — at `../../component-framework/crates/`
- `interfaces` — at `../../interfaces` — where `ILogger` must be defined

### Integration Points

Other Certus components declare `ILogger` as a receptacle for dependency-injected logging. The `extent-manager` and `block-device-spdk-nvme` components both expect an `ILogger` receptacle. This logger component provides that implementation.

## Constitution (Key Rules)

1. **Correctness First** — Every public API must have unit tests and doc tests. Unsafe code must be justified and tested.
2. **Comprehensive Testing** — Unit, integration, and doc tests are mandatory. `cargo test` must pass with zero failures.
3. **Performance Accountability** — Performance-sensitive APIs must have Criterion benchmarks in `benches/`.
4. **Documentation as Contract** — Every public type/function/method must have doc comments with runnable examples. `cargo doc --no-deps` must be warning-free.
5. **Maintainability** — Minimal public API surface. `cargo fmt` + `cargo clippy` enforced.

**Platform**: Linux only. Rust stable toolchain, edition 2021, MSRV 1.75. No nightly features.

## Speckit Workflow

This project uses speckit for spec-driven development. Feature artifacts live under `.specify/features/<feature-name>/`. Key slash commands:

- `/speckit-specify` — Create/update feature spec from natural language
- `/speckit-plan` — Generate implementation plan
- `/speckit-tasks` — Generate dependency-ordered tasks
- `/speckit-implement` — Execute implementation plan
- `/speckit-constitution` — Define/update project principles
- `/speckit-analyze` — Cross-artifact consistency check
- `/speckit-drift` — Analyze drift between specs and code

## Active Technologies
- Rust stable, edition 2021, MSRV 1.75 + `component-framework`, `component-core`, (001-logger-component)
- File I/O via `std::fs::File` (append mode) for file output (001-logger-component)

## Recent Changes
- 001-logger-component: Added Rust stable, edition 2021, MSRV 1.75 + `component-framework`, `component-core`,
