# Implementation Plan: Synthetic Workload Generator

**Feature Directory**: `specs/001-synthetic-workload-generator` | **Date**:
2026-09-15 | **Spec**: [spec.md](./spec.md)

**Git Branch**: `synthetic-workload-generator-rewrite` (independent of the
feature directory name)

**Input**: Feature specification from
`apps/workload-generator/specs/001-synthetic-workload-generator/spec.md`

## Summary

Build a measurement instrument that simulates LLM KV-cache workloads in virtual
time and either drives them into Certus or writes them to a trace file. The
governing design decision is that the **simulation core is a CUDA-free
library** that both execution paths consume, which makes FR-072's "the plan is
identical across live and emit runs" a structural property rather than an
honour-system one, and lets the whole determinism and distribution test surface
run with no accelerator and no server.

Five crates: three CUDA-free libraries that are workspace default members
(`workload-model`, `workload-trace`, `workload-wire`) and two CUDA-linked
binaries that are not (`workload-gen`, `workload-node-agent`), following the
precedent of `apps/remote-lookup-bench`. Remote nodes are reached over plain
pipelined TCP, which the arithmetic in `research.md` D2 shows has an order of
magnitude of headroom because only keys cross the wire.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75 (workspace-wide)

**Primary Dependencies**: `clap` 4 (repo convention), `serde` + `serde_yaml`
0.9 (matching `apps/certus-server-yaml`), `rand` 0.8 + **`rand_chacha`** for
reproducible sampling, `parquet` **behind a non-default feature**,
`hdrhistogram` (binary only), `shm-queue` + `shmq-dispatcher` (live path only),
`criterion` for benchmarks. No hashing crate — key derivation is a specified
splitmix64 chain.

**Storage**: Output files only — a trace directory (JSONL and/or parquet with a
`manifest.json`) and a plan serialisation. No database, no persistent state;
the node agent holds none by design.

**Testing**: `cargo test` — unit, doc, and integration. The three library
crates are default members, so `cargo test --all` covers the simulation core,
distributions, key chaining, trace writing, and wire framing with no hardware.
Criterion benchmarks for the per-key plan-generation path.

**Target Platform**: Linux, x86-64 only. The mailbox transport depends on
x86-TSO store ordering and shared futexes.

**Project Type**: CLI tool plus a companion daemon, driving an external
service.

**Performance Goals**: Per-key plan generation far below the measured 5.6–14
µs/key end-to-end cost of the path it feeds, so the plan queue never reaches
zero (SC-011). Transport headroom: 1M keys/s at batch 64 needs 0.78 batches in
flight at a 50 µs round trip; depth 8 is the default.

**Constraints**: The generator must never be the bottleneck, asserted per run
rather than assumed (Principle I). No per-operation work proportional to
payload size. Latency instrumentation per request, never per key. Lane count
must not exceed the node's channel count, since the mailbox is depth-1 per
channel.

**Scale/Scope**: 10,000 concurrent sessions, 10,000,000 live keys, runs of
hours (SC-012). Live-key tracking must stay within a few hundred megabytes,
which rules out per-key allocation. Per-session state is linear in turn count
while per-turn work is linear in turn index, so total work is quadratic in
session length.

## Constitution Check

*GATE: must pass before Phase 0. Re-checked after Phase 1 — see the bottom of
this section.*

| Principle | Gate | Status |
| --- | --- | --- |
| **I. Client-Protocol Conformance** (NON-NEGOTIABLE) | Op stream is exactly the production client's; reserve→transfer→commit, no single-shot store, reference reports without promotion, events polled, no removal/pinning/promotion | **PASS** — `data-model.md` fixes the operation kinds; `quickstart.md` Scenario 4 checks the stream against the production client's own sequence |
| **II. Interface-Only Consumption** | Reach Certus only through the mailbox transport; declare foreign symbols locally rather than depending on component crates | **PASS** — depends only on `shm-queue` and `shmq-dispatcher`; CUDA symbols declared locally per the `remote-lookup-bench` precedent; `zyre` rejected partly on this ground (`research.md` D2) |
| **III. Code Quality and Correctness** | fmt, clippy `-D warnings`, warning-free docs, `// SAFETY:` on unsafe, Linux-only | **PASS** — planned as CI gates; the only unsafe is the locally declared CUDA FFI |
| **IV. Comprehensive Testing** | Unit + doc tests; wire contract tests; statistical tests; plan determinism; no hardware dependency; single-threaded-safe | **PASS** — three default-member crates carry it; conformance list in `contracts/node-agent-wire.md`, determinism in `quickstart.md` Scenario 2 |
| **V. Performance Validation** | Criterion benchmarks; no per-op work proportional to payload; plan-generation cost benchmarked; n ≥ 8 for hardware claims | **PASS** — benchmark on the per-key path; payload is a pre-filled reusable buffer per `contracts/node-agent-wire.md` |
| **VI. Documentation Standards** | Doc comments with runnable examples; warning-free `cargo doc`; input schema documented in contracts and the example validated as a test | **PASS** — `contracts/workload-input.example.yml` is exercised by Scenario 1 |
| **VII. Maintainability** | YAGNI; minimal justified dependencies; explicit `Result`; readable structure | **PASS with one item to watch** — five crates is the largest structure here, justified below; `parquet` is a genuinely new dependency and is justified and feature-gated in `research.md` D4 |
| **VIII. Measurement Validity** (NON-NEGOTIABLE) | Plan-queue depth asserted per run; virtual clock holds on backpressure; plan reproducible; races preserved and their consequence documented | **PASS** — exit code 3 puts invalidity in the process status, not only the report (`contracts/cli.md`); FR-072 enforced by the crate boundary |
| **IX. Specified Statistical Machinery** | Inverse-transform truncation; half-open integer bounds; residual-life seeding; effective distributions reported; claims traceable | **PASS** — all four are invariants in `data-model.md`; the population measurements are reproducible in `research/population/`, which is also where the controller rationale's own retraction is recorded |

**Five crates versus YAGNI (Principle VII).** Justified rather than waved
through: the split is not modularity for its own sake, it is what makes two
requirements mechanical instead of aspirational. A single crate would put the
simulation core behind CUDA linkage, so User Story 2's "no accelerator"
independent test could not run, and FR-072 would rely on nobody writing a
second simulation path. Each crate boundary corresponds to a real constraint:
CUDA-free testability, default-member membership, and the feature gate that
keeps `parquet` out of the default build.

**Post-Phase-1 re-check**: no new violations. Phase 1 added two contracts
(`key-derivation.md`, `node-agent-wire.md`) that *strengthen* Principles VIII
and IX by pinning the key function with test vectors and making the
build-identity handshake fail-closed.

## Project Structure

### Documentation (this feature)

```text
specs/001-synthetic-workload-generator/
├── spec.md                              # complete, committed (a738ef39)
├── specify-prompt.md                    # provenance for spec.md
├── plan.md                              # this file
├── research.md                          # Phase 0: nine decisions (D8 added in Phase 1)
├── data-model.md                        # Phase 1: entities and invariants
├── quickstart.md                        # Phase 1: six validation scenarios
├── checklists/requirements.md           # spec quality, 16/16
├── contracts/
│   ├── workload-input.example.yml       # NORMATIVE input schema
│   ├── key-derivation.md               # Phase 1: keys, with test vectors
│   ├── node-agent-wire.md              # Phase 1: TCP protocol
│   ├── trace-io.md                     # Phase 1: the emitted trace
│   ├── trace-interop.md                # Phase 1: convert targets, verified
│   └── cli.md                          # Phase 1: subcommands, reports, exit codes
└── tasks.md                             # Phase 2 — NOT created by /speckit-plan
```

### Source Code (repository root)

```text
apps/workload-generator/
├── crates/
│   ├── workload-model/                  # default member, CUDA-free
│   │   ├── src/
│   │   │   ├── description.rs           # YAML schema, validation, effective report
│   │   │   ├── distribution.rs          # the five kinds, truncation, integral draws
│   │   │   ├── pool.rs                  # exact/poisson populations, residual seeding
│   │   │   ├── selection.rs             # index distributions, slot vs recency
│   │   │   ├── keys.rs                  # splitmix64 chain (contract + test vectors)
│   │   │   ├── session.rs               # sessions, turns, canonical ordering
│   │   │   ├── sim.rs                   # virtual-time event loop
│   │   │   └── plan.rs                  # OperationPlan, canonical serialisation
│   │   ├── tests/                       # determinism, statistics, chain nesting
│   │   └── benches/                     # per-key plan-generation cost
│   ├── workload-trace/                  # default member, CUDA-free
│   │   ├── src/
│   │   │   ├── manifest.rs              # self-describing manifest, block_id_space
│   │   │   ├── jsonl.rs
│   │   │   ├── parquet.rs               # behind the `parquet` feature
│   │   │   └── simulator.rs             # the D1 projection
│   │   └── tests/                       # container equivalence, schema invariants
│   ├── workload-wire/                   # default member, CUDA-free
│   │   ├── src/{frame.rs,client.rs,server.rs}
│   │   └── tests/                       # round-trip, oversize, truncation, Hello
│   ├── workload-gen/                    # NOT a default member (CUDA)
│   │   └── src/{main.rs,cli.rs,live.rs,lanes.rs,cuda.rs,report.rs}
│   └── workload-node-agent/             # NOT a default member (CUDA)
│       └── src/{main.rs,agent.rs,payload.rs}
└── specs/001-synthetic-workload-generator/
```

**Structure Decision**: Crates live under `apps/workload-generator/crates/` so
the feature's speckit directory and its code sit together, and the three
library crates join `default-members` in the root `Cargo.toml` while the two
binaries are added as plain members. `workload-gen` carries a default-on `live`
feature; building with `--no-default-features` yields an emit-only tool with no
CUDA and no mailbox dependency, which is what makes User Story 2 and quickstart
Scenarios 1–3 runnable anywhere.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
| --- | --- | --- |
| Five crates rather than one | A crate boundary is the only mechanism that makes FR-072 (identical plan across live and emit) enforceable rather than aspirational, and that keeps the simulation core CUDA-free so US2's no-accelerator test can run | One crate puts the core behind CUDA linkage: US2 becomes untestable without CUDA present, and FR-072 becomes an honour-system property that a second execution path could break invisibly |
| New `parquet` dependency (nothing in the repo uses it today) | FR-055 makes parquet a required output container, and the trace's block-list columns repeat the whole prefix per turn, which is exactly where columnar compression earns its place | Hand-rolling a writer is not serious; a Python converter would make SC-004's "identical records" assertion span two runtimes instead of one test. Mitigated by a non-default feature so the default workspace build never pulls arrow |
| `rand_chacha` alongside `rand` | `StdRng` and `SmallRng` are explicitly not reproducible across `rand` releases, and FR-012/FR-034 require a seed to reproduce a plan | Using `StdRng` would appear to work and silently break reproducibility on a dependency bump — the failure mode is a plan that no longer matches its own recorded seed |

## Phase Status

- [x] **Phase 0** — `research.md`: eight decisions, all checked against the
  repository. Zero `NEEDS CLARIFICATION` remaining. The one item clarify
  deferred (simulator consumability) is settled in D1.
- [x] **Phase 1** — `data-model.md`, `contracts/key-derivation.md`,
  `contracts/node-agent-wire.md`, `contracts/cli.md`, `quickstart.md`, and —
  added during implementation, see `research.md` D8 — `contracts/trace-io.md`
  and `contracts/trace-interop.md`.
- [ ] **Phase 2** — `tasks.md`, via `/speckit-tasks`. Not created here.
