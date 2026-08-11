---
description: "Task list for the synthetic KV workload generator"
---

# Tasks: Synthetic KV Workload Generator

**Input**: design documents in `specs/001-synthetic-workload-generator/`
**Prerequisites**: `plan.md`, `spec.md`, `data-model.md`, `contracts/`, `research.md`

**Tests**: test tasks ARE included. The specification requires them explicitly — each user story
carries an "Independent Test", the Success Criteria are stated as assertions, and constitution
principle VI requires structural invariants to be tested rather than documented and performance
claims to be substantiated by Criterion benchmarks rather than asserted.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: parallelisable — different files, no dependency on incomplete work
- **[USn]**: the user story this task serves (user-story phases only)
- **Suffixed IDs** (`T012a`) are tasks added after the first pass, kept in phase order rather than
  appended out of sequence. Same convention the spec uses for `FR-009a`; it avoids renumbering.

## Path Conventions

Four crates per `plan.md` § Source code. Paths below are repo-relative.

- `apps/workload-model/` — library: schema, keys, corpus, session, plan codec, statistics
- `apps/workload-generator/` — `certus-workload` (`plan`, `report`, `emit`)
- `apps/workload-trace/` — `certus-trace` (`fit`, `validate`, `convert`)
- `apps/workload-runner/` — `certus-workload-run` (`run`), hardware only
- `tests/` — workspace-level integration tests

---

## Phase 1: Setup (Shared Infrastructure)

- [X] T001 Create the four crate skeletons with `Cargo.toml` manifests at `apps/workload-model/`, `apps/workload-generator/`, `apps/workload-trace/`, `apps/workload-runner/`
- [X] T002 Add all four to `members` in the workspace `Cargo.toml`, and add `workload-model`, `workload-generator`, `workload-trace` to `default-members`, leaving `workload-runner` out with an explanatory comment in the style of the existing SPDK entries
- [X] T003 [P] Declare `workload-model` dependencies in `apps/workload-model/Cargo.toml`: `serde`, `serde_yaml`, and `blake3` — noting this is the workspace's first hashing crate, fixed by `contracts/plan-format.md`'s `blake3:` digest prefix
- [X] T004 [P] Declare the `parquet` feature in `apps/workload-trace/Cargo.toml` with `arrow`/`parquet` as optional dependencies, **default off** (SC-012)
- [X] T005 [P] Add `criterion` as a dev-dependency and a `[[bench]]` target to `apps/workload-model/Cargo.toml`
- [X] T006 Verify `cargo build`, `cargo fmt --check` and `cargo clippy -- -D warnings` pass on the empty skeletons before any logic exists

**Checkpoint**: workspace compiles, the four crates exist, and the default build pulls in no `arrow`.

---

## Phase 2: Foundational (Blocking Prerequisites)

These are shared by every user story and nothing below can proceed without them.

- [X] T007 Implement the tagged-union distribution syntax in `apps/workload-model/src/dist.rs` — `const`, `uniform`, `normal`, `lognormal`, `exponential`, `geometric`, `zipf`, `pareto`, `empirical` — with bare scalars sugaring to `const`, and half-to-even rounding with every clamp counted rather than silently applied
- [X] T008 Implement the schema types in `apps/workload-model/src/schema/` per `contracts/workload-schema.md`, with `deny_unknown_fields` so a mistyped parameter cannot take a default (FR-005), and refusal of any `version` the generator does not implement (FR-006)
- [X] T009 Implement `extends` deep-merge in `apps/workload-model/src/schema/extends.rs`, including-document-wins on every conflicting leaf, lists replacing rather than appending (FR-004)
- [X] T010 Implement validation rules 1–23 in `apps/workload-model/src/schema/validate.rs`, returning **all** violations rather than the first
- [X] T011 [P] Implement rule 13's rejection of removed consumer-side keys (`system:`, `topology.holder_tier`) with a message naming design rule 6 and where each quantity now lives — a stale document is a likely input, not a typo
- [X] T012 Implement the three key derivations in `apps/workload-model/src/keys.rs`: `trunk_child(parent, child_index, generation)`, `private_child(parent, minting_session, i)`, `root(root_index, generation)`
- [X] T012a Implement entry size as a pure, deterministic function of key identity in `apps/workload-model/src/keys.rs` — derived from the key's own hash, never from position in the stream (FR-011) — with `corpus.block_bytes` as the distribution it draws from and the value used recorded in the report (FR-011a)
- [X] T013 Implement the `events.bin` codec in `apps/workload-model/src/plan/record.rs` — 40 bytes, little-endian, fields naturally aligned per `contracts/plan-format.md`
- [X] T014 Implement `manifest.json` write/read in `apps/workload-model/src/plan/manifest.rs`, including `plan_format` versioning and reserved-byte rejection (FR-023b)
- [X] T015 Implement content hashing and stream digests in `apps/workload-model/src/plan/digest.rs`, distinguishing a whole-plan hash from the parameter hash an unbounded run carries (FR-021g)

### Foundational tests

- [X] T016 [P] Test that `Generation(0)` reduces `trunk_child` to exactly `H(parent, child_index)`, so the no-churn default is bit-identical to a build with no churn concept (FR-008)
- [X] T017 [P] Test that `private_child` keys on the **minting** session: a spawned child passed its parent's id computes the parent's keys, and passing the reader's id instead produces different keys — the failure that would turn every fan-out into a miss storm (FR-009c)
- [X] T018 [P] Test trunk/private namespace disjointness by construction rather than by sampling (FR-007)
- [X] T019 [P] Assert `size_of::<PlanEvent>() == 40` and every field's offset and alignment as `const` assertions (`contracts/plan-format.md`)
- [X] T019a [P] Test that entry size is a pure function of key identity: the same key yields the same size across separate generation runs and across differing stream positions (FR-011). This is the invariant FR-039b's inference rests on — a non-zero `SIZE_MISMATCH` can only be read as a generator defect if this holds — and constitution principle VI requires depended-on invariants tested rather than documented
- [X] T020 [P] Test that each validation rule rejects what it should and reports **every** violation in a multiply-invalid document

**Checkpoint**: a YAML document can be parsed, merged, validated and rejected; keys derive correctly; plan records round-trip.

---

## Phase 3: User Story 1 — Compact YAML to Reproducible Plan (Priority: P1) 🎯 MVP

**Goal**: a compact YAML plus a seed produces a deterministic, content-hashed plan artifact.

**Independent test**: generate from a checked-in YAML twice and assert byte-identical output and
matching content hashes; generate with a changed seed and assert the hashes differ while the
distributional assertions still hold.

### Tests for User Story 1

- [X] T021 [P] [US1] Test byte-identical plans from the same YAML and seed, and differing hashes with equal distributional properties from a changed seed (SC-003)
- [X] T022 [P] [US1] Test that resident memory stays O(live sessions) as run length grows by an order of magnitude (FR-010)
- [X] T023 [P] [US1] Test the occupancy floor: a config whose `occupancy(p99(shared_depth))` falls below 1.0 is rejected and below 4.0 warns (validation rule 16)
- [X] T024 [P] [US1] Test that a warmup shorter than the computed session-population ramp is **rejected**, not warned (FR-015b)

### Implementation for User Story 1

- [X] T025 [US1] Implement the forest and the piecewise `branching` profile in `apps/workload-model/src/corpus.rs`, with `fanout_at`, `paths` and `occupancy`
- [X] T026 [US1] Implement randomised rounding of a non-integer fanout keyed on **node identity** rather than on the visit, so a long run is reproducible while remaining stochastic (FR-009e)
- [X] T027 [US1] Implement `branching: auto` resolution by the FR-009g closed form with `target_occupancy = 4`, recording the resolved profile in the normalised YAML
- [X] T028 [US1] Implement the session model in `apps/workload-model/src/session.rs`: sticky root binding, turns, think time, private depth, growth per turn, and the FR-014a path-depth formula
- [X] T029 [US1] Implement the session lifecycle — born, retired, private keys dead at retirement — with lifetime and live population **derived** rather than configured (FR-014b, FR-015a)
- [X] T030 [US1] Implement `open_loop` and `closed_loop` arrival with burstiness as an index of dispersion whose neutral value is 1.0 (FR-015, FR-017)
- [X] T031 [US1] Implement the mixture over one session model, with `conversation`/`one_shot`/`scan` as presets rather than schema (FR-013, FR-014)
- [X] T032 [US1] Implement plan writing in `apps/workload-model/src/plan/writer.rs`, keys of a request contiguous and in path order, timestamps non-decreasing
- [X] T033 [US1] Implement chunked look-ahead generation so an unbounded run stays flat and allocation-free with only the horizon finite, and report the horizon (FR-021f)
- [X] T034 [US1] Implement the `certus-workload plan` subcommand in `apps/workload-generator/src/main.rs`
- [X] T035 [US1] Implement `certus-workload emit` for the JSONL container, writing a synthetic `manifest.json` with `provenance: synthetic` and the full block encoding (FR-021b)
- [X] T036 [US1] Enforce the run-length rules: exactly one of `duration`/`requests`/`blocks`/`unbounded`; `blocks` required for file output; `unbounded` rejected for file output (FR-021d, FR-021e)
- [X] T037 [P] [US1] Add a Criterion benchmark for generation throughput establishing FR-037's no-bottleneck claim as a measurement

**Checkpoint**: US1 is independently usable — YAML in, reproducible plan out.

---

## Phase 4: User Story 2 — Characterise a Plan Without Running It (Priority: P1)

**Goal**: report what a workload *is*, entirely from the plan, with no consumer involved.

**Independent test**: run `report` over a checked-in pure-Zipf plan and assert the measured
reuse-distance CDF matches the analytic Zipf reuse-distance distribution within tolerance — a check
on the stream itself rather than on any model of something consuming it.

### Tests for User Story 2

- [ ] T038 [P] [US2] Test the reuse-distance CDF for a pure-Zipf plan against the analytic distribution (SC-005)
- [ ] T039 [P] [US2] Test that the compulsory-miss floor equals the miss rate at unbounded capacity and requires no capacity parameter (FR-034a)
- [ ] T040 [P] [US2] Test that identical plans consumed twice yield identical stream digests, and that a comparison between differing digests is refused (FR-036, FR-062)
- [ ] T041 [P] [US2] Test that a `scan`-shaped mixture entry shows the expected bimodality in the reuse-distance CDF
- [ ] T042 [P] [US2] Add a Criterion benchmark asserting every statistic over a 10^7-event plan completes in under one minute on one core (SC-004)

### Implementation for User Story 2

- [X] T043 [US2] Implement the reuse-distance CDF in `apps/workload-model/src/stats/reuse_distance.rs` — the primary statistic, per object and per byte
- [X] T044 [P] [US2] Implement the compulsory-miss floor in `apps/workload-model/src/stats/floor.rs`
- [X] T045 [P] [US2] Implement the prefix-sharing depth histogram in `apps/workload-model/src/stats/sharing.rs`, reporting **intended** and **realised** as two separate statistics (FR-012a), and ensure every FR-034a quantity is the realised value rather than the configured one (FR-012)
- [X] T046 [P] [US2] Implement request-length distribution and unique-keys-over-time in `apps/workload-model/src/stats/`
- [X] T047 [P] [US2] Implement realised trunk width and occupancy per depth in `apps/workload-model/src/stats/trunk.rs`
- [X] T048 [P] [US2] Implement realised working-set size over `run.wss_window` as a request count in `apps/workload-model/src/stats/wss.rs`
- [X] T049 [US2] Implement `certus-workload report` with human and JSON output, embedding the plan hash and normalised YAML (FR-047)
- [X] T050 [US2] Implement the FR-059/FR-060 warnings, including the plan-side half of the degenerate-workload check

**Checkpoint**: a workload can be characterised and rejected as degenerate before any hardware time is spent.

---

## Phase 5: User Story 3 — Single-Node Hardware Measurement with Tier Attribution (Priority: P1)

**Goal**: throughput, latency per outcome class, and per-tier hit rate against a real server.

**⚠ Externally gated**: attribution requires `served_by` on `EntryResult`, owned by
`components/dispatcher/specs/002-served-by-tier-attribution/` (spec Dependencies §1). Until it lands,
T054–T056 are implementable but T057 cannot be demonstrated. Nothing else in this phase is gated: the
`rw-telemetry` Cargo change that used to be needed went out of scope with the byte-provenance
cross-check.

**Independent test**: run against one node with a plan whose reported working-set size exceeds what
that server was configured to hold in memory, and assert every entry is attributed with
`hits + misses + errors == entries requested`, and that throughput in GB/s and keys/s matches an
independent count of what the runner sent and received.

### Tests for User Story 3

- [ ] T051 [P] [US3] Test that a server returning `SERVED_BY_UNSPECIFIED` produces "attribution unsupported by server" rather than a guessed tier or an unknown bucket (FR-039a)
- [ ] T052 [P] [US3] Test that reported GB/s and keys/s agree with an independently counted byte and request total, and that per-`served_by`-class byte totals sum to the delivered total (FR-042)
- [ ] T053 [P] [US3] Test that warmup operations are excluded from steady-state statistics and counted separately (FR-045)

### Implementation for User Story 3

- [ ] T054 [US3] Implement the batched gRPC lookup path in `apps/workload-runner/src/main.rs` using one process-wide CUDA allocation addressed per entry via `IpcHandle.offset`, with no host/device copy on the measured path (FR-030, FR-031)
- [ ] T055 [US3] Implement the key-prefixed zero-padded payload with an opt-in value check, filling the device buffer once at startup (FR-011b)
- [ ] T056 [US3] Implement populate-on-miss with populate cost accounted separately from lookup cost (FR-032), and the explicit connection-warm phase outside the measured window (FR-033)
- [ ] T057 [US3] Implement `served_by` relay and aggregation — **verbatim, never derived** — with per-outcome-class latency percentiles and the hits+misses+errors identity (FR-039, FR-039d, FR-041)
- [ ] T058 [US3] Implement harness self-overhead measurement, flagging any run where overhead could account for more than 5% of the figure (FR-038)
- [ ] T058a [US3] Implement cumulative open-loop schedule-lag reporting, and refuse to present a configured offered rate as achieved when the schedule slipped (FR-061). This is the measurement-integrity counterpart of FR-009h: a count-based window exists precisely because a time window drifts when the schedule slips, so the slip must be visible
- [ ] T059 [US3] Implement report output: throughput in GB/s and keys/s from the runner's own counts, byte totals **per `served_by` class** as arithmetic over labelled data, and byte hit rate with the FR-040 qualification that it carries no independent information at constant `block_bytes` (FR-042). Report **no** eviction counts, promotion traffic or byte provenance — all out of scope, since each needs a model of the consumer's internals

**Checkpoint**: single-node measurement works; attribution is present or honestly absent.

---

## Phase 6: User Story 4 — Multi-Node Remote-Lookup Measurement (Priority: P2)

**Goal**: characterise remote lookup with representative multi-node request streams.

**Independent test**: sweep `self_affinity` 0.0→1.0 and assert the reported remote-served fraction
tracks it, with fabric bytes ~0 at 1.0.

### Tests for User Story 4

- [ ] T060 [P] [US4] Test that under sticky placement no session remotely fetches a key only it has asked for — so measured remote traffic is cross-session traffic and nothing else (FR-019a)
- [ ] T060a [P] [US4] Test that the measured remote-served fraction tracks configured `self_affinity` across a 0.0→1.0 sweep, and that at 1.0 fabric bytes in the measured window are ~0 (SC-006)
- [ ] T061 [P] [US4] Test that a spawned child hits rather than misses on its inherited prefix when the parent's turns have already completed (FR-018d)
- [ ] T062 [P] [US4] Test that `spawn` and `self_affinity` together are attributed separately rather than as one aggregate remote fraction
- [ ] T063 [P] [US4] Test that validation rejects a fan-out with nowhere to go, a half-configured fan-out, and fan-out combined with per-request placement (rules 22, 23)

### Implementation for User Story 4

- [ ] T064 [US4] Implement session-sticky placement as the default, with `per_request` available for deliberate comparison (FR-019a)
- [ ] T065 [US4] Implement `self_affinity`, `replication.nodes_per_key` and `cold_fraction` as properties of the request streams, expressing nothing about where copies live (FR-018)
- [ ] T066 [US4] Implement agent fan-out in `apps/workload-model/src/session.rs`: spawn at a drawn turn, children inheriting the parent's prefix and placed on other nodes (FR-018c)
- [ ] T067 [US4] Implement lineage-scoped lifetime — a parent's private keys live until the parent and every descendant has retired (FR-018d)
- [ ] T068 [US4] Implement per-node plan partitioning with each node verifying the plan hash, or the parameter hash for an unbounded run, before executing its slice (FR-026, FR-021g)
- [ ] T068a [US4] Implement a cross-node start barrier so every node shares one plan time origin (FR-054). Without it each node's `t_ns` is relative to its own start and no cross-node timing comparison means anything, which would silently undermine every multi-node latency figure
- [ ] T069 [US4] Implement remote-class reporting split by first touch versus repeat, with drift over a run reported as observed rather than as regression (FR-039c)

**Checkpoint**: multi-node measurement works with both diffuse and fan-out shapes.

---

## Phase 7: User Story 5 — Cluster Symmetry Preflight (Priority: P2)

**Goal**: refuse a comparative measurement on an asymmetric cluster, naming what differs.

**Independent test**: introduce a deliberate asymmetry and assert `preflight` refuses and names it.

- [ ] T070 [P] [US5] Test that preflight refuses on each asymmetry class and names the differing attribute (FR-049..FR-053)
- [ ] T071 [US5] Implement node inspection — NIC port speed, GPU model, NVMe count, hugepage capacity, `memlock` limit, Certus build identity — in `apps/workload-runner/src/preflight.rs`
- [ ] T072 [US5] Implement clock-skew bounding against `run.clock_skew_bound`
- [ ] T073 [US5] Implement the refusal path so an asymmetric cluster is a loud, actionable error rather than a silent confound, **and** the `NON-COMPARABLE` marking that SC-009 requires on any report nonetheless produced from one — refusing to run and marking a report that exists anyway are two different protections and SC-009 needs both

---

## Phase 8: User Story 6 — Fit a Model from a Real Trace (Priority: P2)

**⚠ Blocked on research**: T074 owes the `branching` segmentation rule. `fit` cannot be implemented
without it — the 1.8× fanout threshold in `research.md` was chosen by eye, not derived.

**Goal**: a real trace produces a YAML whose synthetic output statistically resembles it.

**Independent test**: the FR-058a round trip — generate from a known YAML, emit, re-fit, and compare
recovered parameters against the originals. Ground truth is exact, so any divergence is a defect in
`fit`, the emitter or the reader.

### Research

- [ ] T074 [US6] **Derive the `branching` segmentation rule** in `research.md`: what jump ratio counts as a fanout event, how boundaries are chosen when width is noisy, and how the FR-055c near-root boundary interacts with it
- [ ] T075 [P] [US6] Derive the four per-statistic `fit`/`validate` tolerance defaults and each statistic's divergence measure, the four being on different scales (FR-057a, FR-057b)
- [ ] T076 [P] [US6] Derive the reuse-distance estimation method and the significance-testing approach behind `repeat: 8`

### Tests for User Story 6

- [ ] T077 [P] [US6] Test the round trip end to end through **both** containers (FR-058a, FR-021j)
- [ ] T078 [P] [US6] Test that the same trace content in parquet and in JSONL yields identical fits — the container is not information
- [ ] T079 [P] [US6] Test that `fit` refuses a partial trace, naming records-consumed against records-declared (FR-055e)
- [ ] T080 [P] [US6] Test that `fit` refuses a parameter whose source field is `unavailable` rather than defaulting it, and leaves churn and placement unset (FR-055, FR-055d, FR-019b)

### Implementation for User Story 6

- [ ] T081 [US6] Implement the trace reader in `apps/workload-trace/src/read/`, detecting the population pattern per trace and **normalising to full block lists on ingest** so the delta/full branch cannot leak into each statistic
- [ ] T082 [US6] Implement manifest interpretation — `source_class`, `id_semantics`, `field_status`, `block_stats` — treating `supports.P` as undocumented and depending on nothing from it
- [ ] T083 [US6] Implement the parquet reader and writer behind the `parquet` feature in `apps/workload-trace/src/parquet.rs`
- [ ] T084 [US6] Implement `certus-trace convert` for events.bin → parquet (FR-021h)
- [ ] T085 [US6] Implement `certus-trace fit`, including the FR-055c root-boundary rule and its reported boundary depth
- [ ] T086 [US6] Implement `certus-trace validate` for plan-vs-plan and plan-vs-trace, taking every statistic from `workload-model::stats` and implementing none (FR-021i)
- [ ] T087 [US6] Implement the fit report: per-statistic divergence, tolerances used, provenance of `reconstructed` versus `native` fields, and order-dependence marking for traces without timestamps

**Checkpoint**: a real trace produces a model, and the round trip proves the tools agree with each other.

---

## Phase 9: User Story 7 — Parameter Sweeps with Statistical Reporting (Priority: P2)

- [ ] T088 [P] [US7] Test that a two-point sweep at `repeat: 8` yields a significance verdict and that arms with differing stream digests are refused (SC-010)
- [ ] T089 [US7] Implement the sweep matrix in `apps/workload-model/src/sweep.rs` — cartesian product over dotted paths, seeds derived deterministically from the root seed
- [ ] T090 [US7] Implement `order: interleaved` as the default so environmental drift does not alias onto one sweep point
- [ ] T091 [US7] Implement significance reporting at `repeat: 8`, with the rejection of a capacity or policy axis falling out of rule 14 rather than being special-cased

---

## Phase 10: User Story 8 — Fault and Churn Injection (Priority: P3)

- [ ] T092 [P] [US8] Test that churn's `generation` term rotates a node's whole subtree implicitly and that a rotation produces the compulsory-miss shock (FR-016b, FR-016d)
- [ ] T093 [P] [US8] Test the churn-adjusted occupancy floor, and that a half-life shorter than warmup or set without a `duration` is rejected (FR-016e, rules 17, 18)
- [ ] T094 [US8] Implement `corpus.trees.churn` with the FR-008 generation term, per-node advance from node identity and seed, and per-segment half-life override
- [ ] T094a [US8] Implement non-stationary root popularity via `drift.half_life`, 0 meaning stationary (FR-016), keeping it strictly separate from churn: drift re-weights **which** shared keys are popular and leaves a consumer's cached entries valid, whereas churn changes **which shared keys exist** and invalidates them (FR-016a). One half-life covering both would mean two physically different things
- [ ] T095 [US8] Implement rotation-event and compulsory-miss-shock reporting, with FR-060's floor accounting for churn-induced misses
- [ ] T096 [US8] Implement scheduled membership events (`stop`, `start`) at absolute plan times (FR-021)

---

## Phase 11: Polish & Cross-Cutting Concerns

- [ ] T097 [P] Write doc comments with runnable examples for every public `workload-model` API and verify `cargo doc --no-deps` is warning-free
- [ ] T098 [P] Ship the presets named in `contracts/workload-schema.md` under `apps/workload-generator/presets/`
- [ ] T098a [P] Implement the optional human-readable plan trace at `run.emit_trace`, for debugging only, and assert it is never accepted as an input (FR-029)
- [ ] T099 [P] Verify SC-012 in CI: `cargo test --all` compiles no columnar dependency while still exercising every statistic, all of `fit`, and a full round trip through JSONL
- [ ] T100 [P] Add a `--features parquet` CI job covering the container path only
- [ ] T101 [P] Verify SC-001: a realistic sharing workload in under 60 lines of YAML, and a variation in under 10 using `extends`
- [ ] T102 Update `research.md` § Open derivations as each item is discharged, and close out the remaining one — the occupancy-bound derivation (FR-009f/FR-009g). The `GetIoStats` cross-check tolerance that used to be listed there is discharged by removal
- [ ] T103 Run `component-check-leakage` and the repo's doc-sync skills, then re-verify `fmt`, `clippy -D warnings` and `cargo doc` across all four crates

---

## Dependencies & Execution Order

### Phase dependencies

- **Setup (Ph 1)** → blocks everything
- **Foundational (Ph 2)** → blocks every user story
- **US1 (Ph 3)** → blocks US2 (nothing to characterise without a plan), US3, US4, US6
- **US2 (Ph 4)** → blocks US6's validate comparisons (shared statistics)
- **US3 (Ph 5)** → blocks US4; externally gated on `served_by`
- **US4 (Ph 6)** → blocks US5 in practice (preflight exists to protect multi-node runs)
- **US6 (Ph 8)** → blocked internally on T074
- **US7, US8** → independent of each other once US1 and US2 land
- **Polish (Ph 11)** → last

### Note on ordering versus `plan.md`

`plan.md` sequences interchange and fitting *before* hardware, because US3 is gated on another
feature's proto change. This file orders phases by story priority as the task workflow requires. Both
are correct: **US6 may be worked in parallel with, or ahead of, US3** whenever `served_by` has not
landed. The dependency graph above is authoritative for what actually blocks what.

### Parallel opportunities

- T003–T005 in setup
- T011 alongside T010; T016–T020 all parallel once T012–T015 land
- Within US2, T044–T048 are independent statistics in separate files
- Within US6, T075 and T076 are independent of T074 and of each other

---

## Parallel Example: User Story 2

```text
# The statistics are independent modules; launch together once T043 defines the shared traversal:
T044  compulsory-miss floor      → src/stats/floor.rs
T045  sharing depth histogram    → src/stats/sharing.rs
T046  request length + unique-keys-over-time
T047  trunk width and occupancy  → src/stats/trunk.rs
T048  working-set size           → src/stats/wss.rs
```

---

## Implementation Strategy

### MVP first

Phases 1–3 (Setup, Foundational, US1) deliver the MVP: a compact YAML becomes a reproducible,
content-hashed plan. That is independently valuable — it is the artifact every other phase consumes,
and it is fully CI-testable with no hardware.

### Incremental delivery

1. **Ph 1–3** → reproducible plans (MVP)
2. **+ Ph 4** → workloads characterisable before any hardware time is spent
3. **+ Ph 8** → fitting and the round trip, the strongest check on the tools
4. **+ Ph 5** → single-node hardware measurement
5. **+ Ph 6–7** → multi-node and preflight
6. **+ Ph 9–10** → sweeps, churn, faults

Steps 1–3 need no hardware and no external dependency, which is most of the feature's value.

### Recommended sequencing note

Do not defer T037 and T042 — the two Criterion benchmarks — to the polish phase. FR-037 and SC-004
are performance *claims*, and constitution principle VI requires them substantiated by measurement
rather than assertion. A benchmark added after the fact tends to be shaped to pass.

---

## Notes

- Every task names a file path; `[P]` means genuinely different files with no incomplete dependency.
- Tests are included because the specification requires them, not by default.
- **T074 is on the critical path for US6** and is analysis rather than coding — schedule it early.
- **T057 is externally gated**; the rest of US3 is not.
- Commit per task or per small group. Verify `fmt`, `clippy -D warnings` and `cargo doc` at each
  checkpoint rather than at the end.
