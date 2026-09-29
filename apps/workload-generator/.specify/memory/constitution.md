<!--
Sync Impact Report
===================
Version change: 0.0.0 → 1.0.0 (initial ratification)
Base: adopted from the repository's de facto constitution, whose canonical
  form is shared by components/dispatch-map and components/dispatcher-p2p
  (identical seven-principle structure; nine other filled constitutions vary
  around it). Note: the only BYTE-identical constitution shared by several
  directories in this repo is the unfilled template (six copies, including
  the repo root), which silently disables the gate — it is not a base.
Retargeted from the base (this is an application, not a component):
  - I. Component-Framework Conformance → I. Client-Protocol Conformance.
    This app uses no component-framework macros, receptacles, or IUnknown; its
    external contract is the shmq wire protocol and the vLLM connector's op
    stream. Precedent: apps/remote-lookup-bench.
  - II. Interface-Only Exposure → II. Interface-Only Consumption.
    Inverted for a client: the constraint is what this app may DEPEND ON,
    not what it may expose.
Retained near-verbatim from the base:
  - III. Code Quality and Correctness
  - VI. Documentation Standards
  - VII. Maintainability and Engineering Practice
  - Platform and Tooling Requirements / Development Workflow / Governance
Adapted from the base:
  - IV. Comprehensive Testing (receptacle-wiring tests → transport-contract and
    plan-determinism tests)
  - V. Performance Validation (adds the generator-is-not-the-bottleneck
    assertion)
Added, specific to a measurement instrument:
  - VIII. Measurement Validity (NON-NEGOTIABLE)
  - IX. Specified Statistical Machinery
Templates requiring updates:
  - plan-template.md — Constitution Check section aligns
  - spec-template.md — requirements section aligns
  - tasks-template.md — test-first phasing aligns
Follow-up TODOs: none
-->

# Synthetic Workload Generator Constitution

This tool exists so that measurements taken through it mean something.
Principles I-VII are the repository's shared engineering standard, retargeted
where a component principle does not apply to an application. Principles VIII
and IX exist because this program's output is evidence.

## Core Principles

### I. Client-Protocol Conformance (NON-NEGOTIABLE)

The operation stream this app emits MUST be what vLLM's connector would emit
for the same workload — no more and no less. More is cheating; less understates
load.

Derived from `certus-connector`, not invented:

- The store path MUST be `Reserve` → `CopyToStore` →
  `CommitStore`/`AbortStore`. `Populate` MUST NOT be used: it is not in the
  connector's vocabulary.
- `Touch` MUST be sent with `promote: 0`. The connector issues a bare
  `dispatcher.touch(key)`; tier promotion happens inside `prepare_load` as a
  consequence of an NVMe hit, never as a client hint.
- `TakeEvents` MUST be polled, because the connector polls it and it costs the
  server real work.
- `Remove`, `Pin`, `Unpin`, and `promote: 1` MUST NOT be sent. At most one
  `ClearMemoryTier` at startup is permitted.

Any deviation from the connector's stream is a specification change requiring a
recorded justification, never a convenience.

**Rationale**: The framework principle this replaces exists so a component
integrates with the rest of Certus. The analogue for a client is that it must
be indistinguishable from the real one — otherwise every number it produces
describes a system nobody runs. Withholding information the real client
supplies is as distorting as inventing information it does not: starving a
recency policy of `Touch` biases the comparison exactly as much as helping it
would.

### II. Interface-Only Consumption

This app MUST reach Certus only through the shmq wire protocol (`shm-queue` for
transport, `shmq-dispatcher::wire` for framing). It MUST NOT link Certus
component crates, call dispatcher internals, or depend on component-framework
wiring.

Where a handful of foreign symbols are needed (CUDA IPC and memcpy), they MUST
be declared locally rather than pulled in through a component crate. Precedent
and rationale are recorded in `apps/remote-lookup-bench/Cargo.toml`: depending
on `gpu-services` unifies cargo features across the workspace and can leave
that crate an incomplete `IGpuServices` impl.

**Rationale**: Same purpose as the component principle, from the other side of
the boundary. A load generator that links the system it measures can
accidentally measure a different build of it, and can drift from what an
external client can actually do.

### III. Code Quality and Correctness

- All code MUST compile without warnings under `cargo clippy -- -D warnings`.
- All code MUST pass `cargo fmt --check` with default `rustfmt` settings.
- `cargo doc --no-deps` MUST produce zero warnings.
- All `unsafe` code MUST include a `// SAFETY:` justification comment.
- Assurance of code correctness is of the highest importance. All logic MUST be
  verified through tests (see Principle IV). Edge cases, error paths, and
  boundary conditions MUST be explicitly tested.
- All code MUST target and run on the Linux operating system exclusively.

**Rationale**: Strict lint and format enforcement catches defects early.
Correctness assurance through testing prevents regressions and builds
confidence in reliability.

### IV. Comprehensive Testing

- All public APIs MUST have unit tests validating correctness.
- All public APIs MUST have Rust documentation tests (`///` doc examples) that
  compile and run as tests via `cargo test`.
- The wire encoding MUST have contract tests asserting byte-for-byte agreement
  with `shmq-dispatcher::wire`, and the generator↔daemon transport MUST have
  round-trip tests independent of any live server.
- Distribution sampling MUST have statistical tests: truncation bounds
  respected, integer draws uniform across their range at the endpoints as well
  as the interior, and seeded runs reproducible.
- Plan determinism MUST be tested directly: for a fixed input file and seed,
  the emitted plan is byte-identical regardless of execution speed or stalls.
- Tests MUST NOT depend on GPU or NVMe hardware, or on a running server; mock
  the transport where hardware is absent.
- All tests MUST pass under single-threaded execution (`--test-threads 1`) for
  CI compatibility.

**Rationale**: Comprehensive testing at every level is the primary mechanism
for assuring correctness. For this app the highest-value tests are the ones
that pin its two external contracts — the wire format and the plan — because a
silent change to either invalidates results without failing anything.

### V. Performance Validation

- All performance-sensitive code MUST have Criterion-based benchmarks,
  available under `cargo bench` or targeted via `cargo bench --bench <name>`.
- No per-operation work may be proportional to payload size. Block payloads
  MUST be pre-filled, reused device buffers carrying at most a small key
  stamp — never bytes constructed per operation.
- The per-key cost of plan generation MUST be benchmarked and MUST remain far
  below the measured 5.6-14 us/key end-to-end cost of the path it feeds.
- Performance regressions MUST be detectable by comparing Criterion results
  across commits.
- Claims about hardware behaviour MUST rest on hardware measurement with n >= 8
  and a stated significance test. This bench has a recorded history of n = 3
  sampling producing conclusions that later measurement reversed.

**Rationale**: Criterion gives statistically rigorous measurement that ad-hoc
timing cannot. Principle VIII's first clause cannot be defended by inspection,
so it has to be benchmarked.

### VI. Documentation Standards

- All public API items (traits, structs, functions, methods) MUST have doc
  comments (`///`) with a summary line, parameter and return descriptions where
  non-obvious, and a runnable `# Examples` section that serves as a doc test.
- `cargo doc --no-deps` MUST build without warnings.
- Module-level documentation (`//!`) MUST describe the module's role.
- The workload input schema MUST be documented in the contract under
  `specs/*/contracts/`, and the shipped example file MUST parse and validate
  against the implementation as a test.

**Rationale**: Well-documented APIs reduce onboarding time, prevent misuse, and
provide executable examples that double as correctness tests. The input schema
is this tool's real user interface, so it earns the same standard as the code.

### VII. Maintainability and Engineering Practice

- Follow YAGNI: do not add features, abstractions, or code paths beyond what
  the current requirements demand.
- Prefer simple, direct implementations over premature abstractions. Three
  similar lines are better than a premature helper.
- Error handling MUST be explicit: use `Result` types; do not panic in library
  code except for unrecoverable invariant violations.
- Dependencies MUST be minimized. Each new dependency MUST be justified by a
  clear need that cannot be met by existing dependencies or reasonable inline
  code.
- Code MUST be structured for readability: well-named identifiers, short
  functions with single responsibilities, minimal nesting.

**Rationale**: Maintainability sustains velocity over time. Simple code is
easier to review, test, debug, and evolve. Minimal dependencies reduce
supply-chain risk and build complexity.

### VIII. Measurement Validity (NON-NEGOTIABLE)

**The generator is never the measurement.** Whenever the generator is the
bottleneck, the run measures the generator. This MUST be asserted per run,
never assumed: the operation plan is produced ahead of the lanes that consume
it, and if the plan queue ever drains, the run is **invalid** and MUST be
reported as invalid rather than published. Every run report MUST carry
plan-queue occupancy and lane utilisation alongside its throughput number; a
throughput number published without them is not a result.

**Virtual time never touches wallclock.** Certus has no clock — its only notion
of time is the sequence of operations it is handed. Virtual time therefore
exists solely to decide that sequence. The virtual clock is the minimum
timestamp among operations still in flight; when Certus applies backpressure a
lane blocks and the clock **holds**, never advancing, skipping, or dilating
unevenly. Think time is consumed entirely at plan time, where it decides
interleaving, and MUST NOT reappear at execution time as idle Certus.

**Reproducible where it can be, honest where it cannot.** The plan is a pure
function of the input file and the seed. The outcome is not: two sessions may
race to mint the same shared prefix, both miss, and both store — which is what
production vLLM does, so the race is preserved rather than designed away.
Hit/miss results are therefore not bit-reproducible, and any A/B comparison
MUST use repetitions with a stated significance test. A deterministic-mint mode
MAY be offered for bisection; reproducibility claims MUST state which mode was
used.

**Rationale**: These three are the difference between an instrument and a load
script. Each has a specific failure mode that produces plausible numbers rather
than an error: a queue that underran reports the generator's speed as the system's, a
wallclock-coupled clock lets a slow server quietly reshape the workload it is
being judged on, and single-run A/B comparisons of a racing system report noise
as signal.

### IX. Specified Statistical Machinery

- Truncation MUST be inverse-transform between F(min) and F(max) — a real
  truncation, never clamping, which piles mass at the boundary and shifts the
  mean.
- Integer-valued draws MUST truncate on [min - 0.5, max + 0.5] and round to
  nearest, so every integer in range carries equal weight.
- Population pools MUST be seeded from the equilibrium residual-life
  distribution, never from the lifetime distribution itself, which synchronises
  the pool into cohorts.
- Where a parameter's effective value differs from what the user wrote — a
  truncation that moves a mean, an implied maximum that discards tail mass —
  the effective distribution MUST be reported at load time, and configurations
  discarding more than 5% of a distribution's mass MUST be refused.
- Any statistical claim in a spec, comment, or report MUST be traceable to a
  derivation or a measurement in this repository.

**Rationale**: Silent reshaping of a user's stated intent is a defect, not a
convenience — and it is undetectable downstream, because the workload still
runs and still produces numbers. Requiring traceability is not pedantry, and
this principle has already caught its own first error. During design, an
integral controller was proposed, simulated, and rejected on the strength of a
measured "8-23% population bias". Re-running it against a reference C
implementation showed **that measurement did not support that conclusion**: the
controller regulates the mean to under 1%, and the bias came from a
badly-scaled parameter sweep. The requirement survives on a different and
better ground — a regulator halves the population's dispersion, where the
instrument needs the Poisson `var = mean` of a real population — but the
rationale on record was wrong for a week. The evidence now lives in
`research/population/`, runnable, which is what would have caught it sooner.

## Platform and Tooling Requirements

- **Target OS**: Linux only, x86-64 (tested on RHEL/Fedora). The shmq transport
  relies on x86-TSO store ordering and shared futexes; it MUST NOT be ported to
  a weakly ordered architecture without adding fences.
- **Language**: Rust stable, edition 2021, MSRV 1.75.
- **Workspace membership**: a workspace member but NOT a default member,
  following `apps/remote-lookup-bench` — it requires CUDA at link time. Build
  explicitly with `-p`.
- **Build/Test/Lint/Format/Docs/Bench**: `cargo
  {build,test,clippy,fmt,doc,bench} -p <crate>` for each crate this feature
  defines; crate names are fixed by the plan.
- **Ingress**: the only path into Certus is the host-local `/dev/shm` shmq
  mailbox. Remote nodes are driven by a resident per-node daemon. Only keys
  cross the network — the daemon reconstructs block payloads from the key — so
  the remote transport's problem is latency hiding through pipelining depth,
  not bandwidth.
- **Concurrency budget**: lane count MUST NOT exceed the server's shmq channel
  count. shmq is depth-1 per channel, so concurrency *is* channel count and
  over-subscription silently serialises.
- **Workload-file portability**: a workload YAML file describes a workload and
  nothing else. Node lists, shm names, run length, output format, and lane
  count are command-line options, so one file is portable across clusters
  unchanged.

## Development Workflow

- All changes MUST pass the full quality gate before merge: `fmt` check,
  `clippy` lint, `doc` build, and all tests (unit, doc, integration).
- Commits SHOULD be atomic and focused on a single logical change.
- Performance-sensitive changes MUST include before/after Criterion results.
- All new public API surface MUST include doc tests and unit tests in the same
  commit that introduces the API.
- Changes to the wire format, the plan format, or the emitted op stream MUST
  update the corresponding contract under `specs/*/contracts/` in the same
  commit.
- Code review MUST verify conformance with this constitution.

## Governance

This constitution is the authoritative reference for all development practices
within the synthetic workload generator. It supersedes informal conventions and
ad-hoc decisions.

- **Amendments**: Any change MUST be documented with a version bump, a
  rationale, and a review of dependent artifacts (templates, specs, plans) for
  consistency. A principle weakened to accommodate an implementation is an
  amendment and MUST be visible as one.
- **Versioning**: semantic versioning.
  - MAJOR: Principle removal or backward-incompatible redefinition.
  - MINOR: New principle or materially expanded guidance.
  - PATCH: Clarifications, wording fixes, non-semantic refinements.
- **Compliance Review**: All pull requests and code reviews MUST verify
  compliance. Non-compliance MUST be resolved before merge.
- **Conflict Resolution**: If a principle conflicts with a practical
  constraint, the conflict MUST be documented, justified, and approved before
  an exception is granted.
- **Non-negotiable principles**: I and VIII. A change that violates either
  invalidates measurements taken through the tool, which is the only thing the
  tool is for. Exceptions MUST NOT be granted for these; they require an
  amendment.

**Version**: 1.0.0 | **Ratified**: 2026-09-15 | **Last Amended**: 2026-09-15
