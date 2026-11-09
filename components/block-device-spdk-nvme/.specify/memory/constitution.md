<!--
  Sync Impact Report
  ====================
  Version change: (new) → 1.0.0
  Modified principles: N/A (initial creation)
  Added sections:
    - Core Principles (6 principles)
    - Platform and Toolchain Constraints
    - Quality Gates
    - Governance
  Removed sections: N/A
  Templates requiring updates:
    - .specify/templates/plan-template.md ✅ reviewed (Constitution Check aligns)
    - .specify/templates/spec-template.md ✅ reviewed (no changes needed)
    - .specify/templates/tasks-template.md ✅ reviewed (no changes needed)
  Follow-up TODOs: None
-->

# block-device-spdk-nvme Constitution

## Core Principles

### I. Correctness First

All code MUST be demonstrably correct. Every public API MUST have unit
tests that verify correctness under normal, boundary, and error
conditions. Unsafe code MUST include a `// SAFETY:` comment justifying
its use and MUST be covered by tests that exercise the unsafe boundary.
Undefined behavior is never acceptable.

### II. Comprehensive Testing

- Every public function, method, and type MUST have unit tests.
- Every public Rust API MUST have documentation tests (`///` examples)
  that compile and pass under `cargo test --doc`.
- Integration tests MUST exist for cross-component interactions.
- TDD (red-green-refactor) is the preferred development workflow.
- `cargo test --all` MUST pass with zero failures at all times.

### III. Performance Accountability

- All performance-sensitive code MUST have Criterion benchmarks in
  `benches/`.
- Benchmarks MUST cover latency and throughput at varying queue depths.
- Performance regressions MUST be justified with rationale or fixed
  before merge.
- `cargo bench` MUST compile and run without errors.

### IV. Documentation as Contract

- Every public type, function, method, and module MUST have doc
  comments with runnable `///` examples.
- `cargo doc --no-deps` MUST complete with zero warnings.
- Documentation describes the contract (preconditions, postconditions,
  error semantics), not the implementation.

### V. Maintainability

- Code MUST pass `cargo fmt --check` and `cargo clippy -- -D warnings`
  with zero violations.
- Public API surface MUST be minimal; prefer private internals exposed
  through narrow, well-tested interfaces.
- Modules MUST follow single-responsibility: one concern per module.
- Dependencies MUST be justified; avoid unnecessary external crates.

### VI. Component-Framework Conformance

- All components MUST conform to the `components/component-framework`
  methodology for defining interfaces, receptacles, and bindings.
- Component MUST use the actor model with dedicated service threads.
- Actor component MUST use shared-memory channels for inter-component messaging
  (ingress commands, asynchronous completion callbacks).
- Component MUST declare dependencies through receptacles (e.g.,
  `ILogger`) rather than hard-coding them.
- Component should provide `IUnknown`
- Component should only expose functions through interfaces.  Public functions outside the component are not allowed.
- Make sure tests and benchmarks run with or without SPDK hardware. If no hardware is present the tests pass but do nothing.


## Platform and Toolchain Constraints

- **Operating System**: Linux only. No Windows or macOS portability is
  required or expected.
- **Language**: Rust, stable toolchain (edition 2021, no nightly
  features).
- **MSRV**: 1.75 or later.
- **Build System**: Cargo. The component participates in the parent
  workspace at `../../Cargo.toml`.
- **External Dependencies**: SPDK (Storage Performance Development
  Kit) for direct NVMe controller access, initialized via the
  `spdk-env` sibling component.

## Quality Gates

All of the following MUST pass before code is merged:

```
cargo fmt --check
cargo clippy -- -D warnings
cargo test --all
cargo doc --no-deps
cargo bench --no-run
```

## Governance

- This constitution supersedes all other development practices for
  this component.
- All code reviews MUST verify compliance with these principles.
- Amendments require: (1) documented rationale, (2) review approval,
  (3) version bump per semantic versioning (MAJOR for principle
  removal/redefinition, MINOR for additions, PATCH for clarifications).
- Complexity beyond what a principle permits MUST be justified in the
  implementation plan with a Complexity Tracking entry.

**Version**: 1.0.0 | **Ratified**: 2026-04-14 | **Last Amended**: 2026-04-14
