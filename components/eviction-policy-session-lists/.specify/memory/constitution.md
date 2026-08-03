<!--
Sync Impact Report
==================
Version change: (template, unversioned) → 1.0.0
Rationale: Initial ratification. First concrete constitution derived from the
  blank template; introduces the full principle set and governance, so this is
  a MAJOR (0 → 1.0.0) establishment rather than an amendment.

Modified principles:
  - [PRINCIPLE_1_NAME] → I. Code Quality & Maintainability
  - [PRINCIPLE_2_NAME] → II. Correctness Assurance (NON-NEGOTIABLE)
  - [PRINCIPLE_3_NAME] → III. Comprehensive Testing
  - [PRINCIPLE_4_NAME] → IV. Performance Discipline
  - [PRINCIPLE_5_NAME] → V. Component-Framework Conformance & Interface Encapsulation
  - (added)           → VI. Linux Platform Target

Added sections:
  - Engineering Standards & Tooling (was [SECTION_2_NAME])
  - Development Workflow & Quality Gates (was [SECTION_3_NAME])

Removed sections: none

Templates requiring updates:
  - .specify/templates/plan-template.md ...... ✅ aligned (Constitution Check
    gate is constitution-driven; no hardcoded principles to change)
  - .specify/templates/spec-template.md ...... ✅ aligned (no principle-specific
    content; generic requirement scaffolding)
  - .specify/templates/tasks-template.md ..... ✅ aligned (test/perf/docs task
    types already expressible; tests remain opt-in per template)
  - .specify/templates/checklist-template.md . ✅ aligned (no changes required)

Follow-up TODOs: none — all placeholders resolved. RATIFICATION_DATE set to the
  first-fill date (2026-08-03) as no earlier adoption date exists.
-->

# Eviction Policy Session Lists Constitution

## Core Principles

### I. Code Quality & Maintainability

Code MUST be readable, idiomatic Rust that matches the surrounding style. Every
commit MUST pass `cargo fmt --check` and `cargo clippy -- -D warnings` (warnings
are errors), and `cargo doc --no-deps` MUST be warning-free. Modules and
functions MUST be small and single-purpose; duplication MUST be refactored
rather than copied. `unsafe` blocks MUST carry a `// SAFETY:` justification.

**Rationale**: Certus components are developed in isolation and integrated via
typed interfaces; consistent, lint-clean, well-factored code keeps each
component's context small and reviewable, which is the core premise of the
component model.

### II. Correctness Assurance (NON-NEGOTIABLE)

Correctness is the highest priority and MUST NOT be traded for speed of
delivery. Public behavior MUST be specified before or alongside implementation
and MUST be covered by tests that would fail if the behavior regressed. Known
defects block release; a change MUST NOT be marked complete while its tests
fail, its implementation is partial, or errors are unresolved. Failures MUST be
reported faithfully with evidence, never hidden or worked around.

**Rationale**: This component makes cache eviction decisions that affect data
lineage and correctness of the wider filesystem; a wrong eviction is difficult
to detect downstream, so correctness assurance is treated as non-negotiable.

### III. Comprehensive Testing

Every public API MUST have unit tests covering correctness, and performance-
relevant public APIs MUST have accompanying performance tests. Every public API
MUST additionally carry a Rust documentation test (`///` example) that compiles
and runs under `cargo test`. Tests MUST run single-threaded-clean (the CI runs
`cargo test --all -- --test-threads 1`) and MUST NOT depend on hardware unless
mocked.

**Rationale**: Doc tests keep examples honest and double as executable
documentation; requiring both correctness and performance coverage on the public
surface guarantees the interface contract is verified in both dimensions.

### IV. Performance Discipline

All performance-sensitive code MUST have Criterion benchmarks committed
alongside it, and those benchmarks MUST be runnable via `cargo bench`.
Performance requirements MUST be stated as measurable targets in the feature
spec/plan and MUST be validated against the Criterion results before a change is
considered complete. Performance regressions MUST be justified or fixed, not
silently accepted.

**Rationale**: Eviction runs on the hot path; without committed, repeatable
Criterion benchmarks a regression is invisible until it degrades production
throughput.

### V. Component-Framework Conformance & Interface Encapsulation

The component MUST conform to the `components/component-framework` methodology
(`define_component!`/`define_interface!`, `IUnknown` discovery, receptacle-based
dependency wiring, actor/channel execution model). Functionality MUST be exposed
ONLY through interfaces: no public functions, types, or symbols may be reachable
from outside the component except via its declared interfaces. All interface
trait definitions MUST live in the `components/interfaces` crate, not inside the
component crate.

**Rationale**: Strict interface encapsulation is what keeps components loosely
coupled and independently integratable; leaking public functions bypasses the
receptacle model and couples callers to internals.

### VI. Linux Platform Target

All code MUST build and run on Linux (the tested platforms are RHEL/Fedora).
Platform-specific assumptions outside Linux are out of scope; code MUST NOT
depend on non-Linux facilities. Toolchain targets are Rust stable, edition 2021,
MSRV 1.75.

**Rationale**: Certus is a Linux-only system tied to Linux userspace and kernel
facilities; constraining the target keeps testing and support tractable.

## Engineering Standards & Tooling

- Formatting: `rustfmt` default configuration; no bespoke style overrides.
- Linting: `cargo clippy -- -D warnings` MUST be clean.
- Documentation: public APIs MUST have doc comments with runnable examples;
  `cargo doc --no-deps` MUST be warning-free.
- Testing: `cargo test --all` (CI: `--test-threads 1`); SPDK/hardware paths MUST
  be exercised via mocks so default-member tests need no hardware.
- Benchmarking: Criterion suites under the crate, invoked with `cargo bench`.
- Toolchain: Rust stable, edition 2021, MSRV 1.75, Linux only.

## Development Workflow & Quality Gates

- Work proceeds on feature branches; changes MUST NOT be committed directly to
  the mainline branch.
- Every change MUST pass, before merge: `cargo fmt --check`,
  `cargo clippy -- -D warnings`, `cargo test --all`, and `cargo doc --no-deps`.
- Performance-sensitive changes MUST include or update Criterion benchmarks and
  cite measured results in the change description.
- Public API additions MUST land with unit tests, a performance test where
  performance-relevant, and a doc test, and the interface trait MUST be defined
  in `components/interfaces`.
- Reviews MUST verify constitution compliance; any deviation MUST be recorded in
  the plan's Complexity Tracking with justification and the rejected simpler
  alternative.

## Governance

This constitution supersedes other conventions for this component when they
conflict. Amendments MUST be proposed via a change that updates this file,
states the rationale, and increments the version per the policy below;
amendments MUST also propagate to dependent templates and guidance docs in the
same change.

Versioning policy (semantic):
- MAJOR: backward-incompatible governance change, or removal/redefinition of a
  principle.
- MINOR: a new principle or section is added, or guidance is materially
  expanded.
- PATCH: clarifications, wording, or typo fixes with no semantic change.

Compliance review: every plan MUST pass the Constitution Check gate before Phase
0 and again after Phase 1 design; every PR/review MUST verify compliance.
Complexity or deviation MUST be justified in the plan's Complexity Tracking
table or the change is rejected. Runtime development guidance lives in the
component and repository `CLAUDE.md` files and MUST stay consistent with these
principles.

**Version**: 1.0.0 | **Ratified**: 2026-08-03 | **Last Amended**: 2026-08-03
