<!--
Sync Impact Report
==================
Version change: 0.0.0 → 1.0.0 (initial ratification)
Modified principles: N/A (new document)
Derived from: components/remote-lookup/.specify/memory/constitution.md v1.0.0
Added sections:
  - Core Principles (11 principles: I-VII inherited, VIII-XI specific to this component)
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

# Certus Transfer Endpoint Component Constitution

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
`cargo test -p certus-transfer-endpoint`.

**Rationale**: Unit tests are the primary mechanism for asserting behavioral
correctness. Untested APIs are considered unfinished.

### III. Documentation Tests

All public API methods MUST include Rust doc comments (`///`) with runnable
examples using ```` ```rust ```` fenced code blocks. These doc tests MUST
compile and pass under `cargo test --doc -p certus-transfer-endpoint`.
Documentation MUST be warning-free under `cargo doc --no-deps`.

**Rationale**: Doc tests serve dual purpose — they document usage for consumers
and act as regression tests that stay synchronized with the implementation.

### IV. Performance Testing

All performance-sensitive code paths MUST have Criterion-based benchmarks in
a `benches/` directory, runnable via `cargo bench -p certus-transfer-endpoint`.
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

### VIII. Hardware-Independent Default Build

The default feature set MUST build and unit-test with **neither rdma-core
(libibverbs/librdmacm) nor SPDK present**, exercising connection-table, framing,
credit and pin logic over an in-process transport seam. Real verbs MUST be gated
behind `rdma`; device reads behind `spdk`. Hardware-dependent tests MUST be
`#[ignore]`d so a default `cargo test` never requires a NIC or an NVMe device.
The component MUST remain in the workspace `default-members`.

**Rationale**: Per `CLAUDE.md`, a plain `cargo build`/`cargo test --all` works
without SPDK, and CI builds only default members. A component whose logic can
only be tested on specialized hardware is a component whose logic is not
routinely tested. Both predecessor crates established this pattern; it is
preserved deliberately.

### IX. Pin Discipline

Residency determination and pinning MUST be a single atomic dispatch-map
lookup. Code MUST NOT determine residency with an unpinned probe and then pin
separately. A read reference MUST NEVER be released for an answer that was not
pinned (`NotExist`, size mismatch) — doing so decrements another reader's pin.
Pins MUST be acquired and released **per entry**, never per batch, and a pin
held across an asynchronous operation MUST be released by the completion reaper
that owns its work-request id. A landing slot MUST NOT be reclaimed or published
until queue-pair teardown guarantees no write can still land in it.

**Rationale**: Every memory-safety hazard in one-sided RDMA transfer reduces to
a pin held too briefly or released twice. The dispatch map already increments
`read_ref` *before* inspecting location precisely so that no probe-then-pin
window can exist; the only way to reintroduce the race is to add one. An
over-released pin silently corrupts an unrelated reader, which no test
reliably catches.

### X. Bounded Resources, Graceful Chunking

Every resource consumed on behalf of a peer MUST be explicitly bounded, and the
bounds MUST be separately named and configured — send-queue credits (in-flight
writes, and transitively held pins), message credits (outstanding requests against
a peer's posted receive buffers), and bounce-buffer occupancy. On exhaustion of
any bound the endpoint MUST chunk internally and make progress; it MUST NOT fail
a request for want of a transient local resource. Serving a peer MUST NOT degrade
the serving node's own cache quality — block-tier entries MUST NOT be promoted
into the memory tier to satisfy a remote request.

**Rationale**: A single request can span a full model context (~3.4 GiB). Unbounded
resources turn a large request into an outage, and conflated bounds either
over-restrict or under-restrict two of the three. Promotion-to-serve would make
serving peers self-harming, which creates incentives to refuse service.

### XI. Loud Failure Over Silent Degradation

Where a safety net would mask a defect and still yield a plausible measurement,
this component MUST fail loudly instead. Deadlines and watchdogs MUST default to
logging full diagnostic state and failing the operation; any degrading
alternative MUST be opt-in and separately configured. Metrics MUST NOT be
structured so that an unexercised path is indistinguishable from a working one.

**Rationale**: This component exists to answer a research question by measurement.
A mechanism that quietly drops work and still reports a number invalidates the
result in a way that is hard to detect. This project has paid for the alternative
more than once — a disable-flag verified against one engine version and never
re-checked, and an optimization silently not firing with correctness intact and
no metric to show it.

## Platform & Technology Constraints

- **Language**: Rust stable, edition 2021, MSRV 1.75
- **OS**: Linux only (RHEL 9 / Fedora primary targets), x86-64
- **Framework**: `components/component-framework` (`define_component!`,
  `define_interface!`, `IUnknown` for runtime interface discovery)
- **Interface location**: All interface trait definitions MUST reside in
  `components/interfaces/src/`
- **Features**: `default = []` (mock seam, memory-tier serving); `rdma` (real
  libibverbs/librdmacm via a hand-written FFI layer and a `cc`/`pkg-config` shim);
  `spdk` (block-tier serving via a block-device receptacle); `telemetry` (metrics).
  `rdma` and `spdk` MUST be independently selectable.
- **Fabric**: RoCE. Single-host RDMA loopback is the verification path for
  hardware-dependent tests.
- **No zyre dependency.** Peer selection is external to this component; it neither
  discovers nor broadcasts.
- **Testing**: `cargo test` for unit/doc tests, Criterion for benchmarks
- **Linting**: `cargo fmt --check` and `cargo clippy -- -D warnings`
- **Documentation**: `cargo doc --no-deps` MUST be warning-free

## Development Workflow

1. **Interface First**: Define or extend the interface trait in
   `components/interfaces` before implementing in this component.
2. **Seam First**: New transport behaviour MUST be expressible and tested over
   the in-process mock seam before the real verbs path is written. If it can only
   be tested on hardware, the seam is wrong.
3. **Test First**: Write failing tests for new functionality before
   implementation. Doc tests count toward this requirement.
4. **Benchmark Early**: If a code path has performance requirements,
   add the Criterion benchmark before or alongside implementation.
5. **Lint Before Commit**: Run `cargo fmt` and `cargo clippy -- -D warnings`
   before every commit. CI will reject non-conforming code.
6. **Review Gate**: All changes MUST pass `cargo test -p certus-transfer-endpoint`,
   `cargo clippy -- -D warnings`, and `cargo doc --no-deps` before merge.
   Changes touching pin acquisition or release MUST be reviewed against
   Principle IX explicitly.

## Governance

This constitution is the authoritative standard for the
`certus-transfer-endpoint` component. All code reviews MUST verify compliance
with these principles. Non-compliance MUST be resolved before merge.

**Amendment procedure**: Changes to this constitution require documented
justification, version increment, and updated `LAST_AMENDED_DATE`. MAJOR
version for principle removal/redefinition, MINOR for additions, PATCH for
clarifications.

**Compliance review**: Every pull request touching this component MUST be
checked against each principle. Violations MUST be flagged as blocking.

**Version**: 1.0.0 | **Ratified**: 2026-10-14 | **Last Amended**: 2026-10-14
