<!--
  Sync Impact Report
  ===========================================================================
  Version change: 0.0.0 (template) -> 1.0.0
  Modified principles: N/A (initial constitution)
  Added sections:
    - Principle I: Linux-First Systems Code
    - Principle II: Comprehensive Testing (NON-NEGOTIABLE)
    - Principle III: Performance Verification
    - Principle IV: Documentation as Code
    - Principle V: Code Correctness Assurance
    - Principle VI: Component Architecture
    - Principle VII: Maintainability and Simplicity
    - Section: Platform and Toolchain Requirements
    - Section: Development Workflow and Quality Gates
    - Governance section
  Removed sections: None
  Templates requiring updates:
    - .specify/templates/plan-template.md — ✅ No updates needed (generic)
    - .specify/templates/spec-template.md — ✅ No updates needed (generic)
    - .specify/templates/tasks-template.md — ✅ No updates needed (generic)
  Follow-up TODOs: None
  ===========================================================================
-->

# Certus spdk-env Constitution

## Core Principles

### I. Linux-First Systems Code

All code MUST target the Linux operating system exclusively. Platform-specific
APIs (epoll, io_uring, hugepages, NUMA) are permitted and encouraged where
they improve performance. No cross-platform abstraction layers are required
unless a specific use case demands it.

- All builds MUST succeed on Linux (x86_64, RHEL/Fedora target).
- CI pipelines MUST run on Linux environments only.
- System-level dependencies (SPDK, DPDK, kernel interfaces) MUST be
  explicitly documented with version constraints.

### II. Comprehensive Testing (NON-NEGOTIABLE)

Every public API MUST have unit tests that verify correctness. Testing is a
mandatory gate — code without adequate test coverage MUST NOT be merged.

- All public functions and methods MUST have corresponding unit tests.
- Rust doc-tests (`///` examples) MUST exist for every public API item
  (functions, structs, traits, methods, type aliases with constructors).
- Doc-tests MUST be compilable and runnable, not just illustrative.
- Integration tests MUST cover cross-component interactions and SPDK
  bindings.
- Tests MUST exercise error paths, boundary conditions, and edge cases.
- `cargo test --all` MUST pass with zero failures before any merge.

### III. Performance Verification

All performance-sensitive code MUST have Criterion-based benchmarks that
establish baselines and detect regressions.

- Performance benchmarks MUST use the Criterion framework (`criterion`
  crate) exclusively.
- Every performance-sensitive public API MUST have an associated Criterion
  benchmark.
- Benchmarks MUST run in CI and results MUST be compared against
  established baselines.
- Performance regressions exceeding documented thresholds MUST block
  merges until resolved or explicitly justified.
- Latency-sensitive paths (I/O submission, completion polling, buffer
  management) MUST have micro-benchmarks.
- Benchmark results MUST be reproducible (fixed iteration counts, warm-up
  periods, statistical significance via Criterion defaults).

### IV. Documentation as Code

Public APIs MUST be fully documented. Documentation is tested and enforced
at build time.

- `#![deny(missing_docs)]` MUST be enabled in all library crates.
- Every public item MUST have a doc comment explaining purpose, parameters,
  return values, errors, and panics (where applicable).
- Doc comments MUST include at least one runnable example (doc-test).
- `cargo doc --no-deps` MUST complete without warnings.
- Unsafe code blocks MUST have a `// SAFETY:` comment explaining the
  invariants that make the usage sound.

### V. Code Correctness Assurance

Correctness is paramount. Multiple layers of verification MUST be applied
to ensure code behaves as specified.

- `#![deny(unsafe_op_in_unsafe_fn)]` MUST be enabled to enforce explicit
  unsafe blocks within unsafe functions.
- `cargo clippy -- -D warnings` MUST pass with no warnings.
- All `unsafe` code MUST have a corresponding safety proof or invariant
  documented inline.
- Miri (`cargo +nightly miri test`) SHOULD be run on non-FFI code paths
  to detect undefined behavior.
- Fuzzing (cargo-fuzz or libFuzzer) SHOULD be applied to parsing and
  serialization code.
- All public API contracts (preconditions, postconditions, invariants)
  MUST be documented and tested.

### VI. Component Architecture

Components MUST conform to the Certus component-framework methodology
to ensure composability, low coupling, and independent development.

- Each component MUST be a separate Rust crate with a well-defined public
  interface (trait-based where appropriate).
- Components MUST minimize cross-component dependencies; shared state
  MUST be mediated through defined interfaces or receptacles.
- Components MUST be independently buildable and testable
  (`cargo test -p <crate>` MUST work in isolation).
- FFI boundaries (e.g., SPDK C bindings) MUST be isolated in dedicated
  `-sys` crates with safe Rust wrappers in companion crates.
- Component interfaces MUST use the project's actor/channel/receptacle
  patterns as defined by the component-framework.

### VII. Maintainability and Simplicity

Code MUST be written for long-term maintainability. Complexity MUST be
justified and minimized.

- Prefer simple, explicit code over clever abstractions.
- Functions MUST have a single, clear responsibility.
- Cyclomatic complexity SHOULD remain low; complex functions MUST be
  decomposed.
- Dependencies MUST be minimized — each new crate dependency MUST be
  justified.
- `cargo fmt --check` MUST pass (standard rustfmt configuration).
- Dead code, unused imports, and TODO comments in merged code are
  prohibited.

## Platform and Toolchain Requirements

- **Language**: Rust (edition 2021, MSRV 1.75)
- **Target OS**: Linux (x86_64, RHEL 9 / Fedora primary)
- **Build system**: Cargo (workspace at repository root)
- **SPDK integration**: Built via `deps/build_spdk.sh`, installed to
  `deps/spdk-build/`
- **System dependencies**: Managed via `deps/install_deps.sh` (dnf-based)
- **Python tooling**: meson, pyelftools (via `deps/requirements.txt`)
- **Required CI checks**: `cargo test --all`, `cargo clippy -- -D warnings`,
  `cargo fmt --check`, `cargo doc --no-deps`, Criterion benchmarks

## Development Workflow and Quality Gates

- **Pre-merge gates** (all MUST pass):
  1. `cargo fmt --check` — formatting compliance
  2. `cargo clippy -- -D warnings` — lint compliance
  3. `cargo test --all` — all unit, doc, and integration tests pass
  4. `cargo doc --no-deps` — documentation builds without warnings
  5. Criterion benchmarks — no unacknowledged regressions
- **Code review**: All changes MUST be reviewed before merge. Reviewers
  MUST verify constitution compliance.
- **Commit discipline**: Each commit MUST represent a logical, atomic
  change. Commits MUST NOT break the build.
- **Branch strategy**: Feature branches merge to main via pull request
  after passing all gates.

## Governance

This constitution is the authoritative reference for engineering standards
in the spdk-env component. It supersedes informal practices and ad-hoc
decisions.

- **Amendments** require: (1) a written proposal describing the change and
  rationale, (2) review and approval, (3) a migration plan for existing
  code if the change imposes new requirements.
- **Versioning** follows semantic versioning:
  - MAJOR: Principle removal or backward-incompatible redefinition.
  - MINOR: New principle or materially expanded guidance.
  - PATCH: Clarifications, wording fixes, non-semantic refinements.
- **Compliance review**: All pull requests MUST include a constitution
  compliance check. Violations MUST be resolved or receive an explicit,
  documented exception before merge.
- **Exception process**: Exceptions to any principle MUST be documented
  inline with rationale and an expiration condition (e.g., "until SPDK
  upstream provides a safe API for X").

**Version**: 1.0.0 | **Ratified**: 2026-04-07 | **Last Amended**: 2026-04-07
