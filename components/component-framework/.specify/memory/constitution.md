<!--
Sync Impact Report
===================
Version change: 0.0.0 → 1.0.0 (MAJOR - initial ratification)
Modified principles: N/A (initial version)
Added sections:
  - Principle I: Code Correctness First
  - Principle II: Comprehensive Testing
  - Principle III: Performance Accountability
  - Principle IV: Documentation as Contract
  - Principle V: Maintainability and Simplicity
  - Section: Platform and Toolchain Constraints
  - Section: Development Workflow
  - Section: Governance
Templates requiring updates:
  - .specify/templates/plan-template.md ✅ (aligned - Constitution Check section exists)
  - .specify/templates/spec-template.md ✅ (aligned - testing and requirements sections exist)
  - .specify/templates/tasks-template.md ✅ (aligned - test-first phasing and checkpoints exist)
Follow-up TODOs: None
-->

# Component Framework Constitution

## Core Principles

### I. Code Correctness First

All code MUST be demonstrably correct before it is considered complete.
Correctness is the highest-priority non-functional requirement and MUST NOT
be traded away for performance, features, or schedule.

- Every public API MUST have unit tests that verify correctness across
  normal inputs, boundary conditions, and error paths.
- All Rust public APIs MUST include documentation tests (`///` examples
  with assertions) that serve as both usage documentation and correctness
  checks. `cargo test --doc` MUST pass with zero failures.
- Unsafe code MUST be justified in a comment, minimized in scope, and
  accompanied by tests that exercise the safety invariants.
- All warnings from `cargo clippy` MUST be resolved before merge.

### II. Comprehensive Testing

Testing is mandatory and MUST cover correctness, contracts, and integration.

- Unit tests MUST exist for every public function, method, and trait
  implementation. Tests MUST verify expected outputs, error conditions,
  and edge cases.
- Integration tests MUST verify cross-module and cross-crate interactions.
- Test coverage MUST be maintained or increased with every change.
- The test suite MUST pass (`cargo test --all`) with zero failures before
  any code is merged.
- Test-Driven Development (TDD) is the preferred workflow: write a failing
  test, implement the minimum code to pass, then refactor.

### III. Performance Accountability

Performance-sensitive code MUST be measured, not assumed.

- All performance-sensitive public APIs MUST have Criterion benchmarks
  (`cargo bench`). Benchmarks MUST be kept in `benches/` and registered
  in `Cargo.toml`.
- Performance requirements MUST be stated as quantitative targets (e.g.,
  throughput, latency percentiles, memory budget) in the feature spec.
- Criterion benchmarks MUST be run before and after changes to
  performance-sensitive code; regressions MUST be investigated and
  justified or resolved before merge.
- Allocation-heavy hot paths SHOULD be profiled; results SHOULD be
  referenced in PR descriptions when performance claims are made.

### IV. Documentation as Contract

Public API documentation is a deliverable, not an afterthought.

- Every public type, function, method, and module MUST have a Rust doc
  comment (`///` or `//!`) that describes purpose, parameters, return
  value, errors, and panics (if any).
- Doc comments MUST include at least one runnable example (doc test) that
  demonstrates correct usage.
- `cargo doc --no-deps` MUST complete with zero warnings.
- README and crate-level docs (`//!` in `lib.rs`) MUST provide a
  quick-start guide sufficient for a new contributor to build and run
  the test suite.

### V. Maintainability and Simplicity

Code MUST be written for the next reader, not just the current author.

- Prefer explicit, readable code over clever abstractions. Introduce
  abstractions only when duplication or complexity justifies them.
- Public API surface MUST be minimal: expose only what consumers need.
- Dependencies MUST be justified; prefer well-maintained crates with
  compatible licenses.
- Code MUST be formatted with `cargo fmt` (default rustfmt config)
  and MUST pass `cargo clippy` without warnings.
- Modules and crates MUST have a single, clear responsibility.

## Platform and Toolchain Constraints

- **Target OS**: Linux (all code MUST build and run on Linux; other
  platforms are not required).
- **Language**: Rust (stable toolchain; nightly features MUST NOT be
  required for building or testing).
- **Testing framework**: `cargo test` for unit/integration/doc tests;
  Criterion for benchmarks.
- **CI gate**: `cargo fmt --check && cargo clippy -- -D warnings &&
  cargo test --all && cargo doc --no-deps` MUST all pass.

## Development Workflow

- Feature branches MUST be used for all changes; direct commits to the
  main branch are not permitted.
- Every pull request MUST pass the CI gate before merge.
- Code review is required; reviewers MUST verify that new public APIs
  include doc tests and unit tests.
- Commits MUST be atomic and describe the "why" of the change.

## Governance

- This constitution supersedes ad-hoc conventions. When a conflict arises
  between this document and other guidance, this document wins.
- Amendments require: (1) a written proposal, (2) review by at least one
  maintainer, and (3) a version bump following semver rules (MAJOR for
  principle removal/redefinition, MINOR for additions, PATCH for
  clarifications).
- All PRs and reviews MUST verify compliance with the principles above.

**Version**: 1.0.0 | **Ratified**: 2026-03-30 | **Last Amended**: 2026-03-30
