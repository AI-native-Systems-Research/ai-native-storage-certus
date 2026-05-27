<!--
Sync Impact Report
===================
Version change: 0.0.0 → 1.0.0 (initial ratification)
Modified principles: N/A (initial version)
Added sections:
  - I. Component-Framework Conformance
  - II. Interface-Only Exposure
  - III. Code Quality and Correctness
  - IV. Comprehensive Testing
  - V. Performance Validation
  - VI. Documentation Standards
  - VII. Maintainability and Engineering Practice
  - Platform and Tooling Requirements
  - Development Workflow
  - Governance
Templates requiring updates:
  - plan-template.md — ✅ reviewed, Constitution Check section aligns
  - spec-template.md — ✅ reviewed, requirements section aligns
  - tasks-template.md — ✅ reviewed, test-first phasing aligns
Follow-up TODOs: none
-->

# DispatchMap v0 Constitution

## Core Principles

### I. Component-Framework Conformance

All code in this component MUST conform to the `components/component-framework`
methodology. Components MUST use `define_component!` and `define_interface!`
macros. Every component MUST implement `IUnknown` for runtime interface
discovery. Dependencies MUST be declared as typed receptacles and wired via
`bind()`. Actor threading and lock-free channel patterns defined by the
framework MUST be followed where applicable.

**Rationale**: The component framework enforces loose coupling, independent
development, and runtime composability. Deviating from it breaks integration
with the rest of the Certus system.

### II. Interface-Only Exposure

The component MUST only expose functionality through interfaces defined in the
`components/interfaces` crate. Public functions outside the component boundary
are NOT allowed. All interface trait definitions (`IDispatchMap`, `ILogger`,
`IExtentManager`, etc.) MUST reside in `components/interfaces`. No struct,
function, or method may be made `pub` if it is not part of an interface
definition or required by the component-framework macros for internal wiring.

**Rationale**: Interface-only exposure guarantees that consumers depend on
contracts, not implementations. This enables independent component evolution
and substitution without breaking callers.

### III. Code Quality and Correctness

- All code MUST compile without warnings under `cargo clippy -- -D warnings`.
- All code MUST pass `cargo fmt --check` with default `rustfmt` settings.
- `cargo doc --no-deps` MUST produce zero warnings.
- All `unsafe` code MUST include a `// SAFETY:` justification comment.
- Assurance of code correctness is of the highest importance. All logic
  MUST be verified through tests (see Principle IV). Edge cases, error
  paths, and boundary conditions MUST be explicitly tested.
- All code MUST target and run on the Linux operating system exclusively.

**Rationale**: Strict lint and format enforcement catches defects early.
Correctness assurance through testing prevents regressions and builds
confidence in component reliability.

### IV. Comprehensive Testing

- All public APIs MUST have unit tests validating correctness.
- All public APIs MUST have Rust documentation tests (`///` doc examples)
  that compile and run as tests via `cargo test`.
- Integration tests MUST verify component wiring through receptacles and
  interface interactions (`ILogger`, `IExtentManager` bindings).
- Tests MUST cover: happy paths, error paths, boundary conditions, and
  concurrent access patterns where applicable.
- Test execution MUST be deterministic and MUST NOT depend on external
  hardware (use mocks for SPDK-dependent paths where hardware is absent).
- All tests MUST pass under single-threaded execution
  (`--test-threads 1`) for CI compatibility.

**Rationale**: Comprehensive testing at every level (unit, doc, integration)
is the primary mechanism for assuring code correctness. Doc tests serve
double duty as living documentation and regression guards.

### V. Performance Validation

- All performance-sensitive code MUST have Criterion-based benchmarks.
- Benchmarks MUST be available under `cargo bench` or targeted via
  `cargo bench --bench <name>`.
- Public APIs that are on critical data paths MUST include performance
  unit tests that assert latency or throughput expectations where
  measurable thresholds can be defined.
- Performance regressions MUST be detectable by comparing Criterion
  benchmark results across commits.

**Rationale**: Storage systems have strict performance requirements.
Criterion benchmarks provide statistically rigorous measurement and
regression detection that ad-hoc timing cannot.

### VI. Documentation Standards

- All public API items (traits, structs, functions, methods) MUST have
  doc comments (`///`) with:
  - A summary line describing purpose.
  - Parameter and return value descriptions where non-obvious.
  - A runnable `# Examples` section that serves as a doc test.
- `cargo doc --no-deps` MUST build without warnings.
- Module-level documentation (`//!`) MUST describe the module's role
  within the component.

**Rationale**: Well-documented APIs reduce onboarding time, prevent
misuse, and provide executable examples that double as correctness tests.

### VII. Maintainability and Engineering Practice

- Follow YAGNI: do not add features, abstractions, or code paths beyond
  what the current requirements demand.
- Prefer simple, direct implementations over premature abstractions.
  Three similar lines are better than a premature helper.
- Error handling MUST be explicit: use `Result` types; do not panic in
  library code except for unrecoverable invariant violations.
- Dependencies MUST be minimized. Each new dependency MUST be justified
  by a clear need that cannot be met by existing dependencies or
  reasonable inline code.
- Code MUST be structured for readability: well-named identifiers,
  short functions with single responsibilities, minimal nesting.

**Rationale**: Maintainability sustains velocity over time. Simple code
is easier to review, test, debug, and evolve. Minimal dependencies
reduce supply-chain risk and build complexity.

## Platform and Tooling Requirements

- **Target OS**: Linux only (tested on RHEL/Fedora).
- **Language**: Rust stable, edition 2021, MSRV 1.75.
- **Build**: `cargo build -p dispatch-map` (SPDK workspace member,
  not a default member).
- **Test**: `cargo test -p dispatch-map` (all tests, including doc tests).
- **Lint**: `cargo clippy -p dispatch-map -- -D warnings`.
- **Format**: `cargo fmt -p dispatch-map --check`.
- **Docs**: `cargo doc -p dispatch-map --no-deps`.
- **Benchmarks**: `cargo bench -p dispatch-map` (Criterion-based).
- SPDK dependencies are required at build time via the `interfaces`
  crate with `features = ["spdk"]`.

## Development Workflow

- All changes MUST pass the full quality gate before merge:
  `fmt` check, `clippy` lint, `doc` build, and all tests (unit,
  doc, integration).
- Commits SHOULD be atomic and focused on a single logical change.
- Performance-sensitive changes MUST include before/after Criterion
  benchmark results.
- All new public API surface MUST include doc tests and unit tests
  in the same commit that introduces the API.
- Code review MUST verify conformance with this constitution.

## Governance

This constitution is the authoritative reference for all development
practices within the DispatchMap v0 component. It supersedes informal
conventions and ad-hoc decisions.

- **Amendments**: Any change to this constitution MUST be documented
  with a version bump, a rationale, and a review of dependent
  artifacts (templates, specs, plans) for consistency.
- **Versioning**: This constitution follows semantic versioning:
  - MAJOR: Principle removal or backward-incompatible redefinition.
  - MINOR: New principle or materially expanded guidance.
  - PATCH: Clarifications, wording fixes, non-semantic refinements.
- **Compliance Review**: All pull requests and code reviews MUST
  verify compliance with these principles. Non-compliance MUST be
  resolved before merge.
- **Conflict Resolution**: If a principle conflicts with a practical
  constraint, the conflict MUST be documented, justified, and
  approved before an exception is granted.

**Version**: 1.0.0 | **Ratified**: 2026-04-27 | **Last Amended**: 2026-04-27
