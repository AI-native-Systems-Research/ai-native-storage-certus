<!--
Sync Impact Report
===================
Version change: 0.0.0 → 1.0.0 (initial ratification)
Modified principles: N/A (initial version)
Added sections:
  - I. Consumer Independence (NON-NEGOTIABLE)
  - II. Determinism and Reproducibility
  - III. One Definition per Statistic
  - IV. Evidence over Assertion
  - V. Loud Failure over Quiet Wrongness
  - VI. Code Quality and Correctness
  - VII. Documentation as Contract
  - Platform and Tooling Requirements
  - Development Workflow
  - Governance
Templates requiring updates:
  - plan-template.md — ✅ reviewed; the Constitution Check in plan.md is rewritten against these
  - spec-template.md — ✅ reviewed; requirements sections align
  - tasks-template.md — ⚠ not yet exercised (tasks.md not generated)
Follow-up TODOs:
  - The repository-level .specify/memory/constitution.md is still an unfilled template. It cannot be
    inherited from and does not propagate: `specify init` scaffolds a fresh template rather than
    copying the parent's. Recommend filling or deleting it as separate work.
Notes on scope:
  - This is an APP (a CLI tool suite), not a component. The component-framework conformance and
    interface-only-exposure principles that other constitutions in this repository open with do not
    apply and are deliberately absent; principle I states the boundary discipline that replaces them.
-->

# Workload Generator Constitution

## Core Principles

### I. Consumer Independence (NON-NEGOTIABLE)

The generator MUST NOT contain any concept of a tier, cache, memory, or disk. A workload is a
statistical statement about *which blocks are asked for, by whom, in what order, at what size*, and it
MUST mean exactly the same thing whether its consumer has two storage tiers, five, one, or none.
Capacities, eviction policies, watermarks, pinning, and the placement of copies are properties of the
*consumer* and MUST be absent from the schema, the plan artifact, and every report the generator
produces. Where a block was resolved from is an outcome the consumer reports; the generator MUST relay
such an attribution verbatim when one is offered and MUST NOT derive, infer, or predict one.

**Rationale**: This is the constraint from which most of the design follows. A generator that knows
what a tier is becomes a benchmark harness for one specific storage system, and its workloads stop
being comparable across consumers — including across future versions of the same consumer. Keeping the
boundary absolute is also what lets a workload be replayed against a simulator, a competitor, or a
plain file and have the comparison mean something.

### II. Determinism and Reproducibility

Generation MUST be fully determined by the input document plus its seed. Key identity MUST be
computable from a key's own path without reference to arrival order, global mutable state, or any
earlier request. Every random draw that shapes structure MUST be keyed on the *entity* it describes
rather than on the moment it is visited, so that a run of any length is reproducible while remaining
stochastic. Resident memory MUST be bounded by the live entity population rather than by run length.

**Rationale**: Reproducibility is the whole value of a synthetic workload — an experiment that cannot
be repeated exactly is not a measurement. Path-computable identity is what simultaneously buys
independent multi-node generation, bounded memory, and unbounded run length; each of those would
otherwise need its own mechanism, and arrival-order dependence would forfeit all three at once.

### III. One Definition per Statistic

Any statistic computed over more than one kind of input MUST have exactly one implementation, in a
shared library, used by every binary that reports it. A statistic MUST NOT be reimplemented per tool,
per container format, or per input kind.

**Rationale**: These tools exist to compare things — a fitted model against its source trace, one arm
against another. Two implementations of a statistic drift, and a comparison between them then compares
two different definitions of the same quantity. That failure is invisible: it does not error, it
*appears to succeed*, and it silently invalidates every result derived from it.

### IV. Evidence over Assertion

A quantity presented as measured MUST be measured, and its provenance MUST be recorded. A quantity
that is a judgement MUST be labelled as one, even where evidence is consistent with it. Where a model
does not reproduce an observed phenomenon, the omission MUST be stated rather than left to be inferred,
and where a measurement is confounded, the confound MUST be recorded beside the result. Defaults MUST
be derived rather than asserted, and a figure derived from a sample MUST NOT be presented as a property
of a population.

**Rationale**: This tool's output is evidence used to make engineering decisions, so its own
credibility is a functional requirement. A number whose status is unclear is worse than no number,
because it will be relied on exactly as far as a real measurement would be.

### V. Loud Failure over Quiet Wrongness

A configuration that is internally consistent but cannot measure what it claims MUST be **rejected**,
not warned about, whenever the resulting numbers would be wrong rather than merely noisy. Validation
MUST report every violation in a document rather than the first. A capability that is unavailable MUST
be reported as unavailable with its reason; an absent or zeroed value MUST NEVER be allowed to read as
a passing result. Where a parameter cannot be determined from the available input, it MUST be left
unset rather than defaulted.

**Rationale**: The failures worth engineering against here are not crashes but plausible wrong answers
— a warmup shorter than the population ramp, a fit from six records, a cross-check against a counter
nobody enabled. Each produces a confident report that is silently meaningless, and each is cheap to
detect at the point where the information still exists.

### VI. Code Quality and Correctness

Code MUST be formatted with `rustfmt` defaults and MUST pass `cargo clippy -- -D warnings`. Performance
claims MUST be substantiated by Criterion benchmarks rather than by assertion. Any `unsafe` block MUST
carry a `// SAFETY:` justification. Invariants the design depends on — namespace disjointness, record
width and alignment, digest agreement — MUST be enforced in code and covered by tests, not merely
documented.

**Rationale**: These are the repository's existing standards (`CLAUDE.md`) and this app is held to
them. The invariant clause is specific to this feature: several of its correctness properties are
structural rather than behavioural, so a suite that only exercises outputs would not detect their
violation.

### VII. Documentation as Contract

The normative input format, artifact layout, and interchange format MUST each be specified in a
contract document, and the implementation MUST conform to the contract rather than the contract being
amended to match the implementation. Public library APIs MUST carry doc comments with runnable
examples, and `cargo doc --no-deps` MUST be warning-free. Where a decision reverses an earlier one,
the reversal and its reason MUST be recorded rather than the earlier text silently replaced.

**Rationale**: These artifacts outlive any one run and are consumed by tools not yet written, so the
contract is the interface. Recording reversals matters because several decisions in this feature were
reached by refuting an earlier one; without the reasoning, a rejected option looks like an oversight
and gets reintroduced.

## Platform and Tooling Requirements

- **Platform**: Linux only, x86_64. Rust edition 2021, MSRV 1.75.
- **Default build hygiene**: `cargo test --all` MUST compile and pass without SPDK, CUDA, RDMA, or a
  columnar-format dependency. Crates requiring hardware MUST stay out of `default-members`; heavy
  optional dependencies MUST be feature-gated with the feature off by default.
- **Test isolation**: hardware-dependent tests MUST NOT run as part of the default test target.
- **Reproducible artifacts**: every emitted artifact MUST record the generator version and build, the
  normalised input, the seed, and a digest of what it identifies — and MUST state which kind of digest
  it carries wherever more than one kind is possible.

## Development Workflow

- **Specification first**: functional requirements, success criteria, and contracts precede
  implementation. A requirement that cannot be tested MUST be restated until it can be.
- **Design decisions are recorded with rationale** in the specification's clarification log, including
  options rejected and why. Superseded decisions MUST be marked, not deleted.
- **Evidence lives in `research.md`**, separate from normative text, so requirements state conclusions
  while the measurements and their limitations remain auditable.
- **Commits** are per-feature-area, with messages stating what changed and why, and MUST note when a
  change alters an earlier requirement rather than adding to it.

## Governance

This constitution governs development of the workload generator tool suite. It supersedes informal
conventions and ad-hoc decisions within this app.

- **Amendments**: Any change MUST be documented with a version bump, a rationale, and a review of
  dependent artifacts (templates, specs, plans) for consistency.
- **Versioning**: Semantic versioning:
  - MAJOR: Principle removal or backward-incompatible redefinition.
  - MINOR: New principle or materially expanded guidance.
  - PATCH: Clarifications, wording fixes, non-semantic refinements.
- **Compliance Review**: All pull requests and code reviews MUST verify compliance with these
  principles. Non-compliance MUST be resolved before merge.
- **Conflict Resolution**: If a principle conflicts with a practical constraint, the conflict MUST be
  documented, justified, and approved before an exception is granted.
- **Relationship to the repository**: the repository-level constitution at
  `.specify/memory/constitution.md` is an unfilled template and is therefore not inherited. Should it
  later be filled, any conflict with this document MUST be resolved in favour of the repository-level
  one, and this document amended.

**Version**: 1.0.0 | **Ratified**: 2026-08-10 | **Last Amended**: 2026-08-10
