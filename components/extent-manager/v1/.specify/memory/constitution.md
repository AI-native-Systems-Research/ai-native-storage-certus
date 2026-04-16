<!--
Sync Impact Report
===================
Version change: 0.0.0 -> 1.0.0
Modified principles: N/A (initial creation)
Added sections:
  - Preamble
  - Principle 1: Correctness First
  - Principle 2: Comprehensive Testing
  - Principle 3: Performance Accountability
  - Principle 4: Documentation as Contract
  - Principle 5: Maintainability and Code Quality
  - Principle 6: Component-Framework Conformance
  - Principle 7: Interface-Driven Public API
  - Principle 8: Platform and Toolchain Discipline
  - CI Gate
  - Governance
Removed sections: N/A
Templates requiring updates:
  - .specify/templates/plan-template.md - pending (not yet created)
  - .specify/templates/spec-template.md - pending (not yet created)
  - .specify/templates/tasks-template.md - pending (not yet created)
Follow-up TODOs: None
-->

# Project Constitution

**Project**: Extent Manager v1
**Description**: A fixed-size storage extent manager for the Certus
storage system. Manages extent allocation, metadata persistence, and
crash recovery on NVMe SSDs using 4KiB-atomic writes.
**Version**: 1.0.0
**Ratification Date**: 2026-04-16
**Last Amended Date**: 2026-04-16

---

## Preamble

This constitution defines the non-negotiable engineering principles
governing the Extent Manager v1 component. All contributors, automated
agents, and code generation tools MUST adhere to these principles.
Deviations require an explicit amendment through the governance process
defined below.

---

## Principles

### Principle 1: Correctness First

Every public API MUST have unit tests that verify correctness under
normal operation, edge cases, and error conditions. Unsafe code MUST
be justified with a safety comment and covered by tests that exercise
the invariants the unsafe block depends on. All data-path code MUST
be tested for crash consistency — fault injection tests MUST verify
that recovery produces a consistent state after simulated failures at
every write boundary.

**Rationale**: The extent manager is a storage-critical component.
Silent data corruption or lost allocations are unacceptable. Testing
is the primary mechanism for establishing confidence in correctness.

### Principle 2: Comprehensive Testing

Unit tests, integration tests, and doc tests are MANDATORY for all
public interfaces. `cargo test -p extent-manager` MUST pass with zero
failures and zero ignored tests before any merge. Test coverage MUST
include:

- Happy-path operations for every public method.
- Boundary conditions (zero slots, maximum slots, full bitmaps).
- Concurrent access from multiple threads.
- Crash recovery scenarios using `MockBlockDevice` with
  `FaultConfig` and `reboot_from()`.
- Error propagation for all `Result`-returning methods.

Tests MUST run without SPDK, hugepages, or NVMe hardware — all I/O
MUST be abstracted through `MockBlockDevice` for test execution.

**Rationale**: Comprehensive tests catch regressions early and enable
confident refactoring. Hardware-free testing ensures CI reliability.

### Principle 3: Performance Accountability

All performance-sensitive public APIs MUST have Criterion benchmarks
in `benches/`. `cargo bench -p extent-manager --no-run` MUST compile
without errors. Benchmarks MUST cover at minimum:

- Single-extent allocate and free operations.
- Bulk allocation and deallocation.
- Metadata read/write throughput.
- Recovery/open time as a function of extent count.

Performance regressions MUST be detected and addressed before merge.
Benchmark results SHOULD be compared against a baseline when
evaluating changes to hot paths.

**Rationale**: The extent manager sits in the storage data path.
Measurable performance baselines prevent silent degradation and
provide evidence for optimization decisions.

### Principle 4: Documentation as Contract

Every public type, function, method, and trait MUST have rustdoc
comments. Doc comments MUST include:

- A one-line summary of purpose.
- A description of parameters, return values, and error conditions.
- At least one runnable `# Examples` block demonstrating typical use.

`cargo doc -p extent-manager --no-deps` MUST complete with zero
warnings. Documentation serves as the API contract — if the docs
and the code disagree, the code has a bug.

**Rationale**: Documentation enables independent use and review of
the component. Runnable examples serve as additional correctness
tests and prevent doc rot.

### Principle 5: Maintainability and Code Quality

All code MUST pass `cargo fmt -p extent-manager --check` and
`cargo clippy -p extent-manager -- -D warnings` with zero violations.
The public API surface MUST be minimal — prefer `pub(crate)` over
`pub` for internal types. Functions MUST have a single clear
responsibility. Code duplication MUST be eliminated through
well-named abstractions only when three or more instances exist.

Naming conventions MUST follow Rust API Guidelines (RFC 430). Error
types MUST implement `std::error::Error` and provide actionable
messages.

**Rationale**: Consistent formatting and lint-clean code reduce
review friction and cognitive load. Minimal public surfaces reduce
the maintenance burden and accidental coupling.

### Principle 6: Component-Framework Conformance

All components MUST be defined using the `define_component!` macro.
All interfaces MUST be defined using the `define_interface!` macro.
Components MUST follow the component-framework lifecycle:
`new_default()` -> wire receptacles -> `initialize()`/`open()` -> use
provider interfaces. Receptacles and providers MUST be declared via
the macro system and wired through the framework — direct
construction of inter-component dependencies is prohibited.

**Rationale**: The component-framework enforces a uniform wiring and
lifecycle model across the Certus system. Conformance ensures that
the extent manager integrates correctly with the broader system and
benefits from framework-provided facilities (dependency injection,
lifecycle management).

### Principle 7: Interface-Driven Public API

All public functions MUST be exposed exclusively through a
`define_interface!`-declared interface trait. No standalone `pub fn`
items may exist outside of interface implementations. Consumer code
MUST interact with the extent manager solely through `IExtentManager`
and `IExtentManagerAdmin` trait objects. Internal helper functions
MUST use `pub(crate)` or narrower visibility.

**Rationale**: Interface-driven design enables substitution (mocking,
alternative implementations) and enforces a clean separation between
API contract and implementation detail. This is a core tenet of the
component-framework methodology.

### Principle 8: Platform and Toolchain Discipline

All code MUST compile and run on the Linux operating system. No
platform-conditional compilation (`#[cfg(target_os = ...)]`) for
non-Linux targets is permitted. The Rust stable toolchain MUST be
used — no nightly features. The minimum supported Rust version (MSRV)
is 1.75. Edition 2021 MUST be used. No external runtime dependencies
beyond the component-framework crates are permitted.

**Rationale**: The Certus storage system targets Linux exclusively.
Stable-toolchain and minimal-dependency constraints ensure
reproducible builds and reduce supply-chain risk.

---

## CI Gate

The following commands MUST all pass before any code is merged:

```bash
cargo fmt -p extent-manager --check
cargo clippy -p extent-manager -- -D warnings
cargo test -p extent-manager
cargo doc -p extent-manager --no-deps
cargo bench -p extent-manager --no-run
```

Failure of any command blocks the merge.

---

## Governance

### Amendment Procedure

1. Propose the change as a discussion or pull request with a clear
   rationale.
2. The amendment MUST be reviewed by at least one project maintainer.
3. Update this constitution file, increment the version, and update
   `Last Amended Date`.
4. Propagate changes to all dependent templates and artifacts listed
   in the Sync Impact Report.

### Versioning Policy

This constitution follows semantic versioning:

- **MAJOR**: Removal or incompatible redefinition of a principle.
- **MINOR**: Addition of a new principle or material expansion of
  existing guidance.
- **PATCH**: Clarifications, wording fixes, non-semantic refinements.

### Compliance Review

All pull requests MUST be evaluated against these principles.
Automated CI checks enforce Principles 2-5 and 8. Principles 1, 6,
and 7 require review-time verification. Non-compliance MUST be
resolved before merge.
