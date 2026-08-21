# Implementation Plan: Synthetic KV Workload Generator

**Branch**: `synthetic-workload-generation` (renamed 2026-08-21 from `002-served-by-tier-attribution`; see § Branch note) | **Date**: 2026-08-10
**Spec**: `apps/workload-generator/specs/001-synthetic-workload-generator/spec.md`

**Input**: the feature specification above, its three contracts, and `research.md`.

## Summary

Turn a compact YAML statement of a workload's *statistics* into a deterministic stream of KV block
references, and drive that stream either into a Certus server or into an interchange trace file.

The generator knows nothing about storage. It has no concept of a tier, cache, memory or disk
(FR-018a): it emits which blocks are asked for, by whom, in what order, at what size, and every
question about where a block was resolved from is the consumer's to answer and report. That single
constraint is what shapes the build — it removes an entire axis from the schema, defers cache
simulation out of scope, and makes the deliverable a *reference-trace producer* rather than a
benchmark harness with a cache model inside it.

Approach: one library holding the model, the plan codec and the statistics; three thin binaries over
it, split by direction of data flow; and a phased build ordered by the spec's own P1/P2/P3 story
priorities, with the hardware phases gated on a dependency owned by another feature.

## Technical Context

**Language/Version**: Rust, edition 2021, MSRV 1.75 (per repo `CLAUDE.md`).

**Primary Dependencies**: `serde` + `serde_yaml` (schema), `serde_json` (JSONL container and
manifests), a cryptographic hash for content and stream digests, `clap` (four subcommand surfaces),
`criterion` (FR-037's no-bottleneck claim needs a benchmark, not an assertion). Feature-gated:
`arrow`/`parquet` in `workload-trace` only, default **off** (SC-012). Hardware-only: `tonic` plus
locally-declared CUDA externs in `workload-runner`.

**Note**: no hashing crate exists anywhere in the workspace today, so this is a genuinely new
dependency rather than a reuse. `contracts/plan-format.md` writes digests as `blake3:...`, which
fixes the choice unless the contract changes.

**Storage**: plain files — `events.bin` (fixed-width records), `manifest.json`, and optional
JSONL/parquet interchange. No database, no persistent state between runs.

**Testing**: `cargo test --all` for everything except the runner; single-threaded in CI per the
repo's existing convention. The FR-058a round trip is a workspace-level integration test because it
spans two binaries (FR-021j). Criterion benchmarks for the generation path.

**Target Platform**: Linux (RHEL/Fedora), x86_64. The runner additionally needs a Certus server,
CUDA, and for multi-node work a symmetric RDMA cluster.

**Project Type**: CLI tool suite over a shared library — four crates, three binaries.

**Performance Goals**: FR-037, the generator must not be the bottleneck, which at the platform's
measured ceiling means sustaining a hardware runner's request rate from pre-generated events without
allocating on the issuing path. SC-004: every FR-034a statistic over a 10^7-request plan in under one
minute on one core. FR-028: 10^7 events routine.

**Constraints**: FR-010 resident memory O(live sessions), independent of run length — which is what
makes FR-021e's unbounded runs possible at all. SC-012: the default build compiles no columnar
dependency. Repo-wide: `cargo fmt --check`, `clippy -D warnings`, `cargo doc --no-deps`
warning-free, public APIs documented with runnable examples.

**Scale/Scope**: 10^7-event plans routine; unbounded direct-to-server runs; up to a few dozen nodes;
~120 functional requirements across 8 user stories.

## Constitution Check

*GATE: must pass before Phase 0. Re-checked after Phase 1.*

Evaluated against `apps/workload-generator/.specify/memory/constitution.md` v1.0.0.

**Provenance note.** This constitution was authored for this app, not inherited. The
repository-level `.specify/memory/constitution.md` is an unfilled template, and `specify init`
scaffolds a fresh template rather than copying a parent's — which is why 7 of the 17 constitutions in
this repository are still placeholders. Ten components have filled ones, individually written; they
share a recognisable core but only two are identical. This app is not a component, so the
component-framework-conformance and interface-only-exposure principles those open with are
deliberately absent, replaced by principle I.

| Principle | Verdict | Evidence in this plan |
| --- | --- | --- |
| I. Consumer Independence *(non-negotiable)* | **PASS** | FR-018a; no `system:` section; attribution relayed not derived (FR-039d); `stats/` carries no capacity concept |
| II. Determinism and Reproducibility | **PASS** | Path-computable keys (FR-009b); fanout keyed on node not visit (FR-009e); O(live sessions) memory (FR-010) enabling unbounded runs (FR-021e) |
| III. One Definition per Statistic | **PASS** | `workload-model::stats` is the sole implementation; FR-021i forbids reimplementation. This principle is the reason the layout has a library at all |
| IV. Evidence over Assertion | **PASS** | `research.md` separates measurement from requirement; `target_occupancy = 4` labelled a judgement (FR-009g1); the diffuse-sharing confound recorded beside its result |
| V. Loud Failure over Quiet Wrongness | **PASS** | FR-015b and validation rules 17, 20-23 reject rather than warn; FR-055e refuses a partial fit; FR-039a refuses to let an absent capability read as a pass |
| VI. Code Quality and Correctness | **PASS, with an obligation** | fmt/clippy/doc standards adopted; **Criterion benchmarks are required, not optional**, because FR-037 and SC-004 are performance claims. Structural invariants (namespace disjointness, 40-byte record, digest agreement) must be *tested*, per the principle's second clause |
| VII. Documentation as Contract | **PASS** | Three contracts written before implementation; reversals marked rather than deleted throughout the clarification log |

**Platform and tooling requirements**: satisfied — three of four crates are default members, the
runner is `members` only, and `parquet` is feature-gated off (SC-012 is the measurable form of that
same requirement).

**No violations to justify.** See § Complexity Tracking for two decisions recorded as deliberate
complexity rather than as exceptions.

## Project Structure

### Documentation (this feature)

```text
apps/workload-generator/specs/001-synthetic-workload-generator/
├── spec.md                      # written
├── plan.md                      # this file
├── research.md                  # written; § Open derivations still open
├── data-model.md                # written with this plan
├── quickstart.md                # written with this plan
├── contracts/
│   ├── workload-schema.md       # written (normative YAML)
│   ├── plan-format.md           # written (events.bin + manifest)
│   └── trace-io.md              # written (trace containers, both encodings)
└── tasks.md                     # NOT created here — /speckit.tasks output
```

### Source code

```text
apps/workload-model/             # library: no binary
├── src/
│   ├── schema/                  # serde types for the YAML; unknown-field rejection; validation rules 1-23
│   ├── dist.rs                  # the one tagged-union distribution syntax
│   ├── keys.rs                  # child_id = H(parent_id, child_index, generation); private + spawn namespaces
│   ├── corpus.rs                # forest, branching profile, occupancy, churn
│   ├── session.rs               # lifecycle, turns, spawn lineage, placement
│   ├── plan/                    # events.bin codec, manifest, digests, partitioning
│   └── stats/                   # THE FR-056 statistics — single implementation
└── benches/                     # generation throughput (FR-037), statistics cost (SC-004)

apps/workload-generator/         # binary: certus-workload  (plan | report | emit)
apps/workload-trace/             # binary: certus-trace     (fit | validate | floor | convert)
                                 #   arrow/parquet behind `parquet`, default off
                                 #   eviction-replay-benchmark + the two policy components, for
                                 #   `fit --cache-curve` (FR-057d). A measuring instrument in the
                                 #   TOOL: workload-model stays free of `interfaces` (FR-018a)
apps/workload-runner/            # binary: certus-workload-run (run) — hardware only

tests/                           # workspace-level: the FR-058a round trip crosses two binaries
```

**Structure Decision**: one library, three binaries, per spec § Scope and boundary. Two properties
drive it and neither is aesthetic:

1. **`stats/` must have exactly one implementation.** FR-056's four statistics are computed over real
   traces by `fit` and over generated plans by `report` and `validate`. Two implementations would
   drift, and then a `validate` comparing a fitted model against the trace it was fitted from would
   compare two different definitions of reuse distance — a comparison that fails by *appearing to
   succeed* (FR-021i). This is the single strongest argument in the layout.
2. **The generator and the trace tool point in opposite directions** — parameters→keys versus
   keys→parameters. Different inputs, different failure modes, no shared control flow.

Workspace wiring: add all four to `members`; add `workload-model`, `workload-generator` and
`workload-trace` to `default-members`; leave `workload-runner` out, following the existing pattern
and comment style already used for the SPDK crates and `remote-lookup`.

### Branch note, and the speckit setup that fixes it

The spec lives at `apps/workload-generator/specs/001-synthetic-workload-generator/` and the git branch
is `synthetic-workload-generation`. **The branch was renamed 2026-08-21 from
`002-served-by-tier-attribution`**, which is where this work began — as a dependency of that feature —
and which had become misleading once the workload generator was almost all of the branch's content. The
served-by design still rides on this branch; its spec keeps its own directory name,
`components/dispatcher/specs/002-served-by-tier-attribution/`, because that is a feature directory
rather than a branch.
The original mismatch was not merely cosmetic. The repository-level
`.specify/scripts/bash/setup-plan.sh` derives its feature directory from the branch name, so it would
have created a stray `specs/002-served-by-tier-attribution/` at the repo root and copied the template
into it. This plan was therefore written to the correct path by hand. The rename does not on its own
fix that — the branch name still differs from the directory name — and it does not need to, because
the app-local setup below removes the dependence on the branch name altogether.

**Now fixed at the source.** `apps/workload-generator` has been speckit-initialised
(`specify init . --integration claude --script sh`) with `.specify/feature.json` pinning
`specs/001-synthetic-workload-generator`, so the app-local scripts resolve correctly and independently
of the git branch — verified: `check-prerequisites.sh --json` returns this feature's directory and
detects all four Phase 0/1 documents. Use the **app-local** scripts and skills under
`apps/workload-generator/`, not the repository-level ones, for anything scoped to this feature.

**Version skew to be aware of.** The installed CLI is `specify 0.12.12.dev0`, whereas every component
in this repository was initialised with `0.5.1` (recorded in each `.specify/init-options.json`). So this
app's templates, scripts and skill set are a later generation than the rest of the repository: it gains
`speckit-converge`, `speckit-implement` and `speckit-taskstoissues`, and lacks the `speckit-drift` and
`speckit-git-*` skills the components carry from the `spec-kit-sync` extension. That extension has
deliberately **not** been installed here, since it fetches code from a URL; without it this app has no
`extensions.yml` and therefore none of the automatic `before_plan` / `after_clarify` commit hooks. That
is a difference in behaviour, not a defect — commits have been made explicitly throughout.

## Phased build order

Ordered by the spec's story priorities, with each phase independently valuable and testable.

### Phase 1 — model, plan, statistics (P1, no hardware)

`workload-model` plus `certus-workload plan` and `report`. Delivers **US1** (compact YAML to
reproducible plan) and **US2** (characterise a plan without running it).

Order within the phase matters in one place: `keys.rs` and the branching profile must precede
`stats/`, because the statistics are defined over realised key streams and there is nothing to
measure until keys exist.

Exit criteria: SC-003 (byte-identical plans from one YAML+seed), SC-005 (reuse-distance CDF matches
the analytic Zipf form), SC-004 (statistics over 10^7 events under a minute), plus every validation
rule rejecting what it should. All CI-testable.

### Phase 2 — interchange and fitting (P1/P2, no hardware)

`certus-workload emit` (JSONL) and `certus-trace` (`fit`, `validate`, `convert`). Delivers **US6**
and completes the FR-058a round trip.

**This phase has a genuine prerequisite**: `research.md` § Open derivations owes the **`branching`
segmentation rule** — what jump ratio counts as a fanout event and how boundaries are chosen when
width is noisy. `fit` cannot be implemented without it; the 1.8× threshold used in the measurements
was chosen by eye. Also owed here, and cheaper: the four per-statistic `fit`/`validate` tolerances
and their divergence measures (FR-057b), and the reuse-distance estimation method.

Exit criteria: the round trip recovers parameters within tolerance through **both** containers;
`fit` refuses a partial trace (FR-055e) and refuses parameters whose source field is `unavailable`;
SC-012 holds — the default build compiles no `arrow`.

### Phase 3 — single-node hardware measurement (P1, gated)

`certus-workload-run run`. Delivers **US3**.

**Gated on another feature**: per spec Dependencies §1, per-tier attribution needs `served_by` added
to `EntryResult`, specified in `components/dispatcher/specs/002-served-by-tier-attribution/`. Until
that lands the runner can measure throughput and latency but must report attribution as unsupported
(FR-039a). Nothing else in this phase is gated — the `rw-telemetry` Cargo change that used to be
needed went out of scope along with the byte-provenance cross-check (spec § Out of Scope).

### Phase 4 — multi-node (P2)

Placement, agent fan-out, preflight, sweeps. Delivers **US4**, **US5**, **US7**. Needs a symmetric
cluster; `preflight` refusing an asymmetric one is itself part of the deliverable (FR-049..FR-053).

Sequence within the phase: sticky placement (FR-019a) before fan-out (FR-018c), because fan-out's
defining property — an inherited prefix resident on one specific node — does not hold under
per-request placement, and validation rule 23 rejects that combination.

### Phase 5 — churn and fault injection (P3)

`corpus.trees.churn` (FR-016b..FR-016e) and membership events. Delivers **US8**. Churn is last
because it is off by default, is not fittable (FR-055d), and its occupancy interaction (FR-016e) is
easiest to get right once occupancy reporting is already trustworthy.

## Complexity Tracking

No constitution gates exist to violate (see § Constitution Check). Two decisions are nonetheless
worth recording as deliberate complexity, each with the simpler option and why it was rejected:

| Decision | Why needed | Simpler alternative rejected because |
| --- | --- | --- |
| Four crates rather than one | `stats/` must be shared by binaries that point in opposite directions; `arrow` must stay out of the default build | One crate puts `arrow` in every `cargo test --all` (SC-012 fails) or duplicates the statistics, which silently invalidates every `validate` |
| `parquet` as a default-off feature rather than excluding `workload-trace` from `default-members` | Keeps `fit` and `validate` tested by default | Excluding the crate leaves the FR-058a round trip — their strongest check — out of CI, which is worse than a slow build |

## Risks

1. **The segmentation rule blocks Phase 2.** It is analysis, not coding, and it is the one open
   derivation on the critical path. Everything else in `research.md` § Open derivations can be
   settled alongside implementation.
2. **Phase 3 is gated on another feature's proto change.** Schedulable in parallel, but the runner
   cannot demonstrate its headline capability until `served_by` lands.
3. **`fit` is validated mainly by the round trip**, whose ground truth is exact but *synthetic*. It
   proves the emitter and reader agree and the estimators invert the generator; it cannot prove the
   model resembles reality. Only fitting real traces does that, and those are external and
   non-CI (spec § Scope).
4. **Trace collections are external and unversioned.** Any fit result is reproducible only against a
   named local copy, so fit reports must record what they read (FR-055 already requires the
   `field_status` provenance half of this).
5. **Unbounded runs are the least-exercised path.** They interact with pre-generation (FR-021f),
   hashing (FR-021g) and churn's clock at once, and no CI test can run one to completion by
   definition; bound the horizon and assert invariants instead of end state.

## Phase status

- **Phase 0 (research)**: partially complete. `research.md` holds the trace measurements; its
  § Open derivations lists seven outstanding items, one of which gates Phase 2.
- **Phase 1 (design & contracts)**: complete. Three contracts written; `data-model.md` and
  `quickstart.md` written with this plan.
- **Phase 2 (tasks)**: not started — `/speckit.tasks` output, deliberately not produced here.

**Agent context update**: `.specify/scripts/bash/update-agent-context.sh` does not exist in this
repo, so step 3 of the skill's Phase 1 has nothing to run. The repo-level `CLAUDE.md` already
records the workspace conventions this feature follows; no new technology is introduced beyond the
hashing crate and the feature-gated columnar reader noted above.
