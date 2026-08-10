# Implementation Plan: Synthetic KV Workload Generator

**Branch**: `002-served-by-tier-attribution` (see § Branch note) | **Date**: 2026-08-10
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

**The constitution cannot be evaluated: `.specify/memory/constitution.md` is an unfilled template.**
Every principle is still a `[PRINCIPLE_N_NAME]` / `[PRINCIPLE_N_DESCRIPTION]` placeholder, the
governance section is `[GOVERNANCE_RULES]`, and the version is `[CONSTITUTION_VERSION]`. There are
therefore no gates to pass or violate, and reporting a pass would be reporting on nothing.

Rather than invent principles, this plan is checked against the repo's **actual** written standards
in `CLAUDE.md`, which is what the codebase is really held to:

| Standard (`CLAUDE.md`) | Status | Note |
| --- | --- | --- |
| `rustfmt` default formatting | **Will comply** | No deviation needed |
| `clippy -D warnings` | **Will comply** | Warnings are errors |
| Public APIs documented, runnable examples, `cargo doc --no-deps` clean | **Will comply** | `workload-model` is the public surface and carries the doc burden |
| Criterion benchmarks for performance-sensitive code | **Required here** | FR-037 and SC-004 are performance claims; they need benchmarks, not assertions |
| `unsafe` requires `// SAFETY:` justification | **Expected to be N/A** | No `unsafe` is anticipated outside the runner's CUDA externs |
| Default build excludes SPDK/hardware crates | **Complies, and extends it** | Three of four crates are default members; the runner is `members` only, and SC-012 adds the same discipline for `arrow` |
| Components accessed only through interfaces | **N/A by construction** | FR-034 removed the only interface dependency (`IEvictionPolicy`) when cache simulation was deferred |

**Recommendation, outside this plan's scope**: fill in the constitution, or delete it. An unfilled
template silently disables a gate that every `/speckit.plan` run in this repo claims to check.

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
apps/workload-trace/             # binary: certus-trace     (fit | validate | convert)
                                 #   arrow/parquet behind `parquet`, default off
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

### Branch note

The spec lives at `apps/workload-generator/specs/001-synthetic-workload-generator/` while the current
branch is `002-served-by-tier-attribution`, because this work began as a dependency of that feature.
`.specify/scripts/bash/setup-plan.sh` derives its feature directory from the branch name and would
therefore have created a stray `specs/002-served-by-tier-attribution/` at the repo root and copied
the template into it; this plan was written to the correct path by hand instead. `apps/workload-generator`
is not speckit-initialised (no `.specify/`), so nothing pins the mapping. Running the
`tools-speckit-init` skill against `apps/workload-generator` would fix that durably; until then every
speckit command in this feature needs the same manual redirection.

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
(FR-039a). SC-007a additionally needs a `rw-telemetry` Cargo change in `certus-server-yaml`
(Dependencies §2) and is separate from SC-007 for exactly that reason.

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
