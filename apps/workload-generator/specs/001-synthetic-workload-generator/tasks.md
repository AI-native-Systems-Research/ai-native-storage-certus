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

- [x] T006 Implement the `Distribution` enum and sampling in
  `crates/workload-model/src/distribution.rs` for `constant`, `uniform`,
  `empirical`, `normal`, `exponential`, drawing from `rand_chacha::ChaCha20Rng`
  — **not** `StdRng`, which is not reproducible across `rand` releases
- [x] T007 Implement inverse-transform truncation between `F(min)` and `F(max)`
  in `crates/workload-model/src/distribution.rs` — a real truncation, never
  clamping
- [x] T008 Implement integral draws in
  `crates/workload-model/src/distribution.rs`: truncate on `[min − 0.5, max +
  0.5]` and round to nearest, so every integer in range carries equal weight
- [x] T009 Implement `empirical` from inline samples and from a file, with
  `interpolate: false` as the default for counts, in
  `crates/workload-model/src/distribution.rs`
- [x] T010 Accept `inf` in any numeric position in
  `crates/workload-model/src/distribution.rs`
- [x] T011 Implement effective-mean and quantile reporting after truncation in
  `crates/workload-model/src/distribution.rs`, the value FR-003 reports and
  FR-004 gates on
- [x] T012 [P] Tests in `crates/workload-model/tests/distribution.rs`:
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
  hashing crate: `DefaultHasher` and `ahash` are unstable across versions,
  which would leave a trace verifiable only by the binary that wrote it
- [x] T015 Pin the chain test vectors in `crates/workload-model/tests/keys.rs`
  — once committed these values must never change, since changing them
  invalidates every previously generated trace

**Found while implementing T014, and RESOLVED by re-balancing the salt layout
(`contracts/key-derivation.md` updated, T015 vectors re-pinned).** The salt is
assembled with **XOR** over fixed-width fields, so a field that overflows does
not saturate: it corrupts its neighbour and aliases two different blocks onto
one key, which would surface only as an inexplicable cache hit. `keys.rs`
asserts each field's width rather than truncating, which converts the hazard
into a loud panic.

The contract's original layout put the tag at bit 48 and gave every field 16
bits — spending 16 bits on a 3-value tag while capping `block_ordinal` **and
`instance_index`** at 65 536. Both were reachable: a session's input stream
grows every turn, so `turns × E[input_growth]` passes 65 536 in a long run, and
a document pool written as `size: 100000` exceeds the instance field. The tag
moved to the top 2 bits and the 14 freed bits went to the two fields that
needed them:

| Field | Bits | Was | Now | Ceiling |
| --- | --- | --- | --- | --- |
| tag | 62–63 | 16 bits | 2 bits | 3 kinds (`0` reserved) |
| `class_id` | 50–61 | 16 bits | 12 bits | 4 096 shared classes |
| `instance_index` | 24–49 | 16 bits | 26 bits | 67 108 864 per pool |
| `block_ordinal` | 0–23 | 16 bits | 24 bits | 16 777 216 per instance or stream |
| `session_id` | 24–61 | 32 bits | 38 bits | 274 877 906 944 per run |

Both arrangements now use all 64 bits exactly (`2 + 12 + 26 + 24` and
`2 + 38 + 24`), which `field_maxima_are_accepted` pins by asserting the all-max
salts are exactly `7fff…`, `bfff…` and `ffff…`, and
`field_widths_sum_to_the_whole_word` checks by arithmetic on the shift
constants, so moving one shift without its neighbour fails a test instead of
silently overlapping.

**Every ceiling is now unreachable** against SC-012's 10 000 concurrent
sessions and 10 000 000 live keys, so **T017 does not need a load-time check
for these** — the asserts are cheap insurance on a path nothing should reach.
Re-pinning was free only because no trace had yet been generated; the contract
now carries a *Versioning* section saying that a later change must be a new
version recorded in the manifest, never an edit.

### Description parsing and validation

- [x] T016 Implement the YAML schema and parser in
  `crates/workload-model/src/description.rs`, matching
  `contracts/workload-input.example.yml`, which is normative
- [x] T017 Implement validation in `crates/workload-model/src/description.rs`:
  reject an unknown `version`; reject a `uses` entry naming an undeclared
  class; refuse when a count distribution truncated to its pool discards more
  than 5% of its mass, naming both the requested and effective mean; reject the
  `poisson` population form for an unbounded lifetime; reject any host-specific
  or tuning field
- [x] T018 [P] Tests in `crates/workload-model/tests/description.rs`: the
  shipped example validates and reports `long_document`'s effective count mean
  (**5.1435** at pool 20, discarding 1.83%); lowering that pool to 10 is
  **refused** naming both 5 and **3.9515**, having discarded 13.53%

  **Corrected while implementing T011.** The figures originally written here
  and in the example YAML — 5.565 and 4.218 — are the *continuous* truncated
  means. A count is integral (FR-009), so the draw is truncated on
  `[0.5, pool + 0.5]` and rounded to nearest, and the realised mean is lower.
  Reporting the continuous figure would report a quantity no draw realises,
  which Principle VIII forbids. Both values are now asserted against an
  independently computed reference by
  `example_count_statistics_match_the_shipped_description` in
  `tests/distribution.rs`, and the example's comment is corrected.

  The **percentages were already right** and are unchanged: the half-unit
  widening cancels, because `exp(-(p + 0.5)/5) / exp(-0.5/5) = exp(-p/5)`. The
  gate outcome is also unchanged under either definition — pool 10 refuses,
  pool 20 passes — so this corrects a reported number, not a decision.

**Two schema decisions settled while implementing T016/T017, because the spec
does not state them and both change behaviour:**

1. **An absent `pool:` means `exact: 1`, NOT the Poisson default.** FR-014
   makes the fluctuating form the default *for an explicit size*, and the
   example's `pool.size` comment confirms a bare integer is shorthand for
   `{poisson: N}`. But the shipped example's `system_prompt1` and
   `system_prompt2` have no `pool:` at all and an infinite lifetime, and a
   Poisson birth rate of `size / E[lifetime]` is undefined there — so a Poisson
   default would refuse the normative example. Absent means one immortal
   instance, exactly as its comment says. `exact` with an unbounded lifetime is
   accepted (FR-017's mint-once case); `poisson` with one is refused, and the
   refusal names `{exact: N}` as the fix.
2. **`rank_by` defaults to `slot`, and the default is reported.** FR-020
   requires both forms to exist but names no default. `slot` is chosen because
   it leaves the written `lifetime` doing what its author expects, whereas
   `recency` makes `lifetime` nearly inert. Following FR-021's precedent for a
   missing `selection`, a defaulted `rank_by` on a multi-instance pool is
   *reported* at load rather than refused.

**Also done early:** the four salt ceilings are now **load-time refusals**
rather than mid-run panics — a pool beyond 67 108 864 instances, an instance or
a `turns x input_growth` product at or beyond 16 777 216 blocks, or more than 4
096 shared classes. `keys.rs` exposes them as `MAX_CLASSES`,
`MAX_INSTANCES_PER_POOL`, `MAX_BLOCKS_PER_STREAM` and `MAX_SESSIONS_PER_RUN`.
This satisfies the T015 note's requirement without waiting for the projection.

**T083 is effectively already done**: `tests/description.rs` asserts the
shipped example parses and validates, which is what T083 asked a doc test to
do. Keep T083 only if a *doc* test is wanted specifically.

### Populations

- [x] T019 Implement `SharedPool` with the `exact` and `poisson` forms in
  `crates/workload-model/src/pool.rs` — no feedback controller and no gain
  parameter (FR-016)
- [x] T020 Implement equilibrium residual-life seeding in
  `crates/workload-model/src/pool.rs`, used for both forms at `t = 0` **Found
  while implementing T019/T020 — `data-model.md` conflated two indices, and
  both the data model and the key contract are corrected.** A shared instance
  needs a **bounded selection index** (reused on death, so popularity attaches
  to a position) *and* a **unique key identity** (so a replacement does not
  inherit its predecessor's keys). One field cannot do both: if the salt used
  the reused slot, a finite `lifetime` would produce no key churn at all and
  the churn rate `research/population/seeding.py` measures would be identically
  zero. `SharedInstance` therefore carries `slot` and `mint`. A consequence:
  the salt's 26-bit `instance_index` bounds **total mints per class per run**,
  not the live count, so **T033's projection must check it** — the load-time
  check in T017 covers only the live count.

**Also in T019:** pending deaths are a binary heap rather than a scan of the
slots, because a scan costs O(pool size) per event against a 10 000-instance
scale target. Two notes for whoever touches it: `BinaryHeap::iter` is in
*arbitrary* order, not sorted order, so the earliest entry can only be had from
`peek`; and the heap deliberately has no lazy deletion, because every entry is
popped exactly at its death — a `debug_assert` in `advance_to` pins that
assumption rather than leaving dead defensive code that no test exercises.

- [x] T021 Implement retirement and release in
  `crates/workload-model/src/pool.rs`: an expired instance becomes unselectable
  while existing users continue, and its bookkeeping is freed when the last
  user ends. Certus is told nothing

**T021's design question, asked and answered: retirement frees the slot at
once; it does not wait for the refcount.** A dying instance is moved out of its
selection slot in the same virtual instant, and an exact pool refills that slot
immediately; only the instance's *bookkeeping* waits for its last reader. The
alternative — holding the slot until the refcount reaches zero — would mean a
session asking for a whole class could not be served, because a slot would be
occupied by something unselectable and FR-022's draw would come up short for a
reason that has nothing to do with the population process. So `live()` counts
slot occupancy, a retired instance occupies no slot, and
`a_full_pool_stays_deliverable_when_every_object_is_a_zombie` pins it with
every object in the class zombied at once. The `slot`/`mint` split is what
makes this possible: the zombie keeps the key identity its reader still needs,
the replacement takes over the selection position.

**Also fixed in T021 — a latent defect in the exact-replacement path.** `kill`
pushed the dead slot onto the free list unconditionally, and the exact branch
then refilled that same slot without popping it, so `free_slots` grew by one
entry per death for the whole run and every entry named an *occupied* slot. It
was inert only because an exact pool never calls `mint` after seeding; the
first birth that did would have handed out a slot that already had an occupant,
putting two instances at one selection index. `kill` now takes a `free_slot`
flag, `mint` carries a `debug_assert` that a free-listed slot is really empty,
and the test asserts the free list stays empty — verified by reintroducing the
bug, which reports 800 occupied slots free-listed.

**Trace-format decision, taken between T021 and T022** and written up as
`research.md` D8, `contracts/trace-io.md`, `contracts/trace-interop.md`, FR-075
to FR-078, and tasks T062a-T062f. Nothing above the emit phase changes; the
question was whether the emitted schema is a standard and whether we should
emit someone else's instead. **It is not a standard** — it is another team's
normalisation layer over roughly seven public formats, which is what makes it
the right superset to emit and to convert from. **WekaTrace was rejected as an
output and adopted as an input**: every session in that corpus starts at `t =
0.0` with no field for session start, and its identifiers are session-scoped,
so it can carry neither cross-session interleaving nor cross-session reuse.
**Mooncake is the standard export**, verified to satisfy both. An OTel writer
is out of scope with the reasoning recorded so it need not be re-derived.

- [x] T022 Implement uniform instance selection with a bounded index space in
  `crates/workload-model/src/selection.rs`, and report the implied working-set
  size when a pool has more than one instance and no `selection` (FR-021).
  Non-uniform selection is US4
- [x] T023 Implement drawing without replacement bounded by `min(nominal pool
  size, live count)` in `crates/workload-model/src/selection.rs`, and count how
  often that bound binds
**T022/T023 notes.** FR-021's working-set report already existed from T017, so
T022 was the mechanism only. Three things worth recording:

**`rank_by` is inert under uniform selection**, and the code says so rather
than implying otherwise. A uniform draw over the live instances is the same
distribution however the ranks are numbered, so no test here can distinguish
`slot` from `recency` by outcome — one asserts they agree, which is the honest
form. `rank_by` still fixes the index space, because that is what US4 draws
over.

**The draw returns slots in slot order, never rank order.** A session's `uses`
order fixes where each instance's blocks land in its prefix chain, so the order
must be stable for the session's life; a recency rank changes every time
another instance is born, so ordering by rank would silently rearrange a live
session's prefix. This also satisfies T026's within-class ordering at the
source.

**The two bounds are counted apart** (FR-023), because they mean opposite
things: bound by *nominal* is an authoring error the load-time gate should have
caught, while bound by *live* is a Poisson population doing its job.
Attribution is pinned by a test — verified by flipping the two branches, which
fails it. A third counter, `empty_pool`, covers the `e^-N` case raised while
building T021: a nominal-2 Poisson pool is empty 13.5% of the time, and there
the draw delivers nothing at all. Two ratios are reported, `binding_fraction`
and `shortfall_fraction`, because a bound that binds always and costs one
instance is a different distortion from one that binds rarely and costs most of
the draw.

**Still open from T021's discussion**: FR-004's 5% truncation gate is computed
against the *nominal* size, so for a Poisson pool it under-reports how much the
count distribution is really narrowed. The run-time counters above now measure
it after the fact; projecting it at load time is a closed-form Poisson tail and
belongs with T033.

- [x] T024 [P] Test in `crates/workload-model/tests/pool.rs`: residual-life
  seeding produces flat churn from `t = 0`, whereas seeding from `lifetime`
  itself produces the cohort artifact — zero churn early then a burst, with
  periodic modulation persisting for several generations

**T024 needed a way to reach the wrong behaviour**, since the shipped code only
ever seeds from equilibrium — correctly. Added `SeedingPolicy` with
`seed_with_policy`: `seed()` keeps its signature and always uses
`Equilibrium`, so no production path can select the defect by accident, and
`Lifetime` is documented as existing only to be the second arm of a controlled
comparison. A requirement whose justification can be re-run is worth more than
one whose justification is a comment.

Four tests, on the same pool and windows as
`research/population/seeding.py` so the two are directly comparable, asserting
the **structure** and not the digits — the reference's "zero churn for three
windows, modulation still 0.44 after eight generations against 0.04" would make
this a flake detector for the RNG if pinned. Both arms were verified by
reintroducing a bug:

- **A missing length bias in the residual sampler is caught by the exponential
  control, and by nothing else.** Injected it: the control fails (2.85 vs 1.91
  per window) while both normal-lifetime tests still pass, because they only
  ask that equilibrium seeding be *flat* and a wrong-but-flat sampler satisfies
  them. The comment in the test says exactly this, and it is now a measured
  claim.
- **The defect arm is not vacuous.** Making `Lifetime` behave as `Equilibrium`
  fails two tests, so the cohort artifact is really being reproduced rather
  than asserted into existence.

A third test compares the arms directly, so the pair cannot pass by both being
broken the same way, and checks that **total** churn agrees across arms — the
seeding must change *when* keys churn, not how many.

### Sessions, turns, and the plan

- [x] T025 Implement `SessionPool` and `Session` in
  `crates/workload-model/src/session.rs`, with `pool.size` meaning concurrent
  sessions so arrival rate is an output
**T025 notes.** Four design decisions, and one hole found in T017.

**A session has no death event.** A shared object's death is an event in its
own right; a session's *is* its final turn. A death heap was written first and
removed: it works, but it makes the pool and the loop each decide independently
when a session ends, and if they ever disagree — a skipped turn, a re-drawn
think time — the population silently stops matching the operation stream. The
loop now calls `finish`, which **panics if turns remain**, because retiring
early would drop operations from the plan and no downstream statistic would
reveal it.

**The whole turn schedule is drawn at birth**, as absolute virtual times.
Beyond making `dies_at` a value immediately, this makes each session's schedule
a function of *its own* birth: with lazy per-turn draws, adding one session
would shift every later think time in the run. Costs one `f64` per turn per
live session — under 2 MB at the 10 000-session, 20-turn design scale.

**Sessions live in a slab, not a packed `Vec`.** The first version used
`swap_remove`, which **silently invalidated the birth handles** the caller
holds across a step — so binding would have been applied to the wrong session.
Caught by the birth-reporting test; a test now pins that a handle survives
another session's death.

**Seeding gives a residual number of remaining TURNS, not a mid-chain prefix.**
The equilibrium argument in discrete form (length-biased turn count, then a
uniform point in it, so `E[R] ~ (T+1)/2`). Giving a seeded session the prefix
it "would have" accumulated was rejected: those blocks would be *read* by an
operation in the plan while never having been *written* by one, so hit rates
would be computed against a key space partly conjured out of nothing. The price
is a recorded bias — for the first mean-duration, seeded sessions have shorter
chains than steady state.

**`SessionIds` is a distinct type because session ids must be unique across
CLASSES.** `keys::input_salt` takes a session id and a block ordinal and has
**no class field**, so two classes each numbering from zero would derive
*identical keys* for unrelated sessions — showing up as inexplicable
cross-class hits.

**Found and fixed in T017's validation**: a class whose `think_time` has mean
zero
makes sessions instantaneous, so no finite arrival rate can sustain any
concurrency and `size / E[duration]` diverges. Now refused at load with the
arithmetic named, rather than dividing by zero mid-run.

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
  `crates/workload-trace/src/manifest.rs` per `contracts/trace-io.md`:
  `source_class: pre_hashed`, full encoding, block geometry, and
  `block_id_space` recording that identifiers are chained u64 keys rather than
  dense mint-order integers. **Written last** during emit, so a directory
  without one is incomplete by construction
- [ ] T053 [P] [US2] Implement the JSONL writer in
  `crates/workload-trace/src/jsonl.rs` per `contracts/trace-io.md`, emitting
  **one record per invocation** (not per session, so the file streams in
  virtual-time order and nothing is buffered) with the full encoding,
  `full_input_blocks` populated and the trailing-partial-block convention
  honoured, recording `partial_final_valid`. The six per-row invariants the
  contract lists are what T055 asserts
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

### Interoperability with other tools' formats (added by `research.md` D8)

Every one of these is a **conversion**, not a new emit container (FR-075), and
each is a projection of the emitted schema, so none of them touches the
simulation. `contracts/trace-interop.md` carries the verified upstream details
and is normative for all of them. All are [US2]-scoped: no hardware, no server.

- [ ] T062x [US2] Implement the projection module in
  `crates/workload-trace/src/project.rs`: each target is a **function of the
  plan's record stream**, with two entry points calling it — `emit`'s in-stream
  flags and `convert`'s stored-trace pass (FR-075, FR-075a). Equivalence
  between the two is then structural rather than tested. A projection carries
  no manifest and is refused as an input to the determinism check (FR-075b)
- [ ] T062y [US2] Wire the emit-time projection flags in
  `crates/workload-gen/src/cli.rs`: `--mooncake`, `--cachesim`, `--simulator`,
  each a file, with **at least one output required** across them and
  `--output`. A projection **without** `--output` is the expected case, not a
  corner — it is what avoids materialising ~27 GB of native trace to obtain a
  much smaller file. The pre-flight projection must size only the outputs
  requested (FR-073)
- [ ] T062a [P] [US2] Implement the Mooncake writer in
  `crates/workload-trace/src/mooncake.rs`, emitting `{timestamp, input_length,
  output_length, hash_ids}` one document per line per **request**, `timestamp`
  in true milliseconds off the virtual clock (not quantised — upstream's 3 s
  tick is its corpus's property, not the format's), and `len(hash_ids) ==
  ceil(input_length / block_size)` per upstream's ceil convention. Wire as
  both `emit --mooncake` and `convert --to mooncake`
- [ ] T062b [US2] Implement dense renumbering for the Mooncake writer in
  `crates/workload-trace/src/mooncake.rs`: one dense identifier per distinct
  key across the **whole output** (FR-078). **This is the one mistake that
  would look like success** — renumbering per session yields a file that loads
  and replays while cross-session reuse has silently vanished
- [ ] T062c [P] [US2] Test in `crates/workload-trace/tests/mooncake.rs`: a
  converted trace reproduces the upstream invariants measured on
  `conversation_trace.jsonl` — `len(hash_ids) == ceil(input_length /
  block_size)` on **every** row, `timestamp` non-decreasing, identifiers
  globally dense (`distinct == max + 1`) — and, the load-bearing assertion,
  **two sessions that shared a shared-object instance still share identifiers
  after renumbering**
- [ ] T062d [P] [US2] Implement the libCacheSim CSV writer in
  `crates/workload-trace/src/cachesim.rs` as `(time, obj_id, size)` rows, and
  have `convert --to cachesim` **print the `--trace-type-params` string** for
  the layout it wrote — that reader's columns are configurable, so the layout
  is only meaningful alongside its parameter string. Traps from upstream:
  numeric ids require `obj-id-is-num=1` or the reader errors, and the CSV
  reader is **ASCII-only**
- [ ] T062e [US2] Read the `oracleGeneral` struct layout from libCacheSim's
  reader source and record it in `contracts/trace-interop.md` **before**
  writing any bytes. Upstream documents the four fields (time, obj-id, size,
  next-access-time in reference count) in prose only; widths and endianness are
  **unverified**. This task is the verification, not the writer
- [ ] T062f [US2] Implement the `oracleGeneral` writer in
  `crates/workload-trace/src/cachesim.rs` once T062e has pinned the layout,
  computing next-access-time in one **backward pass** over the plan. An emit
  run knows the whole future of its own trace, so this is the one thing we can
  give a cache simulator that a real trace cannot — it makes Belady/optimal
  baselines available
- [ ] T062h [US2] Implement loss declaration for every conversion (FR-077) in
  `crates/workload-trace/src/convert.rs`: each target names what it dropped
  (session grouping, input/output separation, `partial_final_valid`), and a
  converted file is refused as an input to the determinism check **T062i and
  T062j are DEFERRED, not scheduled.** Reading a third-party corpus is a
  different axis from emitting one, and this feature does not need it: 24 real
  traces are already in the emitted schema. They are kept here with their
  measurements because the analysis was done and should not be repeated.

- [ ] T062i [DEFERRED] Implement the WekaTrace reader in
  `crates/workload-trace/src/weka.rs`, reading one **session** per
  line into emitted-schema invocation records. Read-only by decision. Must
  **check** `hash_id_scope` rather than assume it — the corpus says `local`,
  and a revision declaring a global scope would change what an import means.
  Must not invent an input/output split, since the format does not distinguish
  them
- [ ] T062j [DEFERRED] Test in `crates/workload-trace/tests/weka.rs` against a
  small committed fixture, not the 1.85 GB corpus: `len(hash_ids) * 64 == in`
  on every main request, multi-megabyte lines parse (the real corpus's first
  line is 2.76 MB), and a `subagent` group's nested `requests` are read in
  trace order. A fixture whose sessions are prefix extensions of each other
  must import as **nested** chains, matching T028's property from the other
  direction
- [ ] T062k [US2] Add a `convert` section to `quickstart.md`: emit the shipped
  example, convert to Mooncake and to cachesim, and check the reuse-preserving
  assertion by hand. Needs only a Rust toolchain, so it belongs with scenarios
  1-3

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
- [x] T087 [P] Land the population simulation from `~scooter/popsim/` in the
  repository so the controller-rejection and cohort-seeding measurements are
  reproducible rather than external. **DONE, as `research/population/`** rather
  than as a gated test, per the user's decision: it is research provenance for
  FR-013 to FR-016, not a check on the implementation.

  **Porting it found that FR-016's recorded rationale was WRONG.** The claimed
  "8-23% positive population bias from the non-negative creation rate" does not
  reproduce: checked against the reference C implementation in
  `~scooter/birth-death/` (whose `xystats` reports `y_mean = 99.1974` against a
  target of 100), the controller regulates the mean to under 1%, and at those
  parameters the non-negative constraint never binds at all. The figure came
  from a badly-scaled parameter sweep — the per-sample loop gain goes as the
  square of the sampling period. FR-016 now stands on the variance argument
  instead: a regulator holds `var/mean` at 0.48 where an uncontrolled
  birth-death population sits at 1.00, so it suppresses exactly the fluctuation
  FR-013 asks for. Corrected in `spec.md`, the constitution, `plan.md`, and
  annotated in `specify-prompt.md` (provenance, so annotated rather than
  rewritten).

  **FR-015's rationale was right** and is now reproducible. `seeding.py`'s
  `residual_sampler` is the algorithm **T020 must implement**: a length-biased
  pick of a lifetime, then a uniform point within it — general over all five
  distribution kinds, no closed form needed. Its per-generation table is
  **T024's reference measurement**: seeded from the lifetime distribution the
  first three windows see exactly zero churn and modulation is still 0.44 after
  eight generations, against 0.04 for residual-life seeding. T024 should assert
  that structure rather than the digits, so it cannot go flaky.

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
- **Reflow these documents with `scripts/reflow.py`, not by hand.** Three
  separate manual or throwaway-script attempts have corrupted them, each
  differently: merging adjacent list items, duplicating a numbered list marker,
  and joining the contracts' `**Version**:` / `**Status**:` headers. All three
  traps are encoded in the script, and it refuses to write unless the
  whitespace-collapsed word stream is byte-identical to the input's.
- Commit after each task or logical group.
- **The projection (T033–T035) is load-bearing, not a nicety.** It is the
  justification for removing four output-length caps, so if it slips, `emit`
  has a required span and no way to know what that span costs — the situation
  the design set out to avoid. Build it in Foundational, not in Polish.
- Two decisions were deliberately left to this phase and are still open: the
  concrete chain test-vector values (T015 pins them permanently), and whether
  to land `~scooter/popsim/` as a repo test (T087, needs a decision).
