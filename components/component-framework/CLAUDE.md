# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Rust component framework targeting Linux. Managed with speckit v0.4.0 for spec-driven development.

## Build and Test Commands

```bash
cargo fmt --check          # Check formatting
cargo clippy -- -D warnings # Lint (warnings are errors)
cargo test --all           # Unit + integration + doc tests
cargo test --doc           # Doc tests only
cargo bench                # Criterion benchmarks
cargo doc --no-deps        # Build documentation (must be warning-free)
```

**CI gate** (all must pass before merge):
```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps && cargo bench --no-run
```

## Constitution (Key Rules)

The project constitution lives at `.specify/memory/constitution.md`. Core mandates:

1. **Correctness First** — Every public API must have unit tests. All Rust public APIs must have doc tests. Unsafe code must be justified and tested.
2. **Comprehensive Testing** — Unit, integration, and doc tests are mandatory. TDD preferred. `cargo test --all` must pass with zero failures.
3. **Performance Accountability** — Performance-sensitive APIs must have Criterion benchmarks in `benches/`. Regressions must be justified or fixed.
4. **Documentation as Contract** — Every public type/function/method must have doc comments with runnable examples. `cargo doc --no-deps` must be warning-free.
5. **Maintainability** — Minimal public API surface. `cargo fmt` + `cargo clippy` enforced. Single-responsibility modules.

**Platform**: Linux only. Rust stable toolchain (no nightly features).

## Speckit Workflow

Available slash commands for spec-driven development:

- `/speckit.constitution` — Define/update project principles
- `/speckit.specify` — Create/update feature spec from natural language
- `/speckit.clarify` — Identify underspecified areas in spec
- `/speckit.plan` — Generate implementation plan
- `/speckit.tasks` — Generate dependency-ordered tasks
- `/speckit.implement` — Execute implementation plan
- `/speckit.analyze` — Cross-artifact consistency check
- `/speckit.drift` — Analyze drift between specs and code

Feature artifacts live under `.specify/features/<feature-name>/`.

## Active Technologies
- Rust stable (edition 2021, MSRV 1.75+) + `proc-macro2`, `quote`, `syn` (proc macros); (001-com-component-framework)
- Rust stable (edition 2021, MSRV 1.75+) + `proc-macro2`, `quote`, `syn` (existing); no new external dependencies (002-registry-refcount-binding)
- N/A (in-memory runtime constructs) (002-registry-refcount-binding)
- Rust stable (edition 2021, MSRV 1.75+) + `proc-macro2`, `quote`, `syn` (existing proc macros); no new external dependencies (003-actor-channels)
- Rust stable (edition 2021, MSRV 1.75+) + crossbeam-channel 0.5, kanal 0.1, rtrb 0.3, tokio (sync feature only) 1.x, criterion 0.5 (existing) (004-channel-benchmarks)
- N/A (in-memory channels) (004-channel-benchmarks)
- Rust stable (edition 2021, MSRV 1.75+) + `libc` 0.2 (already in Cargo.lock; add as direct dependency), `criterion` 0.5.1 (existing) (005-numa-aware-actors)
- N/A (in-memory constructs + sysfs reads) (005-numa-aware-actors)

## Recent Changes
- 001-com-component-framework: Added Rust stable (edition 2021, MSRV 1.75+) + `proc-macro2`, `quote`, `syn` (proc macros);
