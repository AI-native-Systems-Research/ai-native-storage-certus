<!--
  Sync Impact Report
  ==================
  Version change: 0.0.0 → 1.0.0 (initial ratification)
  Added principles:
    - I. Code Quality & Correctness
    - II. Extensive Testing
    - III. Documentation
    - IV. Component Architecture
    - V. Interface Discipline
    - VI. Performance Engineering
    - VII. Maintainability
  Added sections:
    - Platform & Toolchain Constraints
    - Development Workflow & Quality Gates
  Templates requiring updates:
    - .specify/templates/plan-template.md ✅ (Constitution Check section compatible)
    - .specify/templates/spec-template.md ✅ (requirements/success criteria aligned)
    - .specify/templates/tasks-template.md ✅ (test-first phasing compatible)
  Follow-up TODOs: None
-->

# Certus Constitution

## Core Principles

### I. Code Quality & Correctness

All code MUST compile without warnings under `cargo clippy -- -D warnings`.
All code MUST be formatted with `rustfmt` (default configuration).
Unsafe code MUST include a `// SAFETY:` comment justifying correctness.
All logic MUST be verified through unit tests that exercise both the
expected path and error/edge-case paths. Assurance of code correctness
is a primary engineering objective — when in doubt, add a test.

### II. Extensive Testing

Every module MUST have unit tests that verify functional correctness.
Every performance-sensitive path MUST have Criterion benchmarks with
documented baseline expectations. Integration tests MUST cover
cross-component interactions. Tests MUST run single-threaded in CI
(`--test-threads 1`) to ensure deterministic results. Test coverage
MUST include error paths, boundary conditions, and concurrent access
patterns where applicable. All tests MUST pass before code is merged.

### III. Documentation

All public APIs MUST have Rust doc comments (`///`) with runnable
`# Examples` sections that compile and execute as documentation tests
(`cargo test --doc`). `cargo doc --no-deps` MUST produce zero warnings.
Internal modules MUST include module-level documentation (`//!`)
explaining purpose and design rationale. Per-component `README.md` files
MUST accurately reflect current functionality.

### IV. Component Architecture

All new components MUST conform to the `components/component-framework`
methodology. Components MUST use `define_component!` and
`define_interface!` macros. Every component MUST implement `IUnknown`
for runtime interface discovery. Dependencies between components MUST
be declared as typed receptacles and wired via `bind()`. Actor-based
components MUST run on dedicated OS threads with lock-free channel
communication.

### V. Interface Discipline

Components MUST only expose functionality through interfaces defined
in the `components/interfaces` crate. Public functions outside the
component boundary are NOT allowed — all external access goes through
interface trait methods. Interface traits MUST be defined with
`define_interface!` in the shared interfaces crate, not locally.
New interface definitions MUST include documentation tests
demonstrating usage patterns.

### VI. Performance Engineering

Performance-sensitive code MUST have Criterion benchmarks. Performance
requirements MUST be stated as measurable targets (latency percentiles,
throughput, memory bounds). Regressions MUST be caught by benchmark
comparison before merge. Lock-free and zero-copy patterns are preferred
where they do not compromise correctness. Memory allocation in hot
paths MUST be minimized and justified.

### VII. Maintainability

Code MUST follow established Rust idioms and the principle of least
surprise. Modules MUST have a single, clear responsibility. Dependencies
MUST be minimized — only add a crate when it provides substantial value
over a local implementation. Feature flags MUST gate optional
heavyweight dependencies (e.g., SPDK). Dead code MUST be removed, not
commented out. Complexity MUST be justified in comments when unavoidable.

## Platform & Toolchain Constraints

- **Operating System**: Linux only (RHEL/Fedora tested). No
  cross-platform abstractions for Windows or macOS.
- **Language**: Rust stable, edition 2021, MSRV 1.75.
- **Build System**: Cargo workspaces with `default-members` excluding
  hardware-dependent crates.
- **CI**: GitHub Actions on `ubuntu-latest`, single-threaded test
  execution, clippy warnings as errors, fmt check enforced.
- **Hardware-dependent crates** (SPDK): Built explicitly with `-p`,
  require pre-built SPDK at `deps/spdk-build/`.

## Development Workflow & Quality Gates

1. **Format gate**: `cargo fmt --check` MUST pass.
2. **Lint gate**: `cargo clippy -- -D warnings` MUST pass.
3. **Test gate**: `cargo test --all` MUST pass (single-threaded in CI).
4. **Doc gate**: `cargo doc --no-deps` MUST produce zero warnings.
5. **Benchmark gate**: Performance-sensitive changes MUST include
   benchmark results showing no regression.
6. **Review gate**: All changes MUST be reviewed for adherence to
   this constitution before merge.

## Governance

This constitution is the authoritative source of engineering standards
for the Certus project. All code contributions MUST comply with these
principles. Violations discovered in review MUST be resolved before
merge.

**Amendment procedure**: Amendments require explicit justification,
documentation of the change rationale, and a version bump following
semantic versioning (MAJOR for principle removal/redefinition, MINOR
for new principles or material expansion, PATCH for clarifications).

**Compliance review**: Each pull request MUST include a self-assessment
against the Constitution Check section in the implementation plan.
Reviewers MUST verify compliance with all applicable principles.

**Version**: 1.0.0 | **Ratified**: 2026-06-03 | **Last Amended**: 2026-06-03
