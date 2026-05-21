<!--
Sync Impact Report
===================
Version change: 0.0.0 (template) → 1.0.0
Modified principles: N/A (initial constitution)
Added sections:
  - Principle I: Component-Framework Conformance
  - Principle II: Interface-Only Public API
  - Principle III: Comprehensive Testing (NON-NEGOTIABLE)
  - Principle IV: Performance Assurance
  - Principle V: Documentation & Maintainability
  - Principle VI: Code Quality & Correctness
  - Principle VII: Linux-Only Platform
  - Section: Platform & Technology Constraints
  - Section: Development Workflow
  - Section: Governance
Removed sections: N/A (initial constitution)
Templates requiring updates:
  - plan-template.md: ✅ Constitution Check section aligns with principles
  - spec-template.md: ✅ Requirements/success criteria align with principles
  - tasks-template.md: ✅ Phase structure supports testing-first workflow
Follow-up TODOs: None
-->

# Dispatcher v0 Constitution

## Core Principles

### I. Component-Framework Conformance

All code MUST conform to the `components/component-framework` methodology:
- Components MUST use `define_component!` and `define_interface!` macros
- Every component MUST implement `IUnknown` for runtime interface discovery
- Dependencies MUST be declared as typed receptacles and wired via
  `bind()` by name
- Actor-based components MUST run on dedicated OS threads with lock-free
  channel communication
- Component lifecycle MUST follow: `new_default()` → bind receptacles →
  `initialize()` → use interface methods → `shutdown()`

### II. Interface-Only Public API

Components MUST NOT expose public functions outside the component boundary:
- All functionality MUST be exposed exclusively through interfaces
- All interfaces MUST be defined in the `components/interfaces` crate
- No public functions, structs, or types outside the component are
  permitted unless they are part of a defined interface
- Internal implementation details MUST remain private to the component

### III. Comprehensive Testing (NON-NEGOTIABLE)

All public APIs MUST have unit tests for correctness:
- Every public interface method MUST have at least one unit test
  verifying correct behavior
- Rust documentation tests (`///` examples) MUST exist for all public
  API items — `cargo test --doc` MUST pass
- Integration tests MUST verify component wiring and receptacle binding
- Edge cases, error paths, and boundary conditions MUST be tested
- `cargo test -p dispatcher` MUST pass with zero failures before any
  merge

### IV. Performance Assurance

All performance-sensitive code MUST have Criterion-based benchmarks:
- Benchmark suites MUST be available under `benches/` for every
  performance-critical path
- Benchmarks MUST use the Criterion framework (`cargo bench`)
- Performance regressions MUST be detectable via benchmark comparison
- Public API methods that are on the hot path MUST have associated
  performance tests documenting expected throughput or latency
  characteristics

### V. Documentation & Maintainability

All public APIs MUST be well documented:
- Public APIs MUST have doc comments with runnable examples
- `cargo doc -p dispatcher --no-deps` MUST complete with zero warnings
- Code MUST be structured for readability and long-term maintainability
- Unsafe code MUST include `// SAFETY:` justification comments
- Module-level documentation MUST describe purpose and usage patterns

### VI. Code Quality & Correctness

Assurance of code correctness is of high importance:
- `cargo fmt -p dispatcher --check` MUST pass (rustfmt default formatting)
- `cargo clippy -p dispatcher -- -D warnings` MUST pass (warnings are
  errors)
- All code MUST compile without warnings
- Error handling MUST use idiomatic Rust `Result`/`Option` types
- Unsafe blocks MUST be minimized and each MUST have a safety
  justification
- Code MUST follow established Rust idioms and best practices

### VII. Linux-Only Platform

All code MUST run on the Linux operating system:
- No platform-conditional compilation for non-Linux targets is permitted
- System-level dependencies (SPDK, hugepages, IOMMU) MUST target Linux
  kernel interfaces
- Testing and CI MUST execute on Linux environments
- No Windows or macOS compatibility requirements exist

## Platform & Technology Constraints

- **Language**: Rust stable, edition 2021, MSRV 1.75
- **Target OS**: Linux only (RHEL/Fedora tested)
- **Build system**: Cargo workspace member (not a default member due to
  SPDK dependency)
- **Dependencies**: `component-framework`, `component-core`,
  `component-macros` for the component model; `interfaces` crate for
  all interface definitions
- **SPDK**: Required at runtime for `IBlockDeviceAdmin` and
  `IDispatchMap` receptacles
- **Testing framework**: Built-in `#[test]` for unit/integration,
  Criterion for benchmarks
- **Documentation**: `rustdoc` with runnable examples

## Development Workflow

- All changes MUST pass the full quality gate before merge:
  `cargo fmt --check` → `cargo clippy -D warnings` → `cargo test` →
  `cargo doc --no-deps`
- Benchmarks MUST be run and compared before and after
  performance-sensitive changes
- New public API additions MUST include doc tests, unit tests, and
  (where applicable) Criterion benchmarks in the same changeset
- Component changes MUST verify receptacle binding and lifecycle
  correctness via integration tests

## Governance

This constitution is the authoritative source of engineering principles
for the Dispatcher v0 component. All code reviews and pull requests
MUST verify compliance with these principles.

**Amendment procedure**:
1. Proposed amendments MUST be documented with rationale
2. Amendments MUST include a migration plan for existing code if
   the change affects current implementations
3. Version MUST be incremented per semantic versioning:
   MAJOR for principle removals/redefinitions, MINOR for additions,
   PATCH for clarifications

**Compliance review**:
- Every PR MUST be checked against the Constitution Check gates in
  the plan template
- Complexity beyond what these principles allow MUST be explicitly
  justified

**Version**: 1.0.0 | **Ratified**: 2026-04-28 | **Last Amended**: 2026-04-28
