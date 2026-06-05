<!--
Sync Impact Report
==================
Version change: N/A → 1.0.0 (initial ratification)
Modified principles: N/A (new document)
Added sections:
  - Core Principles (7 principles)
  - Platform & Technical Constraints
  - Development Workflow & Quality Gates
  - Governance
Removed sections: N/A
Templates requiring updates:
  - .specify/templates/plan-template.md ✅ compatible (Constitution Check section)
  - .specify/templates/spec-template.md ✅ compatible (requirements section)
  - .specify/templates/tasks-template.md ✅ compatible (test-first pattern)
Follow-up TODOs: None
-->

# Block Device Filesys Constitution

## Core Principles

### I. Component Architecture Conformance

- Component MUST conform to the `components/component-framework` methodology,
  using `define_component!` and `define_interface!` macros.
- Component MUST implement `IUnknown` for runtime interface discovery.
- Component MUST only expose functionality through typed interfaces.
- Public functions outside the component interface boundary are NOT allowed.
- All interface trait definitions MUST be defined in the
  `components/interfaces` crate.
- Dependencies MUST be declared as receptacles and wired via binding.

**Rationale**: Enforces low coupling, independent development, and typed
integration boundaries consistent with the Certus component model.

### II. Code Quality & Maintainability

- All code MUST pass `cargo fmt --check` with default formatting rules.
- All code MUST pass `cargo clippy -- -D warnings` with zero warnings.
- Unsafe code MUST include a `// SAFETY:` justification comment.
- Code MUST follow idiomatic Rust patterns and established good
  engineering practices.
- Abstractions MUST be justified by concrete requirements, not hypothetical
  future needs.
- Internal module structure MUST be clear and navigable without requiring
  external documentation.

**Rationale**: Maintainability depends on consistent style, static analysis
enforcement, and disciplined use of unsafe code.

### III. Comprehensive Testing

- All public API functions MUST have unit tests verifying correctness.
- All public API functions MUST have Rust documentation tests (`///` examples
  that compile and run via `cargo test`).
- Tests MUST cover both the happy path and edge cases (boundary conditions,
  error paths, resource exhaustion).
- Tests MUST run with `cargo test` without requiring special hardware or
  external services (use mocks where necessary).
- Test failures MUST block integration; no known-failing tests are permitted
  in the main branch.

**Rationale**: Correctness assurance requires that every public contract is
exercised by automated tests that run in CI without special infrastructure.

### IV. Performance Validation

- All performance-sensitive code MUST have Criterion-based benchmarks.
- All public API functions that are performance-sensitive MUST have
  associated performance tests measuring throughput and/or latency.
- Performance benchmarks MUST be runnable via `cargo bench`.
- Performance regressions detected by benchmarks MUST be investigated
  before merging.
- Benchmark results SHOULD be comparable across runs (deterministic
  workloads, controlled allocations).

**Rationale**: Storage and filesystem components have strict performance
requirements; regression detection requires repeatable, quantitative
measurement.

### V. Documentation

- All public API items (functions, structs, traits, enums, modules) MUST
  have doc comments (`///` or `//!`).
- Doc comments MUST include at least one runnable example demonstrating
  correct usage.
- `cargo doc --no-deps` MUST produce zero warnings.
- Documentation MUST describe preconditions, postconditions, and panic
  conditions where applicable.

**Rationale**: Well-documented APIs reduce integration errors and enable
independent component development without reading implementation details.

### VI. Correctness Assurance

- Code correctness is of the HIGHEST importance.
- All arithmetic operations on sizes, offsets, and indices MUST use
  checked or saturating arithmetic, or document why overflow is impossible.
- All resource acquisitions (memory, file descriptors, locks) MUST have
  corresponding release paths, including on error.
- All public API invariants MUST be enforced at the interface boundary
  (validate inputs, return typed errors).
- Where practical, design-level invariants SHOULD be encoded in the type
  system rather than enforced at runtime.

**Rationale**: A block device filesystem component operates on persistent
data; correctness failures can cause data loss or corruption.

### VII. Platform Constraints

- All code MUST compile and run on the Linux operating system.
- Platform-specific APIs (syscalls, io_uring, AIO) are permitted but
  MUST NOT have fallback paths to other operating systems.
- MSRV (Minimum Supported Rust Version) is 1.75, edition 2021.
- Target architecture is x86_64; ARM64 is permitted but not required.

**Rationale**: Certus targets Linux-only inferencing workloads; cross-platform
abstractions add complexity without delivering value.

## Platform & Technical Constraints

- **Language**: Rust stable, edition 2021, MSRV 1.75.
- **OS**: Linux only (RHEL/Fedora tested).
- **Build**: Must integrate with the workspace `cargo build` system.
- **Dependencies**: External crate dependencies MUST be justified and
  version-pinned in `Cargo.toml`.
- **FFI**: Any C/C++ interop MUST use bindgen-generated bindings with
  safe Rust wrappers.
- **Concurrency**: Thread safety MUST be achieved through Rust's ownership
  model; raw synchronization primitives require justification.
- **Error Handling**: Functions MUST return `Result` types for fallible
  operations; panics are prohibited in library code except for
  unrecoverable invariant violations.

## Development Workflow & Quality Gates

All code changes MUST pass the following gates before integration:

1. **Format gate**: `cargo fmt --check` passes.
2. **Lint gate**: `cargo clippy -- -D warnings` passes.
3. **Test gate**: `cargo test` passes (single-threaded in CI).
4. **Doc gate**: `cargo doc --no-deps` produces zero warnings.
5. **Bench gate**: `cargo bench` compiles without errors; performance
   regressions are flagged for review.

Changes to interface definitions in `components/interfaces` MUST be
coordinated with dependent components.

New public API additions MUST include:
- Unit tests for correctness
- Documentation with runnable examples
- Criterion benchmarks (if performance-sensitive)

## Governance

- This constitution supersedes informal practices and ad-hoc conventions
  for the `block-device-filesys` component.
- Amendments require: (1) written justification, (2) review by the
  component maintainer, and (3) version bump per semantic versioning.
- Version policy:
  - MAJOR: principle removal or incompatible redefinition.
  - MINOR: new principle or materially expanded guidance.
  - PATCH: wording clarification or typo fix.
- Compliance review: all code changes MUST be verified against these
  principles before merging.

**Version**: 1.0.0 | **Ratified**: 2026-06-04 | **Last Amended**: 2026-06-04
