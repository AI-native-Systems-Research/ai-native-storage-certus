# Tasks: Synthetic Workload Generator

**Input**: Design documents from `specs/001-synthetic-workload-generator/`

**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`,
`contracts/` (all present)

**Tests**: **Included, not optional.** Constitution Principle IV is a gate, and
`data-model.md` closes with six cross-cutting invariants that each require
their own test. Where a contract already specifies normative values — the
key-derivation test vectors, the wire conformance list — the test is written
*first*, because the expected values exist before the code does.

**Organization**: Tasks are grouped by user story. Note the shape of this
feature before reading further: **its substance is the simulation core**, which
every story needs, so Phase 2 is unusually heavy and the story phases are
comparatively thin. The critical path is Phase 2; story parallelism buys less
here than the template's default assumption.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on incomplete
  work)
- **[Story]**: `US1`–`US4`, matching `spec.md`
- Paths are relative to `apps/workload-generator/`

## Path Conventions

Per `plan.md`, five crates under `apps/workload-generator/crates/`:

| Crate | CUDA | Default member |
| --- | --- | --- |
| `workload-model` | no | yes |
| `workload-trace` | no | yes |
| `workload-wire` | no | yes |
| `workload-gen` | yes (behind `live`) | no |
| `workload-node-agent` | yes | no |

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Crate skeletons, dependencies, and the build split that the rest
depends on.

- [x] T001 Create the five crate skeletons under `crates/` and register them in
  the root `Cargo.toml`: add `workload-model`, `workload-trace`,
  `workload-wire` to both `members` and `default-members`; add `workload-gen`
  and `workload-node-agent` to `members` only, following
  `apps/remote-lookup-bench`
- [x] T002 [P] Declare dependencies per `plan.md` Technical Context in each
  `crates/*/Cargo.toml`: `clap` 4 with `derive`, `serde` + `serde_yaml` 0.9,
  `rand` 0.8 + `rand_chacha`, `criterion` for benches
- [x] T003 [P] Add the `parquet` dependency to
  `crates/workload-trace/Cargo.toml` behind a **non-default** `parquet`
  feature, so the default workspace build never pulls arrow (`research.md` D4)
- [x] T004 Add the default-on `live` feature to
  `crates/workload-gen/Cargo.toml`, gating `shm-queue`, `shmq-dispatcher`, and
  the CUDA link, and confirm `cargo build -p workload-gen
  --no-default-features` succeeds on a machine with no CUDA
- [x] T005 [P] Verify the repo quality gates pass on the empty skeletons:
  `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo doc --no-deps`,
  `cargo test --all -- --test-threads 1`

**Checkpoint**: the workspace builds, and the CUDA-free/CUDA split is real
rather than intended.

**Two pre-existing gate failures found at T005, neither caused by this feature
— do not chase them from inside it, and expect them again at T086:**

1. `cargo test --all` fails in `dispatcher-p2p` with `libgdrapi.so.2: cannot
   open shared object file`. `--all` is `--workspace`, which *overrides*
   `default-members` and selects every member, so it reaches SPDK/GDRCopy
   crates. The gate this feature can actually hold is plain `cargo test`
   (default members) plus `-p` on the two non-default-member crates.
2. `cargo clippy -p workload-gen -- -D warnings` fails on **two
   `manual_checked_ops` lints in `components/interfaces`**, a lint new in this
   toolchain (rustc 1.96). It is latent, not ours: it fires only when
   `interfaces/spdk` is enabled, which `shmq-dispatcher` does, so
   `cargo clippy -p shmq-dispatcher` and `cargo clippy -p remote-lookup-bench`
   — the precedent crate — fail identically on an unmodified tree. Our own
   crates are clean: `cargo clippy -p workload-gen --no-default-features`
   passes, as does the whole default-member workspace.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: `workload-model` — the CUDA-free simulation core that both
execution paths consume. This is what makes FR-072 structural rather than
aspirational, so nothing here may know about mailboxes, CUDA, sockets, or
output containers.

**⚠️ CRITICAL**: no user story can begin until this phase is complete.

### Distributions and sampling

- [ ] T006 Implement the `Distribution` enum and sampling in
  `crates/workload-model/src/distribution.rs` for `constant`, `uniform`,
  `empirical`, `normal`, `exponential`, drawing from `rand_chacha::ChaCha20Rng`
  — **not** `StdRng`, which is not reproducible across `rand` releases
- [ ] T007 Implement inverse-transform truncation between `F(min)` and `F(max)`
  in `crates/workload-model/src/distribution.rs` — a real truncation, never
  clamping
- [ ] T008 Implement integral draws in
  `crates/workload-model/src/distribution.rs`: truncate on `[min − 0.5, max +
  0.5]` and round to nearest, so every integer in range carries equal weight
- [ ] T009 Implement `empirical` from inline samples and from a file, with
  `interpolate: false` as the default for counts, in
  `crates/workload-model/src/distribution.rs`
- [ ] T010 Accept `inf` in any numeric position in
  `crates/workload-model/src/distribution.rs`
- [ ] T011 Implement effective-mean and quantile reporting after truncation in
  `crates/workload-model/src/distribution.rs`, the value FR-003 reports and
  FR-004 gates on
- [ ] T012 [P] Tests in `crates/workload-model/tests/distribution.rs`:
  truncation is not clamping (no mass piled at bounds); `uniform{0,5}` as a
  count yields six equiprobable values rather than half-weight endpoints; a
  fixed seed reproduces a draw sequence exactly; `empirical` discrete never
  emits a value outside its samples while interpolated does

### Key derivation (test vectors first)

- [x] T013 [P] Write the key-derivation tests **before** the implementation, in
  `crates/workload-model/tests/keys.rs`, asserting the three normative
  `splitmix64` vectors from `contracts/key-derivation.md` (`splitmix64(0) =
  e220a8397b1dcdaf`, `splitmix64(1) = 910a2dec89025cc1`, `splitmix64(u64::MAX)
  = e4d971771b652c20`)
- [x] T014 Implement `splitmix64`, the chain `key(parent, salt) =
  splitmix64(splitmix64(parent) ^ salt)`, and the tag-partitioned salt space in
  `crates/workload-model/src/keys.rs` per `contracts/key-derivation.md`. No
  hashing crate: `DefaultHasher` and `ahash` are unstable across versions and
  would break cross-node key consistency
- [x] T015 Pin the chain test vectors in `crates/workload-model/tests/keys.rs`
  — once committed these values must never change, since changing them
  invalidates every previously generated trace

**Found while implementing T014 — the salt layout imposes hard ceilings the
contract does not state, and T017/T033 must enforce them at load time.** The
salt is assembled with **XOR** over fixed-width fields, so a field that
overflows does not saturate: it corrupts its neighbour and aliases two
different blocks onto one key, which would surface only as an inexplicable
cache hit. `keys.rs` therefore asserts each field's width rather than
truncating, which converts the hazard into a loud panic. The ceilings are:

| Field | Width | Ceiling |
| --- | --- | --- |
| `class_id` | 16 bits | 65 536 shared classes |
| `instance_index` | 16 bits | 65 536 live instances per pool |
| `block_ordinal` | 16 bits | 65 536 blocks per instance, and per session's input and output stream |
| `session_id` | 32 bits | 4 294 967 296 sessions per run |

Only `block_ordinal` is reachable in practice: SC-012 targets hour-long runs,
and a long session's input stream grows every turn, so `turns ×
E[input_growth]` can pass 65 536. **A panic mid-run would destroy the run's
output**, so the description validator must refuse it up front from the
projected per-session block count, and the projection already computes exactly
that quantity. Recorded here rather than only in the code because the fix
belongs in a different task from the discovery.

### Description parsing and validation

- [ ] T016 Implement the YAML schema and parser in
  `crates/workload-model/src/description.rs`, matching
  `contracts/workload-input.example.yml`, which is normative
- [ ] T017 Implement validation in `crates/workload-model/src/description.rs`:
  reject an unknown `version`; reject a `uses` entry naming an undeclared
  class; refuse when a count distribution truncated to its pool discards more
  than 5% of its mass, naming both the requested and effective mean; reject the
  `poisson` population form for an unbounded lifetime; reject any host-specific
  or tuning field
- [ ] T018 [P] Tests in `crates/workload-model/tests/description.rs`: the
  shipped example validates and reports `long_document`'s effective count mean
  (~5.57 at pool 20); lowering that pool to 10 is **refused** naming both 5 and
  4.218

### Populations

- [ ] T019 Implement `SharedPool` with the `exact` and `poisson` forms in
  `crates/workload-model/src/pool.rs` — no feedback controller and no gain
  parameter (FR-016)
- [ ] T020 Implement equilibrium residual-life seeding in
  `crates/workload-model/src/pool.rs`, used for both forms at `t = 0`
- [ ] T021 Implement retirement and release in
  `crates/workload-model/src/pool.rs`: an expired instance becomes unselectable
  while existing users continue, and its bookkeeping is freed when the last
  user ends. Certus is told nothing
- [ ] T022 Implement uniform instance selection with a bounded index space in
  `crates/workload-model/src/selection.rs`, and report the implied working-set
  size when a pool has more than one instance and no `selection` (FR-021).
  Non-uniform selection is US4
- [ ] T023 Implement drawing without replacement bounded by `min(nominal pool
  size, live count)` in `crates/workload-model/src/selection.rs`, and count how
  often that bound binds
- [ ] T024 [P] Test in `crates/workload-model/tests/pool.rs`: residual-life
  seeding produces flat churn from `t = 0`, whereas seeding from `lifetime`
  itself produces the cohort artifact — zero churn early then a burst, with
  periodic modulation persisting for several generations

### Sessions, turns, and the plan

- [ ] T025 Implement `SessionPool` and `Session` in
  `crates/workload-model/src/session.rs`, with `pool.size` meaning concurrent
  sessions so arrival rate is an output
- [ ] T026 Implement canonical ordering in
  `crates/workload-model/src/session.rs`: shared classes in declaration order,
  instances sorted by index within a class
- [ ] T027 Implement the append-only turn model in
  `crates/workload-model/src/session.rs`: turn *n* reads the whole prefix,
  mints separate input and output growth, both stored and both extending the
  chain
- [ ] T028 [P] Test in `crates/workload-model/tests/session.rs`: sessions
  drawing overlapping instance sets produce **nested** chains, not divergent
  ones — `{0,1}` versus `{0,1,4}` share the first two objects' blocks
- [ ] T029 Implement the virtual-time event loop in
  `crates/workload-model/src/sim.rs`: think time before each turn, session
  birth and death, pool events. Node placement and migration are US3
- [ ] T030 Implement `OperationPlan` and `Operation` in
  `crates/workload-model/src/plan.rs`: the operation kinds from
  `data-model.md`, totally ordered by virtual time with a deterministic
  tie-break on session id, and per-session order strict while cross-session
  operations may overlap
- [ ] T031 Implement the canonical plan serialisation in
  `crates/workload-model/src/plan.rs`, the artifact SC-003 asserts
  byte-identity against
- [ ] T032 [P] Test in `crates/workload-model/tests/determinism.rs`: the plan
  is byte-identical across repeated runs at a fixed seed, **and differs** for a
  different seed — the second half catches a seed that is not wired through,
  which otherwise looks exactly like determinism

### Projection

- [ ] T033 Implement the run projection in
  `crates/workload-model/src/project.rs`: invocations, distinct keys minted,
  key references, and uncompressed bytes, from the description's own rates,
  using a ~10k-session Monte Carlo draw for `E[K²]` since key references grow
  quadratically with session length
- [ ] T034 Add span inversion to `crates/workload-model/src/project.rs`:
  suggest a span for a target record count, and warn when a span is short
  relative to the longest finite lifetime in the description
- [ ] T035 [P] Test in `crates/workload-model/tests/project.rs`: the projection
  for the shipped example at a 1000-second span is within a stated tolerance of
  an actual emit run's counts

### Benchmarks

- [ ] T036 [P] Criterion benchmark of per-key plan-generation cost in
  `crates/workload-model/benches/plan.rs`, the measurement Principle I's
  never-the-bottleneck claim rests on
- [ ] T037 [P] Audit task: confirm each of the six cross-cutting invariants
  tabulated at the end of `data-model.md` has a named test, and record the
  mapping in a comment at the top of
  `crates/workload-model/tests/determinism.rs`

**Checkpoint**: the simulation core is complete and fully tested with no
accelerator, no server, and no network. Story work can begin.

---

## Phase 3: User Story 1 — Drive a local Certus node (Priority: P1) 🎯 MVP

**Goal**: sustained, realistic traffic into a local Certus node, with a run
report that says both what the throughput was and whether the run was valid.

**Independent Test**: start a Certus server locally, run the generator against
the shipped example with no node list, and confirm sustained traffic plus a
report carrying throughput, plan-queue depth, lane utilisation, and latency
percentiles.

- [ ] T038 [P] [US1] Declare the CUDA FFI symbols locally in
  `crates/workload-gen/src/cuda.rs` with `// SAFETY:` justifications, following
  `apps/remote-lookup-bench` rather than depending on `gpu-services`, which
  would unify cargo features across the workspace
- [ ] T039 [US1] Implement the pre-filled reusable payload buffer with an
  optional key stamp in `crates/workload-gen/src/payload.rs`. No per-operation
  byte construction (FR-038)
- [ ] T040 [US1] Implement the local mailbox client in
  `crates/workload-gen/src/live.rs` over `shm-queue`, framing with
  `shmq-dispatcher::wire`
- [ ] T041 [US1] Map plan operations to the production client's stream in
  `crates/workload-gen/src/live.rs`: reserve → transfer → commit-or-abort for
  stores, **never** the single-shot store; reference reports with `promote: 0`;
  poll for events. Never removal, pinning, unpinning, or promotion
- [ ] T042 [US1] Implement lanes in `crates/workload-gen/src/lanes.rs`: one
  session's turn at a time, per-session order strict, cross-session overlap
  allowed
- [ ] T043 [US1] Reject a lane count exceeding the node's channel count at
  startup in `crates/workload-gen/src/lanes.rs` — the mailbox is depth-1 per
  channel, so over-subscription would silently serialise
- [ ] T044 [US1] Implement the plan queue in
  `crates/workload-gen/src/lanes.rs`, produced ahead of the lanes, recording
  **minimum depth** and **fraction of the run at zero** rather than an average,
  which would conceal a brief exhaustion
- [ ] T045 [US1] Add per-request latency into an `hdrhistogram` in
  `crates/workload-gen/src/report.rs`, timed per request and never per key, so
  instrumentation cannot itself put the generator on the critical path
- [ ] T046 [US1] Implement throughput measurement in
  `crates/workload-gen/src/report.rs`: keys/second, bytes/second, and the ratio
  of virtual time advanced to wallclock elapsed, with the timed window
  excluding any startup cache clear
- [ ] T047 [US1] Implement the live run report in
  `crates/workload-gen/src/report.rs` in both forms — terminal summary and
  structured file — carrying the same facts, plus the reproduction parameters
  (seed, description identity, effective parameters, batch size, lane count)
- [ ] T048 [US1] Implement run validity in `crates/workload-gen/src/report.rs`:
  a run whose plan queue reached zero is **invalid**, its throughput is not
  presented as a result, and the process exits 3 — distinct from success, so a
  sweep driver cannot mistake it for a data point
- [ ] T049 [US1] Implement clean interruption in
  `crates/workload-gen/src/main.rs`: on `SIGINT`/`SIGTERM`, drain in-flight
  requests, write both reports, and classify validity. An interrupted run whose
  plan queue never reached zero is valid and exits 0
- [ ] T050 [US1] Wire the `run` subcommand in `crates/workload-gen/src/cli.rs`
  per `contracts/cli.md`: unbounded by default, optional `--until`,
  `--shm-path`, `--lanes`, `--batch-keys`, `--seed`, `--report`,
  `--clear-cache`, and the documented exit codes
- [ ] T051 [P] [US1] Test in `crates/workload-gen/tests/op_stream.rs`: against
  a mock mailbox, the issued operation sequence matches the production client's
  for the same plan, and contains no forbidden operation

**Checkpoint**: US1 is functional and independently testable against a live
local server.

---

## Phase 4: User Story 2 — Emit a workload trace to a file (Priority: P2)

**Goal**: write the same workload to a trace file in two containers, so a
generated workload and a real trace are interchangeable inputs to analysis.

**Independent Test**: emit the shipped example to both containers with a fixed
seed and no server present; the two contain identical records, every row
satisfies the schema's invariants, and repeating reproduces the output byte for
byte.

- [ ] T052 [P] [US2] Implement the self-describing manifest in
  `crates/workload-trace/src/manifest.rs`: `source_class: pre_hashed`, full
  encoding, block geometry, and `block_id_space` recording that identifiers are
  chained u64 keys rather than dense mint-order integers. **Written last**
  during emit, so a directory without one is incomplete by construction
- [ ] T053 [P] [US2] Implement the JSONL writer in
  `crates/workload-trace/src/jsonl.rs`, emitting the full encoding with
  `full_input_blocks` populated and the trailing-partial-block convention
  honoured, recording `partial_final_valid`
- [ ] T054 [US2] Implement the parquet writer in
  `crates/workload-trace/src/parquet.rs` behind the `parquet` feature, emitting
  records identical to the JSONL writer's
- [ ] T055 [P] [US2] Test in `crates/workload-trace/tests/containers.rs`: the
  two containers yield identical records, and every row satisfies the
  full-encoding invariants declared in its own manifest
- [ ] T056 [US2] Implement the emit report in
  `crates/workload-gen/src/report.rs`: completeness only — sessions started and
  completed, turns and blocks emitted, virtual-time span, records per
  container, plus reproduction parameters. Latency, lane utilisation, and the
  virtual-to-wallclock ratio are **omitted, not zeroed**
- [ ] T057 [US2] Wire the `emit` subcommand in
  `crates/workload-gen/src/cli.rs`: `--until` **required**, `--output`,
  `--format jsonl|parquet|both`
- [ ] T058 [US2] Wire the projection into `emit` in
  `crates/workload-gen/src/cli.rs`: project before writing, compare against
  free space via `statvfs`, and refuse past the free space or a documented
  ceiling, naming both figures. `--force` overrides the ceiling but never the
  free-space check
- [ ] T059 [US2] Wire the `validate` and `plan` subcommands in
  `crates/workload-gen/src/cli.rs`: `validate` runs the load-time checks and
  reports effective distributions and the projection without writing; `plan`
  writes the canonical plan serialisation
- [ ] T060 [P] [US2] Implement the simulator converter in
  `crates/workload-trace/src/simulator.rs`, projecting a trace into the
  `{chat_id, parent_chat_id, hash_ids, type}` shape
  `apps/eviction-replay-benchmark` reads, and wire it as `convert`
- [ ] T061 [P] [US2] Test in `crates/workload-trace/tests/simulator.rs`: a
  converted trace loads in the simulator with distinct-key and session counts
  matching the emit report — the loader derives sessions by walking
  `parent_chat_id`, so a wrong chain still loads while collapsing every session
  into one
- [ ] T062 [P] [US2] Test in `crates/workload-gen/tests/emit_determinism.rs`:
  an emit run's output is byte-identical across repeats and across differing
  `--batch-keys` and `--lanes` values — the executable form of FR-072

**Checkpoint**: US1 and US2 both work independently. US2 needs no hardware, so
it is the CI-testable half of the feature.

---

## Phase 5: User Story 3 — Drive a multi-node cluster (Priority: P3)

**Goal**: place sessions across nodes and migrate them, so a migrated session's
prefix is cold on its new node and must be fetched remotely.

**Independent Test**: with two or more nodes configured and a short migration
interval, migrated sessions produce remote fetches on their new node while
local hit rate on the origin node is unchanged.

- [ ] T063 [P] [US3] Implement the 12-byte frame header and body codecs in
  `crates/workload-wire/src/frame.rs` per `contracts/node-agent-wire.md`, all
  integers little-endian, rejecting an oversized `len` without allocating
- [ ] T064 [P] [US3] Write the wire conformance tests **before** the client and
  server, in `crates/workload-wire/tests/conformance.rs`, covering all six
  cases listed in `contracts/node-agent-wire.md`
- [ ] T065 [US3] Implement the client half in
  `crates/workload-wire/src/client.rs`: `TCP_NODELAY`, configurable pipelining
  depth independent of lane count, correlation ids
- [ ] T066 [US3] Implement the server half in
  `crates/workload-wire/src/server.rs`
- [ ] T067 [US3] Implement the `Hello` handshake in
  `crates/workload-wire/src/client.rs` and `server.rs`, **fail-closed** on
  `proto_version` and `build_id`, and carrying `channels` and `block_bytes`
  back so the generator can check its lane count against the node's real
  capacity
- [ ] T068 [US3] Implement the node agent binary in
  `crates/workload-node-agent/src/main.rs` and `agent.rs`: attach to the local
  mailbox, serve `Submit` as a relay that never decides *what* to issue, exit
  non-zero if the mailbox is absent
- [ ] T069 [US3] Implement the agent's pre-filled reusable payload buffer in
  `crates/workload-node-agent/src/payload.rs`, reconstructing block payloads
  from the key so only keys cross the network
- [ ] T070 [US3] Implement node placement and migration in
  `crates/workload-model/src/sim.rs`: new sessions placed uniformly, a
  migrating session moved uniformly among the others, its stored blocks left
  where they were, and migration inert with fewer than two nodes
- [ ] T071 [US3] Implement agent lifecycle in
  `crates/workload-gen/src/agents.rs`: start before the run and stop after,
  both outside the timed window; idempotent startup that replaces a leftover
  from a crashed run; verified teardown that releases resources even when the
  generator exits abnormally
- [ ] T072 [US3] Implement node-loss handling in
  `crates/workload-gen/src/agents.rs`: abort the whole run, report it invalid,
  name the lost node, exit 3. Never continue on the survivors — a lost node
  makes its sessions' prefixes unreachable, shrinks the migration target set,
  and redistributes load, so any later number describes a different experiment
- [ ] T073 [US3] Add `--node` and `--agent-port` to
  `crates/workload-gen/src/cli.rs`, with exit code 4 for a refused peer
- [ ] T074 [P] [US3] Test in `crates/workload-wire/tests/loopback.rs`:
  submitting a plan through a loopback agent stub yields the same operation
  sequence as the local path — the transport-level form of FR-072

**Checkpoint**: all three execution paths work; US1 and US2 are unaffected by
US3.

---

## Phase 6: User Story 4 — Compare eviction policies (Priority: P4)

**Goal**: the same workload under two Certus configurations produces hit-rate
curves that actually discriminate between eviction policies.

**Independent Test**: sweep cache size against hit rate under two policies and
under both ranking modes; the curves are smooth and concave rather than
step-shaped, and the two ranking modes reorder the policies.

- [ ] T075 [US4] Implement the `selection` distribution over instance index in
  `crates/workload-model/src/selection.rs`, so its spread sets working-set size
  independently of the pool's key-space size
- [ ] T076 [US4] Implement `rank_by: slot` and `rank_by: recency` in
  `crates/workload-model/src/selection.rs`: a slot's popularity is inherited by
  each new occupant, while recency ranking makes heat decay as newer instances
  arrive. Index space stays bounded — numbering by a monotonic mint counter is
  wrong and would mint objects nothing selects
- [ ] T077 [P] [US4] Test in `crates/workload-model/tests/selection.rs`: under
  a concentrated `selection`, the realised reference distribution is skewed as
  configured; under `rank_by: recency` an instance's reference rate decays with
  age, and under `slot` it does not
- [ ] T078 [US4] Add the sweep-supporting fields to the structured report in
  `crates/workload-gen/src/report.rs`: hit/miss/pending split, distinct keys
  touched, and the effective working-set size, so a sweep can be aggregated
  without scraping terminal text
- [ ] T079 [P] [US4] Implement the sweep check in
  `crates/workload-gen/tests/sweep.rs` (or a script under `scripts/`): across
  at least five capacity points spanning a hundredfold range, hit rate rises
  monotonically with **no single step contributing more than half the total
  rise** — and the same check **fails** when `selection` is removed from every
  class, which is what proves the check has teeth
- [ ] T080 [US4] Document the policy-comparison procedure in `README.md`,
  including that hit-dependent comparisons need repetition with a stated
  significance test because mint races are preserved deliberately, and that n ≥
  8 is the recorded floor on this hardware

**Checkpoint**: all four stories are independently functional.

---

## Phase 7: Polish & Cross-Cutting Concerns

- [ ] T081 [P] Write `apps/workload-generator/README.md`: purpose, the five
  crates, the emit-only build, and a pointer to `quickstart.md`
- [ ] T082 [P] Add doc comments with runnable `# Examples` to every public item
  across `crates/*/src/`, and confirm `cargo doc --no-deps` is warning-free
- [ ] T083 [P] Add a doc test asserting that
  `specs/001-synthetic-workload-generator/contracts/workload-input.example.yml`
  parses and validates, so the shipped example cannot drift from the parser
- [ ] T084 Run every scenario in `quickstart.md` end to end and correct any
  drift between the guide and the implementation
- [ ] T085 [P] Record a Criterion baseline for the per-key plan-generation
  benchmark and note it in `README.md`, so later regressions are detectable
- [ ] T086 Final gate: `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo doc --no-deps`, `cargo test --all -- --test-threads 1`, plus the two
  non-default-member crates built and tested explicitly with `-p`
- [ ] T087 [P] Consider landing the population simulation from
  `~scooter/popsim/` as a repo test under `crates/workload-model/tests/`, which
  would make the controller-rejection and cohort-seeding measurements
  reproducible rather than external. **Ask first** — this was offered twice and
  never decided

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no dependencies.
- **Foundational (Phase 2)**: depends on Setup. **Blocks every story.** This is
  the critical path and the bulk of the work.
- **US1 (Phase 3)**, **US2 (Phase 4)**: depend only on Foundational, and are
  independent of each other.
- **US3 (Phase 5)**: depends on Foundational and on US1's live path
  (T040–T042), since the agent relays the same operations.
- **US4 (Phase 6)**: depends on Foundational and on US1 (it needs a live run to
  measure). T075–T077 touch `workload-model` and could land earlier, but have
  no observable effect until a live run exists.
- **Polish (Phase 7)**: depends on whichever stories are being delivered.

### Recommended build order — not strictly priority order

Deliver **US2 before US1**, even though US1 is P1. US2 needs no accelerator, no
server, and no cluster, so it is the only story that can be exercised in CI,
and it turns the Foundational phase's determinism guarantees into something
observable. US1 remains the MVP in the spec's sense — it is the tool's purpose
— but US2 is the cheaper first increment and de-risks the core.

### Within Each Story

- Contract-specified tests first (T013 before T014; T064 before T065–T067),
  because the expected values already exist in the contracts.
- Model before execution; execution before reporting; reporting before CLI
  wiring.

### Parallel Opportunities

- T002, T003, T005 in Setup.
- In Foundational: T012, T013, T018, T024, T028, T032, T035, T036, T037 are
  test/bench tasks in separate files and can proceed alongside their
  implementations once the module exists.
- US1 and US2 can be built by different people in parallel after Foundational.
- Within US2: T052, T053, T055, T060, T061, T062 touch distinct files.
- Within US3: T063 and T064 are independent of the client and server.

---

## Parallel Example: Foundational distributions

```bash
# After T006 lands the module, these can proceed together:
Task: "T012 distribution tests in crates/workload-model/tests/distribution.rs"
Task: "T013 key-derivation vectors in crates/workload-model/tests/keys.rs"
Task: "T018 description validation tests in crates/workload-model/tests/description.rs"
```

## Parallel Example: User Story 2

```bash
Task: "T052 manifest writer in crates/workload-trace/src/manifest.rs"
Task: "T053 JSONL writer in crates/workload-trace/src/jsonl.rs"
Task: "T060 simulator converter in crates/workload-trace/src/simulator.rs"
```

---

## Implementation Strategy

### MVP first

1. Phase 1 Setup.
2. Phase 2 Foundational — **the bulk of the work, and it blocks everything.**
3. Phase 4 US2 (recommended before US1: no hardware, CI-testable).
4. **STOP and VALIDATE**: quickstart Scenarios 1–3 pass with no accelerator.
5. Phase 3 US1 — the spec's MVP. Validate Scenario 4 against a live server.

### Incremental delivery

Setup + Foundational → US2 (traces, no hardware) → US1 (live single node) → US3
(multi-node) → US4 (policy comparison). Each step adds value without breaking
the previous one.

### Parallel team strategy

After Foundational: one person on US1, one on US2. US3 joins once US1's live
path exists; US4 last, since it measures rather than builds.

---

## Notes

- `[P]` means different files and no dependency on incomplete work.
- Commit after each task or logical group.
- **The projection (T033–T035) is load-bearing, not a nicety.** It is the
  justification for removing four output-length caps, so if it slips, `emit`
  has a required span and no way to know what that span costs — the situation
  the design set out to avoid. Build it in Foundational, not in Polish.
- Two decisions were deliberately left to this phase and are still open: the
  concrete chain test-vector values (T015 pins them permanently), and whether
  to land `~scooter/popsim/` as a repo test (T087, needs a decision).
