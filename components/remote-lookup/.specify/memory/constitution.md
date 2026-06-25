<!--
Sync Impact Report
==================
Version change: 0.0.0 → 1.0.0 (initial ratification)
Modified principles: N/A (new document)
Added sections:
  - Core Principles (7 principles)
  - Platform & Technology Constraints
  - Development Workflow
  - Governance
Removed sections: N/A
Templates requiring updates:
  - .specify/templates/plan-template.md ✅ (Constitution Check section compatible)
  - .specify/templates/spec-template.md ✅ (Requirements section compatible)
  - .specify/templates/tasks-template.md ✅ (Phase structure compatible)
Follow-up TODOs: None
-->

# Remote Lookup Component Constitution

## Core Principles

### I. Interface-Only Exposure

All public functionality MUST be exposed exclusively through interfaces
defined in the `components/interfaces` crate. No public functions, structs,
or methods outside of interface trait implementations are permitted at the
crate boundary. The component MUST conform to the `components/component-framework`
methodology using `define_component!` and `define_interface!` macros.

**Rationale**: Enforces low coupling between components. Consumers depend only
on trait contracts, enabling independent evolution and testability.

### II. Comprehensive Unit Testing

Every public API method exposed through an interface MUST have unit tests
that verify correctness under normal conditions, boundary conditions, and
error conditions. Tests MUST be co-located in the same module using `#[cfg(test)]`
or in a dedicated `tests/` directory. All tests MUST pass with
`cargo test -p remote-lookup`.

**Rationale**: Unit tests are the primary mechanism for asserting behavioral
correctness. Untested APIs are considered unfinished.

### III. Documentation Tests

All public API methods MUST include Rust doc comments (`///`) with runnable
examples using ```` ```rust ```` fenced code blocks. These doc tests MUST
compile and pass under `cargo test --doc -p remote-lookup`. Documentation
MUST be warning-free under `cargo doc --no-deps`.

**Rationale**: Doc tests serve dual purpose — they document usage for consumers
and act as regression tests that stay synchronized with the implementation.

### IV. Performance Testing

All performance-sensitive code paths MUST have Criterion-based benchmarks in
a `benches/` directory. Benchmarks MUST be runnable via `cargo bench -p remote-lookup`.
Performance requirements MUST be explicitly stated and measurable. Regressions
detected by benchmarks MUST be investigated before merging.

**Rationale**: Performance claims without measurement are speculation. Criterion
provides statistical rigor for detecting regressions across runs.

### V. Code Correctness Assurance

Code MUST pass `cargo clippy -- -D warnings` with zero warnings. All unsafe
code MUST include a `// SAFETY:` justification comment. Panic-free invariants
MUST be maintained in public APIs — functions MUST return `Result` types for
fallible operations rather than panicking. Logic correctness MUST be
demonstrable through tests, not assumed.

**Rationale**: Clippy catches common mistakes statically. Explicit error handling
and safety documentation make correctness auditable.

### VI. Maintainability

Code MUST use `rustfmt` default formatting. Functions MUST be focused and
short enough to reason about in isolation. Abstractions MUST earn their
existence — no speculative generality. Dependencies MUST be minimal and
justified. The component MUST build and test independently from other
components (aside from `component-framework` and `interfaces`).

**Rationale**: Maintainability degrades when code is hard to read, hard to
change, or tightly coupled to unrelated concerns.

### VII. Linux Platform Commitment

All code MUST compile and run on Linux (RHEL/Fedora). Platform-specific
system calls or kernel features MUST be documented. No Windows or macOS
compatibility is required or maintained. The minimum supported Rust version
is 1.75 (edition 2021).

**Rationale**: The Certus system targets Linux-only deployments. Maintaining
cross-platform compatibility for unused targets adds cost without value.

## Platform & Technology Constraints

- **Language**: Rust stable, edition 2021, MSRV 1.75
- **OS**: Linux only (RHEL 9 / Fedora primary targets)
- **Framework**: `components/component-framework` (`define_component!`,
  `define_interface!`, `IUnknown` for runtime interface discovery)
- **Interface location**: All interface trait definitions MUST reside in
  `components/interfaces/src/`
- **Testing**: `cargo test` for unit/doc tests, Criterion for benchmarks
- **Linting**: `cargo fmt --check` and `cargo clippy -- -D warnings`
- **Documentation**: `cargo doc --no-deps` MUST be warning-free

## Development Workflow

1. **Interface First**: Define or extend the interface trait in
   `components/interfaces` before implementing in this component.
2. **Test First**: Write failing tests for new functionality before
   implementation. Doc tests count toward this requirement.
3. **Benchmark Early**: If a code path has performance requirements,
   add the Criterion benchmark before or alongside implementation.
4. **Lint Before Commit**: Run `cargo fmt` and `cargo clippy -- -D warnings`
   before every commit. CI will reject non-conforming code.
5. **Review Gate**: All changes MUST pass `cargo test -p remote-lookup`,
   `cargo clippy -- -D warnings`, and `cargo doc --no-deps` before merge.

## Governance

This constitution is the authoritative standard for the `remote-lookup`
component. All code reviews MUST verify compliance with these principles.
Non-compliance MUST be resolved before merge.

**Amendment procedure**: Changes to this constitution require documented
justification, version increment, and updated `LAST_AMENDED_DATE`. MAJOR
version for principle removal/redefinition, MINOR for additions, PATCH for
clarifications.

**Compliance review**: Every pull request touching this component MUST be
checked against each principle. Violations MUST be flagged as blocking.

**Version**: 1.0.0 | **Ratified**: 2026-06-19 | **Last Amended**: 2026-06-19
