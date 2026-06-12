<!--
Sync Impact Report
- Version change: N/A → 1.0.0 (initial ratification)
- Added principles: I. Component-Framework Conformance, II. Interface-Only Exposure,
  III. Code Quality and Correctness, IV. Comprehensive Testing,
  V. Performance Measurement, VI. Documentation Standards,
  VII. Maintainability and Graceful Degradation
- Added sections: Platform and Tooling Requirements, Development Workflow
- Removed sections: none
- Templates requiring updates:
  - .specify/templates/plan-template.md — ✅ no changes needed
  - .specify/templates/spec-template.md — ✅ no changes needed
  - .specify/templates/tasks-template.md — ✅ no changes needed
- Follow-up TODOs: none
-->

# Dispatcher-P2P Constitution

## Core Principles

### I. Component-Framework Conformance

All code MUST conform to the `components/component-framework` methodology.
Components MUST use `define_component!` and `define_interface!` macros.
Every component MUST implement `IUnknown` for runtime interface discovery.
Dependencies MUST be declared as typed receptacles and wired via `bind()`.

**Rationale**: The component framework enforces loose coupling and runtime
composability across the Certus system.

### II. Interface-Only Exposure

The component MUST only expose functionality through interfaces defined in
the `components/interfaces` crate. Public functions outside the component
boundary are NOT allowed. No struct, function, or method may be made `pub`
if it is not part of an interface definition or required by the
component-framework macros for internal wiring.

**Rationale**: Consumers depend on contracts, not implementations. This
enables independent component evolution and substitution.

### III. Code Quality and Correctness

- All code MUST compile without warnings under `cargo clippy -- -D warnings`.
- All code MUST pass `cargo fmt --check` with default `rustfmt` settings.
- `cargo doc --no-deps` MUST produce zero warnings.
- All `unsafe` code MUST include a `// SAFETY:` justification comment.
- Assurance of code correctness is of high importance. All logic MUST be
  verified through tests.
- All code MUST target and run on the Linux operating system exclusively.

**Rationale**: Strict lint and format enforcement catches defects early.
Testing prevents regressions.

### IV. Comprehensive Testing

- All public APIs MUST have unit tests validating correctness.
- All public APIs MUST have Rust documentation tests (`///` doc examples)
  that compile and run as tests via `cargo test`.
- Tests MUST cover: happy paths, error paths, boundary conditions, and
  concurrent access patterns where applicable.
- Test execution MUST be deterministic and MUST NOT depend on external
  hardware (use mocks where hardware is absent).
- All tests MUST pass under single-threaded execution
  (`--test-threads 1`) for CI compatibility.

**Rationale**: Comprehensive testing is the primary mechanism for assuring
code correctness.

### V. Performance Measurement

- All performance-sensitive code MUST have Criterion-based benchmarks.
- Benchmarks MUST be available under `cargo bench` or targeted via
  `cargo bench --bench <name>`.
- Performance MUST be measurable: benchmarks MUST exist that allow
  comparison across commits and between code paths.

**Rationale**: Without measurement there is no basis for performance
decisions. Criterion benchmarks provide repeatable, comparable data.

### VI. Documentation Standards

- All public API items MUST have doc comments with a summary line and
  parameter descriptions.
- `cargo doc --no-deps` MUST build without warnings.
- Module-level documentation MUST describe the module's role within
  the component.

**Rationale**: Well-documented APIs reduce onboarding time and prevent
misuse.

### VII. Maintainability and Graceful Degradation

- Follow YAGNI: do not add features or abstractions beyond what the
  current requirements demand.
- Prefer simple, direct implementations over premature abstractions.
- Error handling MUST be explicit: use `Result` types; do not panic in
  library code except for unrecoverable invariant violations.
- The P2P path MUST fall back gracefully to the DRAM path when
  GDRCopy or BAR1 is unavailable. No P2P-related error may crash the
  component or leave it in an unusable state.

**Rationale**: Maintainability sustains velocity over time. Graceful
fallback ensures the component is usable without P2P hardware.

## Platform and Tooling Requirements

- **Target OS**: Linux only.
- **Language**: Rust stable, edition 2021, MSRV 1.75.
- **Build**: `cargo build -p dispatcher-p2p`.
- **Test**: `cargo test -p dispatcher-p2p`.
- **Lint**: `cargo clippy -p dispatcher-p2p -- -D warnings`.
- **Format**: `cargo fmt -p dispatcher-p2p --check`.
- **Docs**: `cargo doc -p dispatcher-p2p --no-deps`.
- **Benchmarks**: `cargo bench -p dispatcher-p2p` (Criterion-based).

## Development Workflow

- All changes MUST pass the full quality gate before merge:
  `fmt` check, `clippy` lint, `doc` build, and all tests.
- Commits SHOULD be atomic and focused on a single logical change.
- All new public API surface MUST include doc tests and unit tests
  in the same commit that introduces the API.

## Governance

This constitution is the authoritative reference for all development
practices within the dispatcher-p2p component.

- **Amendments**: Any change MUST be documented with a version bump
  and rationale.
- **Compliance Review**: All code reviews MUST verify compliance with
  these principles.
- **Conflict Resolution**: If a principle conflicts with a practical
  constraint, the conflict MUST be documented and justified before
  an exception is granted.

**Version**: 1.0.0 | **Ratified**: 2026-06-11 | **Last Amended**: 2026-06-11
