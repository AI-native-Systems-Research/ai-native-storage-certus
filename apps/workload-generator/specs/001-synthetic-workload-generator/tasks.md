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

- [x] T026 Implement canonical ordering in
  `crates/workload-model/src/session.rs`: shared classes in declaration order,
  instances sorted by index within a class **T026 notes.** `bind()` produces
  the order in one place rather than letting the caller assemble it, because
  assembling it wrongly yields a workload that runs, reports plausible numbers,
  and has almost no cross-session reuse. `Bound` carries the `Held`, so a
  session that is dropped without giving its instances back is a leak the type
  system shows — and since `Held` is not `Clone`, `SessionPool` could no longer
  derive `Clone` either. That is correct and worth keeping: cloning a pool
  would duplicate every hold without incrementing its refcount, and releasing
  both copies would free an instance still in use. `take_turn` now
  **debug_asserts the session is bound**, since an unbound session's prefix
  silently omits every shared block.

**MY HEADLINE TEST WAS VACUOUS AND IS FIXED.** The nesting test compared the
leading run of the bound *slots* against the leading run of the bound *mints* —
both derived from the same list, with slot and mint in bijection for immortal
instances — so it held whatever the order was. Proved vacuous by deleting
`sort_unstable` from `Selector::draw`: `instances_are_sorted_within_a_class`
failed, the nesting test still passed. It now compares the bound order against
each set sorted **independently**, and with the sort removed it fails as it
should. **The lesson generalises: a test that derives both sides of its
assertion from the same computation cannot fail.**

**Also found and refused (T017 follow-up):** a session class listing the same
shared class twice in `uses`. The two draws are independent, so one instance
can land at two prefix positions, where prefix chaining gives it two different
keys — which breaks FR-028's "the prefix is a function of the set it drew".
`count` expresses the intent without the ambiguity, and the refusal says so.

- [x] T027 Implement the append-only turn model in
  `crates/workload-model/src/session.rs`: turn *n* reads the whole prefix,
  mints separate input and output growth, both stored and both extending the
  chain **T027 notes.** `PrefixChain` holds the session's keys and `Turn` holds
  **ranges into it**, not copies — a turn's read list is the whole prefix, so
  copying would make FR-025's quadratic *iteration* cost a quadratic allocation
  cost too.

**`Session::take_turn` was made private** (as `advance_cursor`) and replaced by
a free `take_turn(session, growth, rng)`. A public method that advanced the
cursor without minting would let the cursor and the chain drift apart, so a
session could report turns it never minted blocks for. One way to take a turn,
and it always keeps both in step.

**The shared run is laid onto the chain at bind time**, since it is the
*leading* part of the prefix — that is what makes two sessions with the same
set derive the same keys for it, and it is the whole of cross-session reuse.

**Three load-bearing claims, each verified by reintroducing its bug:**

- *Reads exclude the turn's own new blocks* (FR-025). Moving the read boundary
  past the new input fails with "turn 0 read 11 keys, expected 8".
- *Growth is private to its session* (FR-030). Salting growth with a constant
  id instead of the session id fails with "growth blocks leaked between
  sessions".
- *Keys are chained* (FR-027). Deriving from the salt alone fails with "keys
  coincide past the common leading run of [3] and [1, 3]" — and note that the
  leading-run *length* assertion alone did **not** catch it, which is why the
  test also asserts disjointness past the common run.

- [x] T028 [P] Test in `crates/workload-model/tests/session.rs`: sessions
  drawing overlapping instance sets produce **nested** chains, not divergent
  ones — `{0,1}` versus `{0,1,4}` share the first two objects' blocks **T028
  notes.** Aimed at what the unit tests cannot state, rather than repeating
  them: the **named** `{0,1}` vs `{0,1,4}` case as a concrete pair; **strict
  nesting** of a three-session family; **two shared classes**, where agreeing
  on a later class buys nothing if an earlier one differs; and the sharp form
  of FR-027 — `{0,1,4}` and `{0,2,4}` share only object 0's blocks **even
  though both hold object 4**, because it is reached through a different
  parent. All with turns taken, so growth is present and must be seen to
  diverge right after the shared run.

**THE VACUITY TRAP BIT AGAIN, IN A NEW FILE.** The general test compared
leading *bound-order* slot agreement against leading key agreement — both
following the bound order, so both change together and a missing canonical sort
passed all six tests. `common_slots` now sorts each class's slots
**independently**, and with `sort_unstable` deleted from `Selector::draw` the
test fails with "[[2]] and [[3, 2]] agree on 1 leading instances, so 4 keys,
not 0". **Second occurrence of the same mistake in three tasks — the rule to
apply going forward is: derive the expected value from the specification, never
from the code path under test.**

Teeth verified per property: unchained keys fail only
`an_object_held_in_common_at_a_differing_position_yields_nothing` (by design —
it is the test for that), and a lost canonical sort fails only the general
agreement test.

- [x] T029 Implement the virtual-time event loop in
  `crates/workload-model/src/sim.rs`: think time before each turn, session
  birth and death, pool events. Node placement and migration are US3
**T029 notes.** `Simulation::new(&description, seed)` builds and seeds
everything, then `step`/`run_until` deliver turns through a `FnMut(&Session,
&Turn)` — a callback rather than a return value because a session that has just
taken its last turn is finished in the same step, and the callback is the only
moment its chain can still be read. That is the seam T030 builds the plan on.

**Event order at one instant is fixed and deliberate**: shared-pool events,
then
session arrivals, then turns. A turn at `t` must see the population as of `t`;
the reverse order would make a session's shared set depend on the order two
pools happened to be declared in.

**A real defect found by the same-instant test.** An empirical `think_time`
with an atom at zero has a positive mean, so it is accepted, and draws of
exactly zero let a session be born, run and finish **inside one step** — with
its slab handle reused by its replacement in that same step. `admit_new`
filtered unbound handles *before* binding, so a handle listed twice for two
different sessions was bound twice. Fixed by checking immediately before each
bind; verified by reinstating the filter, which fails with "session 57 bound
twice".

**One claim I made and then measured as WRONG.** A comment said the `while` in
step 3 was needed for correctness — that an `if` would deliver a same-instant
turn out of order. It would not: the next step picks it up at the same `t`.
Changing it to an `if` breaks no test. The `while` batches an instant into one
step, which is efficiency, not correctness. Corrected in place.

- [x] T030 Implement `OperationPlan` and `Operation` in
  `crates/workload-model/src/plan.rs`: the operation kinds from
  `data-model.md`, totally ordered by virtual time with a deterministic
  tie-break on session id, and per-session order strict while cross-session
  operations may overlap
- [x] T031 Implement the canonical plan serialisation in
  `crates/workload-model/src/plan.rs`, the artifact SC-003 asserts
  byte-identity against
- [x] T032 [P] Test in `crates/workload-model/tests/determinism.rs`: the plan
  is byte-identical across repeated runs at a fixed seed, **and differs** for a
  different seed — the second half catches a seed that is not wired through,
  which otherwise looks exactly like determinism

**T030-T032 and T037 notes.** A turn becomes check, load, then
reserve/transfer/ commit for **input** and again for **output**, then a poll.
Two store sequences rather than one, because a real engine stores the prompt's
new blocks at the end of prefill and the generated ones as decode produces
them. `OpKind`'s discriminants **are** the wire `op_kind` values from
`node-agent-wire.md`, pinned by a test, so the plan and the agent cannot come
to mean different things.

**`Load` carries the same keys as its `Check`, and must.** Which of them were
present is a run-time outcome (FR-036), so the plan states the candidate set
and the executor narrows it. A plan that recorded hits could not be reproduced.
`Abort` exists for the executor and is **never planned**, asserted by a test.

**Ordering holds by construction, not by sorting.** `sim.rs`'s turn heap now
keys on `(time, session id)` rather than `(time, class, handle)`, which is
exactly the order FR-035 specifies — so an emit run can stream without needing
to hold the plan to sort it. Session ids are run-global and unique, so the
trailing fields never enter the comparison.

**Two corrections found by testing:**

1. **`key_references()` was wrong.** It returned the key *arena* length, but
   `Check` and `Load` share one range as a storage optimisation, so it under-
   reported the figure FR-073 needs — a key mentioned by two operations crosses
   the wire twice. Now summed over operations, with `interned_keys()` for the
   arena.
2. **"References grow quadratically with the span" was wrong.** Per *session*
   the growth is quadratic, but a fixed span holds proportionally fewer long
   sessions, so the run total is **linear** in turn count — measured at 1.4x
   when turns double, not 4x. A projection assuming otherwise would oversize
   every estimate. Both facts now have tests, the per-session one asserting the
   closed form `14T + 2T(T-1)` exactly (80 at T=4, 224 at T=8) rather than a
   ratio.

**T037's audit is in the header of `tests/determinism.rs`, and it reports gaps
rather than a clean sweep**: of the six invariants, batch-size/lane-count
independence is unreachable until the live path (T062), cross-machine key
identity is partial (the vectors pin the key function, but `f64::exp`/`ln` are
not guaranteed bit-identical across libm versions, so a plan *digest* can still
differ across machines), and the emit report's omissions have no test because
the report does not exist until T056. An audit reporting six of six would have
been describing the table, not the tests.

### Projection

- [x] T033 Implement the run projection in
  `crates/workload-model/src/project.rs`: invocations, distinct keys minted,
  key references, and uncompressed bytes, from the description's own rates,
  using a ~10k-session Monte Carlo draw for `E[K²]` since key references grow
  quadratically with session length
- [x] T034 Add span inversion to `crates/workload-model/src/project.rs`:
  suggest a span for a target record count, and warn when a span is short
  relative to the longest finite lifetime in the description
- [x] T035 [P] Test in `crates/workload-model/tests/project.rs`: the projection
  for the shipped example at a 1000-second span is within a stated tolerance of
  an actual emit run's counts

### Benchmarks

- [x] T036 [P] Criterion benchmark of per-key plan-generation cost in
  `crates/workload-model/benches/plan.rs`, the measurement Principle I's
  never-the-bottleneck claim rests on
- [x] T037 [P] Audit task: confirm each of the six cross-cutting invariants
  tabulated at the end of `data-model.md` has a named test, and record the
  mapping in a comment at the top of
  `crates/workload-model/tests/determinism.rs`

**T033-T036 notes.** The projection is **rate-based, never a rehearsal** —
Little's law plus the resolved distributions' moments — so `validate --until n`
costs 102 µs on a 10 000-session description and can be offered as a query.
`E[T²]` is the one moment no kind exposes in closed form (`turns` may be an
empirical sample set), so it is a 10 000-draw Monte Carlo on its own substream;
a test pins that the estimate moves the reference figure by under 1% across
seeds, since otherwise a refusal would depend on the projection's own seed.

**Three things it deliberately does not claim**, each stated in the module
header rather than left for a reader to discover: bytes are for the **canonical
plan only** (that layout is fixed, so the figure is exact — per-container
*trace* bytes need T053's record shape, and guessing them would put an invented
number in a refusal message); the **seeded generation is not modelled**, so a
short span references fewer keys than projected, which is what the span warning
is for; and **emptiness is assumed from means**, which over-projects a
description whose growth is usually zero — the safe direction for a refusal.

**T035's tolerances are stated, not tight** (25% invocations, 30% operations,
60% references and bytes) because the projection is rate-based and a run is
stochastic. A second test pins the operation count *exactly* for a constant
description, which is where a real drift would show.

**T036 measured, and the prediction held.** Per key reference: 19.0 ns for
5-turn sessions, **6.6 ns for 40-turn** ones, 7.9 ns with a 2 000-instance pool
and a 16-wide draw. Long sessions are **2.9x cheaper per key**, because
per-turn overhead amortises over a longer prefix and nothing rescans it — a
regression to *more* expensive per key would mean something in the turn path
had gone superlinear. Against Certus's measured ~5.6 µs/key that is **0.1-0.3%
of the budget, 300-850x headroom on one thread**, which is the quantitative
form of Principle I's never-the-bottleneck claim and now a measurement rather
than an expectation.

**One expectation of mine corrected while writing T033's test**: I asserted the
shipped example mints 10 shared instances' worth of keys. It declares no
`pool:` for that class, and an absent pool means `exact: 1` — the `exact: 10`
in that description is the *session* pool. The code was right; the arithmetic
in my test was not.

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

- [x] T038 [P] [US1] Declare the CUDA FFI symbols locally in
  `crates/workload-gen/src/cuda.rs` with `// SAFETY:` justifications, following
  `apps/remote-lookup-bench` rather than depending on `gpu-services`, which
  would unify cargo features across the workspace
- [x] T039 [US1] Implement the pre-filled reusable payload buffer with an
  optional key stamp in `crates/workload-gen/src/payload.rs`. No per-operation
  byte construction (FR-038)
- [x] T040 [US1] Implement the local mailbox client in
  `crates/workload-gen/src/live.rs` over `shm-queue`, framing with
  `shmq-dispatcher::wire`
- [x] T041 [US1] Map plan operations to the production client's stream, in
  `crates/workload-gen/src/opstream.rs` (not `live.rs`: the mapping is a pure
  function and is the only part of the live path testable without a server, so
  it is its own module)

**T041 done, and it is where the `Touch` defect was found.** Every opcode and
payload layout was read from `lib/shmq-dispatcher/src/{wire,translate}.rs`. The
mapping: `Check`→`CHECK` 1, `Touch`→`TOUCH` 2, `Load`→`LOOKUP` 9,
`Reserve`→`RESERVE` 3, `Transfer`→`COPY_TO_STORE` 4, `Commit`→`COMMIT_STORE` 5,
`Abort`→`ABORT_STORE` 6, `PollEvents`→`TAKE_EVENTS` 10. Three numbering schemes
now exist — the plan's kinds, the node-agent wire's `op_kind`, and the
mailbox's opcodes — and they are deliberately distinct so a change to one
cannot silently redefine another.

**`promote` is a named constant at zero, never a parameter** (FR-043, FR-044).
Promotion is a *decision*, and taking it on the client's behalf would make this
instrument measure a tiering policy it had partly authored. A test reads the
first payload byte off the wire rather than trusting the constant.

**Six opcodes are on a forbidden list**: `PIN`, `UNPIN`, `REMOVE`,
`CLEAR_MEMORY_TIER`, `POPULATE`, `FLUSH_TO_SSD`. A test asserts a whole run's
issued opcodes contain none of them, and a second asserts the list still names
each by its dispatcher constant — a guard list that quietly lost an entry would
be worse than none.

**Payloads are decoded back with the dispatcher's own `wire::Reader`**, not a
hand-written parser, so "my encoder agrees with their decoder" is checked
rather than assumed.

**T038-T040 and T042-T050 are NOT done, and cannot be verified on this
machine.** There is no Certus server running here and `/dev/shm` is empty, so a
lane loop, a plan queue, latency percentiles, throughput, or a validity
classification could be *written* but not *run*. Writing them and reporting
them as done would be the one thing this feature's own Principle VIII forbids.
What is needed to proceed: a Certus server on a local mailbox (SPDK, hugepages
and VFIO per README.md), or a decision to accept unverified code.
- [x] T042 [US1] Implement lanes in `crates/workload-gen/src/lanes.rs`: one
  session's turn at a time, per-session order strict, cross-session overlap
  allowed
- [x] T043 [US1] Reject a lane count exceeding the node's channel count at
  startup in `crates/workload-gen/src/lanes.rs` — the mailbox is depth-1 per
  channel, so over-subscription would silently serialise
- [x] T044 [US1] Implement the plan queue in
  `crates/workload-gen/src/live.rs`, produced ahead of the lanes, recording
  **minimum depth** and **fraction of the run at zero** rather than an average,
  which would conceal a brief exhaustion
- [x] T045 [US1] Add per-request latency into an `hdrhistogram` in
  `crates/workload-gen/src/report.rs`, timed per request and never per key, so
  instrumentation cannot itself put the generator on the critical path
- [x] T046 [US1] Implement throughput measurement in
  `crates/workload-gen/src/report.rs`: keys/second, bytes/second, and the ratio
  of virtual time advanced to wallclock elapsed, with the timed window
  excluding any startup cache clear
- [x] T047 [US1] Implement the live run report in
  `crates/workload-gen/src/report.rs` in both forms — terminal summary and
  structured file — carrying the same facts, plus the reproduction parameters
  (seed, description identity, effective parameters, batch size, lane count)
- [x] T048 [US1] Implement run validity in `crates/workload-gen/src/report.rs`:
  a run whose plan queue reached zero is **invalid**, its throughput is not
  presented as a result, and the process exits 3 — distinct from success, so a
  sweep driver cannot mistake it for a data point
- [x] T049 [US1] Implement clean interruption in
  `crates/workload-gen/src/main.rs`: on `SIGINT`/`SIGTERM`, drain in-flight
  requests, write both reports, and classify validity. An interrupted run whose
  plan queue never reached zero is valid and exits 0
- [x] T050 [US1] Wire the `run` subcommand in `crates/workload-gen/src/cli.rs`
  per `contracts/cli.md`: unbounded by default, optional `--until`,
  `--shm-path`, `--lanes`, `--batch-keys`, `--seed`, `--report`,
  `--clear-cache`, and the documented exit codes
- [x] T051 [P] [US1] Test in `crates/workload-gen/tests/op_stream.rs`: against
  a mock mailbox, the issued operation sequence matches the production client's
  for the same plan, and contains no forbidden operation

**US1 IS RUNNING AGAINST A REAL SERVER on node2.** No root was needed:
`/dev/hugepages` is owned by `scooter` and group-writable, so the server starts
unprivileged. Launch used:

```text
CERTUS_PROFILE=full-fs-block cargo build --release -p certus-server-yaml \
    --features filesys --no-default-features
LD_LIBRARY_PATH=/usr/local/lib ./target/release/certus-server-yaml \
    --device-path /tmp/certus-fs/blockdev.bin --shm-path /dev/shm/certus-shmq \
    --channels 8 --memory-tier-size 2147483648 --format
```

**Measured, 8 lanes of 8 channels, real mailbox round trips**: p50 16 us, p90 21 us,
p99 38 us, max 12.6 ms; 112 requests; plan queue min depth 1 with 0% of samples
at zero, so the run is **valid** and exits 0. The `--lanes 32` refusal was
verified live: it names the node's 8 channels and exits 2.

**Two profile facts worth keeping.** `full-fs-block` is the right profile for
the local path, and needs `--features filesys --no-default-features` — the
build script says so if you omit them. The only profile needing zyre and RDMA
is `full-remote`, which US3 uses.

**zyre was UNBUILT rather than missing, so US3 is not blocked** — I recorded it
as a blocker and that was wrong. `deps/build-zyre.sh` builds it to
`deps/zyre-build/`, after which the library resolves. **Both paths are needed
together**, `/usr/local/lib` for `libgdrapi` and `deps/zyre-build/lib` for
`libzyre`:

```text
LD_LIBRARY_PATH=/usr/local/lib:deps/zyre-build/lib
```

That is why a previously-built server binary would not start.

**T041 found the defect, and running found a second one.** Reading the opcode
table suggested `LOOKUP` takes a key list. The live server answered
*"truncated: need 64 bytes at offset 4, have 32"*: `op_lookup` reads a **handle
batch** — `(key, IpcHandle)` pairs — because a load DMAs into a GPU buffer, and
`COPY_TO_STORE` copies out of one. **Those two operations genuinely need
CUDA**, which is T038/T039's payload buffer. The other six do not.

So the run is **partial and says so in both report forms**: 48 of 160
operations were not issued, and the terminal output states that no data moved
and the throughput is not comparable with a complete run's. Skipping them
silently would have produced a keys-per-second figure from a run that never
transferred a block — the kind of number that gets quoted.

**T044 is now a streaming producer, and the metric has teeth.** The first
implementation built the whole plan before the drive loop, which made FR-062's
invalidity unable to fire — a vacuous check. It is now a producer thread
stepping the simulation in windows of virtual time and pushing one turn at a
time onto a bounded per-lane `sync_channel`, so a consumer that finds its queue
empty is a real statement about the generator. The condition is named an
**underrun**, not starvation: it is a bounded-buffer underrun, whereas
starvation in scheduling names a *fairness* failure, and no lane here is denied
by another lane. Three measurements:

| producer | underruns | valid | exit |
| --- | --- | --- | --- |
| normal | 0 of 24 pops | yes | 0 |
| +3 ms per batch (injected) | 17 of 24 (70.8%) | **no** | **3** |
| unbounded, 28 420 batches | 0 | yes (interrupted) | 0 |

**The initial fill is excluded, and the reason is not convenience.** Counting a
consumer's first look — necessarily at an empty queue, since nothing has been
produced — made every lane underrun exactly once, `[1, 1, 1, 1]`, and every run
invalid. One vacuous metric traded for another. A lane's first batch is now
primed without counting, so an underrun means "this lane had work and ran out",
the condition FR-062 is about. The producer blocking on a full queue is counted
as the positive counterpart, so a run can show that the queue *did* its job
rather than only that it never quite failed. Same principle as FR-046 excluding the startup
cache clear: a run properly begins once its pipeline is full.

Streaming also **bounds memory**: the 20-second unbounded run above held 13.3
MiB RSS across 28 420 batches, where pre-building would have retained every
turn's prefixes. That is what makes `--until` genuinely optional (no
`unwrap_or(60.0)` fallback) and an interrupted run a valid result under FR-074.

**On pacing — I mislabelled this, and the correction matters.** Earlier notes
called "the run does not pace to virtual time" an open limitation. It is not: it
is the specified design. **FR-031** says virtual time "MUST exist only to decide
the order of operations, and MUST NOT be mapped to wallclock time", and the
clarification record settles it — virtual time orders, wallclock measures, and
their ratio is itself a reported metric (FR-066). So `virtual/wallclock` of 430
is a *capability* figure, and FR-062's plan-queue check exists precisely to prove
the generator was not the bottleneck while that ceiling was measured. Pacing is
therefore a **change of experiment**, not a missing feature, and it is discussed
under "If pacing is ever adopted" below rather than tracked as a task.

Latency did rise from p50 16 µs (pre-built) to p50 62 µs with a ~13 ms p99, which
is the producer thread's contention and is the honest cost of not lying about the
queue.

**T038/T039: the live path is now COMPLETE rather than partial.** The two
data-moving operations are issued, and the `PARTIAL RUN` declaration is gone.
Measured on node2 against a freshly started server, seed 7, 4 lanes, 60 s:

| | requests | key references | throughput | max latency |
| --- | --- | --- | --- | --- |
| `--no-payload` (control only) | 168 | 456 | — *declared partial* | — |
| with payload buffer | **240** | **684** | **5544 keys/s, 173.3 MiB/s** | 1627 µs |

240 − 168 = 72 operations and 684 − 456 = 228 keys: exactly the shortfall the
partial-run declaration had been reporting, now closed. p50 rose 38 → 286 µs,
which is real — data actually moves now.

**A defect found while wiring it: `--batch-keys` was inert.** It was parsed,
reported in the run's own reproduction parameters, and never reached the live
path, so FR-069's "largest measured performance lever on this path" did nothing
and the report named a value that had no effect. Requests are now split to it,
and the split is verified to change scheduling without changing the workload:

| `--batch-keys` | requests | key references |
| --- | --- | --- |
| 64 | 240 | 684 |
| 4 | **300** | **684** |

Key references identical, request count different — FR-069 with FR-072 intact.

**A measurement-hygiene trap, recorded because our report cannot see it.** The
first complete run reported 68 keys/s with a 9.8 **second** p99. Nothing was
wrong with the code: the server still held debris from an earlier run killed
mid-flight — `reclaimed N stale reservation(s) (uncommitted > 30s)` repeating,
158 267 accumulated store-backpressure events and 113 772 memory-tier evictions.
Restarting the server gave 5544 keys/s and a 1627 µs max, an **81x** difference.
The run was reported **valid** both times, because FR-062's validity is a
property of *our* queue and says nothing about the state of the server. A
future task should have a live run record the server's tier-event counters at
start and end, so a contaminated baseline declares itself instead of being
quoted.

**T051: the mock is a protocol checker, not a recorder.** Writing down
`Check, Touch, Load, Reserve, Transfer, Commit, Poll` and asserting the encoder
produces it would be vacuous — both sides from the code under test, passing
equally well when the mapping is wrong, which this feature has already been
bitten by three times. So every rule the mock enforces is sourced outside the
crate: the `IDispatcher` contract's error conditions (`KeyNotFound` without a
pending write, `InvalidParameter` on size 0), `check_duplicate_keys` in
`shmq-dispatcher::translate`, and FR-040/043/044/069/072. It decodes with the
server's own `wire::Reader`. 11 tests, one of which (`the_mock_can_actually_fail`)
provokes four violations deliberately, because a checker that cannot object is
decoration.

**A second inert option found: `--clear-cache` did not exist.** T050 is ticked
and lists it, but nothing wired it. Now implemented, issued once on its own path
before the timed window opens (FR-046) — `OpStream` still refuses the opcode, as
a clear inside the operation stream would be the generator evicting on the
policy's behalf.

**The per-key result bytes were being thrown away.** Every one of `CHECK`,
`TOUCH`, `RESERVE`, `COPY_TO_STORE`, `COMMIT_STORE` and `LOOKUP` answers with one
byte per key, and the client checked only the overall status — so a run in which
*every* store was declined reported full throughput. They are now counted and
reported with denominators. They are **outcomes, never generator errors**: we do
not know when Certus will evict anything, and a block stored earlier and absent
later is eviction working, which is the behaviour under measurement. Equally, it
is not the generator's job to be gracious about a server that declines a store —
the report gives the count and refuses to guess the cause, because the wire
carries no reason code.

Measured on node2 with **exactly one** server running:

| run | reserves declined | commits declined | hit rate |
| --- | --- | --- | --- |
| cold cache | 0 of 72 | 0 of 72 | 38.5% |
| same seed again | 72 of 72 | 72 of 72 | 38.5% |
| different seed | 66 of 72 | 66 of 72 | 36.0% |
| warm, `--clear-cache` | **0 of 72** | **72 of 72** | 19.2% |

A cold cache declines nothing, so the store path is sound. A warm one declines
because the key is already there (`create_memory_tier_entry` answers
`AlreadyExists`). The last row matters most: `CLEAR_MEMORY_TIER` frees the memory
tier so `RESERVE` succeeds, but the dispatch map keeps its disk-backed entries so
the commit still cannot land — **`--clear-cache` is not a cold cache and is no
substitute for restarting the server.** A different seed still declines 91.7%
because shared-prefix keys largely survive a seed change. Transfers never decline
because `copy_gpu_to_memory_async` needs no pending write, which is why the
counts read 72 / 0 / 72.

Certus's disk-tier eviction is not yet implemented, so a long streaming run may
eventually see stores fail for that reason; that would be a genuine limitation
rather than intended behaviour, and it is not distinguishable on the wire from
the rows above. Nothing measured here reached it — these runs move about 2.25 MiB
against a 3.7 GiB device.

**A trap of my own making, recorded because the report cannot detect it.** Two
`certus-server-yaml` processes were left polling the same mailbox and device
file. Each keeps its own `pending_stores`, so a `RESERVE` answered by one and a
`COMMIT_STORE` answered by the other finds no pending write, and every decline
figure taken that way is meaningless. Verify with
`ps -eo args | awk '$1 ~ /certus-server-yaml$/'` before trusting a number — and
note that `pkill -f <pattern>` matches the invoking shell's own command line,
which killed this session's shell twice.

**A DEFECT FOUND BY QUESTIONING THE HIT RATE: shared-prefix blocks are read
but never stored.** The reported hit figures did not reconcile with the declined
stores, and chasing that turned up two things.

First, a real bug in the new accounting: `op_check` answers a **three-valued**
`check_state` (`MISS` 0, `RESIDENT` 1, `PENDING` 2) while `op_lookup` answers a
binary flag, and both were counted in one arm as "1 means hit". That made every
`PENDING` key a miss, although `wire.rs` says explicitly that `byte != 0` is the
correct existence test. `CHECK` and `LOOKUP` are now counted and reported
separately, `PENDING` in its own column — excluded from the hit numerator because
the key is not loadable at that instant, and kept in the denominator because the
cache does hold it.

Second, and structural. Measured on one fresh server, a **cold** and a **warm**
run of the same seed report byte-identical results — 60 resident, 0 pending, 96
miss of 156 checks — which cannot happen if the cache retains what the previous
run stored. The plan explains it exactly:

```
312 read refs = 192 refs to 4 shared keys nothing ever stores  (always miss)
              + 120 refs to session-private keys stored earlier (always hit)
```

and the measured misses are `96 + 96 = 192`. Every miss in the run is a reference
to a shared-object block, and **no operation ever reserves one**. So
cross-session prefix sharing — the phenomenon this generator exists to exercise —
contributes **zero** cache hits, only intra-session prefix reuse does, and every
live hit rate is structurally understated. Equilibrium seeding places shared
objects as though they already existed, but nothing puts them into Certus.

`spec.md`'s own race note ("two sessions race to mint the same shared prefix,
both may miss and both store") presumes sessions *do* store shared prefixes, so
this contradicts the spec rather than implementing it. The fix cannot be "store
on miss", because a plan that reacts to run-time outcomes is no longer a function
of description and seed (FR-072). The deterministic form is available: **the
session that mints a shared instance stores its blocks**, minting already being a
plan-time event on `SharedInstance::mint`. Later sessions read and hit. Left
unfixed pending a decision, because it changes which operations a plan contains.

**Keys derive from structural position, not from the seed.** A different seed
declined 66 of 72 reserves, which measured out as exactly 66 of 72 stored keys
shared between seed 7 and seed 99 (91.7%). Not a collision — keys are 64-bit
`splitmix64` outputs and the salt encodes `(tag, class, instance_index,
block_ordinal)`, all structural counters with no seed component, so the same
coordinate yields the same key in every run. The seed changes which coordinates
are used and when. Consequence for experiment hygiene: **a new seed does not give
a fresh key space against a warm server.**

Declines and hit rate were never comparable quantities, which was the smell worth
following: declines concern the 72 session-private keys the run stores, misses
concern the 4 shared keys it never stores. Disjoint populations.

**THE LIVE PATH IS NOW REACTIVE (FR-072a), which fixes the shared-block defect
and one more nobody had raised.** A turn offers every key from the **root** of its
prefix through the end of its new growth, then touches and loads what came back
resident and stores what came back absent. Cache outcomes change the
*interaction* and never the workload: the virtual clock, the session
interleaving, the turn schedule and the key path remain functions of description
and seed, which holds structurally because the producer builds turns from the
simulation without seeing any response.

Measured on node2, one fresh server, seed 7, 4 lanes, 60 s:

| | check resident | lookup hit | stores declined |
| --- | --- | --- | --- |
| **before** — cold *and* warm, identical | 60 of 156 (38.5%) | 38.5% | 72 of 72 (100%) |
| **after** — cold | 140, 8 pending, 84 miss of 232 (**60.3%**) | **100%** (140/140) | 8 of 84 (9.5%) |
| **after** — warm, same seed | 228 of 228 (**100%**) | **100%** | *none needed* |

A warm run is now 100% resident, which is what a re-run of the same seed should
be and never was. `LOOKUP` is 100% because the executor loads only what `CHECK`
reported resident — a client does not load what it knows is absent. Throughput
7026 keys/s, 219.5 MiB/s.

The 8 declined reserves pair exactly with the 8 `PENDING` checks, and that is
FR-036's race actually occurring: two lanes both miss the same shared block and
both store it, one wins. It is counted, not treated as an error.

**Two properties a fixed operation list could not express**, both now guarded by
tests:

1. **An evicted block is stored again.** Certus may evict at any time; without a
   re-store a run's hit rate can only decay and the generator measures a cache it
   never refills.
2. **Shared prefix blocks are stored at all.** No turn mints them, so the plan's
   own `Reserve` operations exclude them and nothing stored them ever.

The mock mailbox now answers `CHECK` from its own committed and pending sets, as
`op_check` does, so the resident branch is actually exercised — a mock that
always said `MISS` would store on every turn and test nothing. It also objects to
a `LOOKUP` for a key it never committed, since the executor must load only what
`CHECK` reported resident. 15 tests.

**US3 architecture settled (FR-072b): the key path crosses the wire, not the
operations.** The reactive rule costs several round trips per turn — fine at
`/dev/shm` latency, unacceptable over a fabric — so `workload-node-agent`
receives the turn's path and does the check, loads and stores against its own
local Certus. The wire carries only what is deterministic (paths, session
identity, virtual timing) and never cache outcomes, which keeps FR-072 intact
across nodes while keeping the chatter host-local.

**PACED MODE IS AGREED AND DEFERRED (FR-078).** To be implemented once
everything else is settled; the design below is what was agreed, so it should not
need re-deriving. It is deferred because it changes the spec in several places and
because supporting both modes is genuinely more code, not because it is unwanted:
latency is the reason for it, and a percentile gathered while the generator
sprints describes a queue the real workload would never form.

Deferred tasks, in order:

- [ ] T088 [DEFERRED] Add the paced mode to `crates/workload-gen/src/live.rs`: hold
  each request until its virtual time is due, `due = t0 + (virtual timestamp −
  virtual start) / rate`, with a `--rate` multiplier defaulting to 1.0. **Due times
  are absolute from `t0`, never relative to the previous submission** — otherwise a
  slow server stretches think time and dilates the workload it is being judged on
- [ ] T088a [DEFERRED] Make **paced the default** and the selector positive:
  `--pacing real|none` defaulting to `real`, with `--rate` valid only under `real`.
  `--unpaced` was considered and rejected — a negative flag cannot be read without
  knowing the default, and `--unpaced --rate 10` is a combination that would have
  to be refused
- [ ] T088b [DEFERRED] Project a paced run's **wallclock cost** before starting,
  symmetrically with FR-073's size projection: at rate 1.0 a run costs its virtual
  span, so `--until 3600` is an hour and silence would look like a hang
- [ ] T088c [DEFERRED] Make `--rate` a **calibration** control, in virtual seconds
  per wallclock second: a description's durations are arbitrary with respect to any
  machine, so the same description must be aimable at faster or slower hardware
  without being rewritten. CLI only, never a description field — it describes the
  target, not the workload (FR-069's reasoning, FR-005 portability). Assert the plan
  fingerprint is independent of the rate
- [ ] T088d [DEFERRED] Report the measured `virtual/wallclock` against the
  requested rate as a **cross-check**: a shortfall is accumulated lateness arriving
  by a second route, and the two must agree
- [ ] T088e [DEFERRED] Add a **rate sweep** to find capacity: offered load scales
  with rate while the workload's shape does not, so the rate at which lateness
  leaves zero is where this machine stops serving this workload on time. A
  load-versus-latency curve, which answers "can this machine serve this workload"
  better than the work-conserving ceiling does
- [ ] T089 [DEFERRED] Replace the validity metric under pacing with **lateness**
  in `crates/workload-gen/src/report.rs`: `lateness = submitted − due`, positive
  only, reported as percentiles with its request count (FR-066a). A run whose
  lateness exceeds tolerance is invalid for FR-062's reason — the generator, not
  Certus, set the pace
- [ ] T090 [DEFERRED] Make the report name the mode, since a paced throughput and
  a work-conserving one are not comparable and would otherwise be quoted together
- [ ] T091 [DEFERRED] Scope FR-031 to the work-conserving mode and re-word FR-062
  so it does not appear to apply under pacing, where an empty queue is the normal
  intended state

**If pacing is ever adopted, it replaces a metric rather than adding a delay.**
Today's run is work-conserving: it issues as fast as the mailbox allows and
measures the ceiling. A paced run would issue each turn at a wallclock time
proportional to its virtual timestamp, so think time becomes real waiting and the
offered load equals the workload's own rate. That answers a different question —
"can Certus serve *this* workload with acceptable latency?" instead of "how fast
can Certus go?" — and it would buy realistic concurrency (today the in-flight
session count is whatever the lanes allow, not arrival rate x duration) and put
the server's time-dependent behaviour on the right timeline (eviction, background
write-through and the 30-second checkpoints we observed all run on wallclock, and
compressing 60 virtual seconds into 0.14 s means the cache never sees the idle
periods the workload describes).

The catch is FR-062. Under pacing an **empty plan queue is the normal, intended
state** — no work is due yet — so "a lane found its queue empty" stops meaning
"the generator was the constraint". The validity metric would have to become
**schedule lateness**: how far past its due time each turn was actually issued.
Adopting pacing therefore means replacing the underrun check, not adding a sleep,
and it makes every experiment cost real time equal to its virtual span.

**Bandwidth and latency were both measuring the wrong thing (FR-066, FR-066a,
FR-066b).** Capturing hit/miss turned out to be *required* for bandwidth, not
merely informative.

`bytes_per_second` was `key_references * block_bytes`, which charges a full block
to every key of every request — but `CHECK`, `TOUCH`, `RESERVE` and
`COMMIT_STORE` move **no payload at all**. Under the reactive rule a resident
block produces three key references and transfers one block, a missing one
produces four and transfers one, so the figure overstated bandwidth **4.9x**
(219.5 -> 45.1 MiB/s on the same run) and conflated reads with writes. Payload
now comes from the only place the information exists — the hit/miss results — and
is reported per direction: **read 32.7 MiB/s (140 blocks), write 17.8 MiB/s (76
blocks)**.

Aggregate latency was similarly a blend. Split per operation:

| op | requests | p50 | p90 | p99 | max |
| --- | --- | --- | --- | --- | --- |
| CHECK | 24 | 79 | 300 | 470 | 470 |
| RESERVE | 24 | 75 | 452 | 687 | 687 |
| COMMIT_STORE | 24 | 144 | 495 | 788 | 788 |
| TAKE_EVENTS | 24 | 225 | 441 | 677 | 677 |
| TOUCH | 20 | 250 | 286 | 455 | 455 |
| LOOKUP | 20 | 276 | 476 | 686 | 686 |
| **COPY_TO_STORE** | 24 | **435** | 1230 | 1671 | 1671 |

The aggregate p50 of 243 us is between `CHECK` at 79 and `COPY_TO_STORE` at 435,
describing the mix rather than the server — and it would move with the hit rate
even if Certus did not. `TOUCH` and `LOOKUP` show 20 requests to the others' 24
because they are issued only when something is resident, and the first turn of
each session has nothing.

**RETRACTED: `TOUCH` is not slower than `CHECK`.** The table above appeared to
show it three times slower, which was a p50 over **20 requests** — noise. At 8000
requests per operation the two are within a microsecond, and `TOUCH` is marginally
the faster in most runs:

| lanes | CHECK | TOUCH | TAKE_EVENTS | LOOKUP |
| --- | --- | --- | --- | --- |
| 1 | 25 | **22** | 18 | 274 |
| 4 | 291 | **292** | 290 | 534 |
| 6 | 465 | **448** | 441 | 727 |

A second 24-request run made `TOUCH` look four times *faster* than `CHECK` (58
against 233), which is the same noise in the opposite direction.

What the adequate sample does show is worth more than the false finding was:

* **Every control operation costs the same** — `CHECK`, `TOUCH`, `RESERVE`,
  `COMMIT_STORE` and `TAKE_EVENTS` all sit at ~20 us uncontended. None of them is
  distinguishable by its own work.
* **Only the payload operations have a distinct cost.** `LOOKUP` is 274 us at one
  lane against `CHECK`'s 25 — about eleven times — and that gap is the DMA.
* **Control latency is dominated by queuing, and scales with lane count**: 25, 291
  and 465 us at 1, 4 and 6 lanes. So a control-operation latency is a statement
  about concurrency at the server, not about the operation.

The report now prints each operation's request count and marks a count too small
to quote (`QUOTABLE_REQUESTS = 100`), which is what caught this — the count column
was already there, and reading it is what turned a plausible finding into a
retraction.

**A third defect the split exposed: we were transferring into reservations we did
not hold.** 88 blocks written against 12 declined reserves — `RESERVE` answers
per key and the executor ignored it, so 12 blocks of payload went to no slot and
their commits then failed for want of a pending write. With the filter: 76
written, **0 of 76 commits declined**, and the only remaining declines are the 8
reserve declines that pair with the 8 `PENDING` — FR-036's mint race.

**US1 is complete.** Nothing is open against it.

**Checkpoint**: US1 is functional against a live local server for the control path.

---

## Phase 4: User Story 2 — Emit a workload trace to a file (Priority: P2)

**Goal**: write the same workload to a trace file in two containers, so a
generated workload and a real trace are interchangeable inputs to analysis.

**Independent Test**: emit the shipped example to both containers with a fixed
seed and no server present; the two contain identical records, every row
satisfies the schema's invariants, and repeating reproduces the output byte for
byte.

- [x] T052 [P] [US2] Implement the self-describing manifest in
  `crates/workload-trace/src/manifest.rs` per `contracts/trace-io.md`:
  `source_class: pre_hashed`, full encoding, block geometry, and
  `block_id_space` recording that identifiers are chained u64 keys rather than
  dense mint-order integers. **Written last** during emit, so a directory
  without one is incomplete by construction
- [x] T053 [P] [US2] Implement the JSONL writer in
  `crates/workload-trace/src/jsonl.rs` per `contracts/trace-io.md`, emitting
  **one record per invocation** (not per session, so the file streams in
  virtual-time order and nothing is buffered) with the full encoding,
  `full_input_blocks` populated and the trailing-partial-block convention
  honoured, recording `partial_final_valid`. The six per-row invariants the
  contract lists are what T055 asserts **T052/T053 notes, and a correction to
  `contracts/trace-io.md`.** I had written `input_length` in **blocks** and
  `block_size` as a count of blocks. That was wrong and is fixed: both now
  follow the corpus — `block_size` is **tokens per block** and `input_length`
  is `blocks * block_size` tokens. Deviating on those two fields would have
  broken exactly the field-level comparability with real traces that the format
  exists for.

**A turn's prompt is not a turn's reads.** `Turn` separates them because FR-025
defines the read set as the prefix *before* the turn, but a trace's
`full_input_blocks` is the request's prompt — that prefix **plus** the turn's
own new input, since the new user text is part of what gets sent. Conflating
them would understate every prompt by one turn's growth and would break the
schema's own `len(full_input_blocks) * block_size == input_length` invariant.

**`request_end` and `partial_final_valid` are null, not zero.** A turn occupies
a single instant because no service time is modelled, so a zero duration would
be a measurement never made; and this generator mints whole blocks, so there is
no trailing partial. Both are **present and null** so a reader need not
recognise which trace it is (FR-056) — the same rule as FR-071's omitted report
fields.

**Row verification is on by default** in the writer (FR-058), including the
append-only check against the previous row of the same session, which is the
one invariant a single row cannot express. Two tests build a broken row by
hand, because the simulation cannot produce one.

- [x] T054 [US2] Implement the parquet writer in
  `crates/workload-trace/src/parquet.rs` behind the `parquet` feature, emitting
  records identical to the JSONL writer's
- [x] T055 [P] [US2] Test in `crates/workload-trace/tests/containers.rs`: the
  two containers yield identical records, and every row satisfies the
  full-encoding invariants declared in its own manifest **T054/T055 notes.**
  The feature gate is **verified, not asserted**: `cargo tree -p
  workload-trace` shows **0** arrow crates by default and **28** with
  `--features parquet`.

**Parquet buffers, and that is the one place the emit path's bounded-memory
property is weakened.** A column has to be assembled before it can be encoded,
so the writer holds `ROW_GROUP_ROWS` = 8 192 rows. Weakened by a *constant*
rather than by the run — peak buffered rows is the row-group size whatever the
span — and recorded in the module header rather than left to be found on a
memory graph.

**Arrow's `ListBuilder` marks its item field nullable by default, and I told
the builder rather than weakening the schema.** A trace's schema is part of its
self-description (FR-056); declaring "possibly null" for items that never are
would be a false claim about the data.

**T055 compares the containers through their serialised forms**, reading the
parquet back and rebuilding records from the JSONL text, rather than comparing
what the two writers were handed. Each writer is convincing in isolation — the
failure mode is a field that means something slightly different in one of them,
and that only shows up on a round trip. A fifth test pins that parquet really
is smaller than JSONL for the same records, since if that stopped holding the
feature would be carrying arrow for nothing.

**One gap recorded in the test file's header rather than hidden**: the whole
file is
`#![cfg(feature = "parquet")]`, so the *equivalence* claim is only tested with
the feature on. CI must run `--features parquet` or SC-004 is untested — a
feature-gated test nobody enables is indistinguishable from no test.

- [x] T056 [US2] Implement the emit report in
  `crates/workload-gen/src/report.rs`: completeness only — sessions started and
  completed, turns and blocks emitted, virtual-time span, records per
  container, plus reproduction parameters. Latency, lane utilisation, and the
  virtual-to-wallclock ratio are **omitted, not zeroed**
- [x] T057 [US2] Wire the `emit` subcommand in
  `crates/workload-gen/src/cli.rs`: `--until` **required**, `--output`,
  `--format jsonl|parquet|both`
- [x] T058 [US2] Wire the projection into `emit` in
  `crates/workload-gen/src/cli.rs`: project before writing, compare against
  free space via `statvfs`, and refuse past the free space or a documented
  ceiling, naming both figures. `--force` overrides the ceiling but never the
  free-space check
- [x] T059 [US2] Wire the `validate` and `plan` subcommands in
  `crates/workload-gen/src/cli.rs`: `validate` runs the load-time checks and
  reports effective distributions and the projection without writing; `plan`
  writes the canonical plan serialisation
- [x] T060 [P] [US2] Implement the simulator converter in
  `crates/workload-trace/src/simulator.rs`, projecting a trace into the
  `{chat_id, parent_chat_id, hash_ids, type}` shape
  `apps/eviction-replay-benchmark` reads, and wire it as `convert`
- [x] T061 [P] [US2] Test in `crates/workload-trace/tests/simulator.rs`: a
  converted trace loads in the simulator with distinct-key and session counts
  matching the emit report — the loader derives sessions by walking
  `parent_chat_id`, so a wrong chain still loads while collapsing every session
  into one
- [x] T062 [P] [US2] Test in `crates/workload-gen/tests/emit_determinism.rs`:
  an emit run's output is byte-identical across repeats and across differing
  `--batch-keys` and `--lanes` values — the executable form of FR-072

**T060/T061 notes. `chat_id` must be unique across the whole FILE, not per
session** — the loader keeps a single `chat_id -> root` map, so two sessions
each numbering turns from zero would be merged into one conversation. It is a
running counter over emitted rows, with each session remembering its previous
turn's counter. **Verified by injecting the per-session version: 98 sessions
collapsed into 1, and nothing errored.**

**`hash_ids` is the turn's prompt, not its whole chain.** The simulator counts
every listed key as one access, and a turn reads its prompt while it stores its
output. No key is lost — a turn's output is part of the next turn's prompt —
except the final turn's, which is stored and never read again, correctly absent
from a trace of accesses.

**T061 drives `eviction_replay_benchmark::replay::load` itself** (a test-only
path dependency) rather than a reimplementation. `research.md` D1 is a claim
about *another app's* record shape, and my reading of it is exactly what could
be wrong, so the only way to check it is to run that code. **The assertion that
matters is not "it loaded" but that the loader's derived session count equals
the run's** — a wrong chain loads silently and reshapes the conversation graph,
and a lineage-aware policy would then score against a workload nobody
described.

**The converter refuses a broken parent chain** rather than emitting it, for
the same reason.

**Both entry points are wired and produce byte-identical output**, checked end
to end: `emit --simulator <file>` writes the projection in the same pass
(FR-075), and `convert <trace> --to simulator` projects a stored trace
(FR-075a). `cmp` on the two outputs of the same seed: identical. `convert`'s
output names what the projection drops (FR-077).

**T056-T059 and T062 notes. The emit path runs end to end on the shipped
example**, and running it found two real defects that no test had:

1. **The report counted only session class 0 of two.** 3 862 sessions where the
   truth was 34 641. It reads perfectly plausibly, which is why
   `Simulation::session_totals()` now exists as its own method and a test
   asserts against a *two-class* description.
2. **The reference projection was 2.9x high, and the mean-based warning could
   not see it.** The example's `turns` is exponential(10), so mean duration is
   105 s — under the 300 s span, no warning — while the **p99 session runs 470
   s** and is cut off before its expensive late turns. The projection counts
   every session's full quadratic contribution. The warning is now keyed on the
   **p99**, says the figure is an UPPER BOUND, and says not to compare it with
   a run's actual count. The direction is the safe one for a refusal.

**FR-071 is enforced by the type, not by discipline**: `EmitReport` has **no
fields**
for latency, lane utilisation or the wallclock ratio — not `Option`, not zero.
An `Option` left at `None` is one careless `unwrap_or(0.0)` from a published
zero, and `skip_serializing_if` is avoided for the same reason. Generation
speed is the one wallclock figure FR-071 allows and is named
`generation_rate_invocations_per_second` so it cannot be read as a server
result.

**The two size refusals are deliberately not the same check.** Free space is
hard and `--force` does **not** override it — overriding it yields a truncated
directory and a full filesystem, on a shared box for other people too. The 32
GiB ceiling is a guard rail and `--force` does override it. Both name **both
figures**; a refusal that says only "too large" cannot be acted on. `statvfs`
is read on the **output** directory's filesystem, and a failed call is a
refusal, not a pass.

**`check_size` takes free space as a parameter so both branches are testable.**
On a box with less free space than the ceiling the free-space check always
fires first, so the end-to-end test could never reach the ceiling branch — it
is now honestly named for what it asserts (a refused run writes nothing) and
the branches are covered by unit tests where free space is a value.

**`workload-gen` gained a lib target** so every subcommand is testable
in-process. `cli::run_argv` returns an exit code rather than calling `exit`; a
CLI only reachable by spawning a process tends not to be exercised, and the
exit codes are contract.

**The half of FR-072 this cannot yet reach**, recorded in the test header
rather than implied: identity across differing `--batch-keys` and `--lanes`
needs flags that arrive with US1. The property holds *structurally* —
`workload-model` has no parameter to receive either — but a crate boundary is
an argument, not a test, and the executable form is still owed.

**Confirmed pre-existing, not ours**: `cargo clippy -p workload-gen` with the
default
`live` feature fails on 4 `manual_checked_ops` lints in
`components/interfaces`, reached through `shmq-dispatcher` enabling
`interfaces/spdk`. Both `--no-default-features` states are clean.

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
- [x] T062a [P] [US2] Implement the Mooncake writer in
  `crates/workload-trace/src/mooncake.rs`, emitting `{timestamp, input_length,
  output_length, hash_ids}` one document per line per **request**, `timestamp`
  in true milliseconds off the virtual clock (not quantised — upstream's 3 s
  tick is its corpus's property, not the format's), and `len(hash_ids) ==
  ceil(input_length / block_size)` per upstream's ceil convention. Wire as
  both `emit --mooncake` and `convert --to mooncake`
- [x] T062b [US2] Implement dense renumbering for the Mooncake writer in
  `crates/workload-trace/src/mooncake.rs`: one dense identifier per distinct
  key across the **whole output** (FR-078). **This is the one mistake that
  would look like success** — renumbering per session yields a file that loads
  and replays while cross-session reuse has silently vanished
- [x] T062c [P] [US2] Test in `crates/workload-trace/tests/mooncake.rs`: a
  converted trace reproduces the upstream invariants measured on
  `conversation_trace.jsonl` — `len(hash_ids) == ceil(input_length /
  block_size)` on **every** row, `timestamp` non-decreasing, identifiers
  globally dense (`distinct == max + 1`) — and, the load-bearing assertion,
  **two sessions that shared a shared-object instance still share identifiers
  after renumbering** **T062a-T062c notes. The renumbering bug was injected and
  the tests sorted themselves into exactly two groups**, which is the useful
  result: `two_sessions_that_shared_an_instance_still_share_identifiers`
  **failed** ("shared 5 keys in the trace but 7 identifiers after conversion")
  while **all four structural conformance tests passed**. That is the FR-078
  failure in its pure form — a file that satisfies every invariant upstream
  states and has lost all cross-session reuse.

**First injection attempt did not fire, and that was informative too.** I tried
resetting the map whenever a row's first key was unseen; with a single shared
instance every session's prompt starts with the *same* key, so the condition
was false from row two onward. Renumbering every row was the injection that
worked.

**One of my own tests is recorded as weak rather than left implied.**
`the_prompt_prefix_structure_survives_the_conversion` passes under per-row
renumbering, because restarting at zero each row *also* yields `0,1,2,...`
prefixes. It shows identifiers are assigned in prompt order; it does not show
reuse survives. Noted in the test.

**Block size is read from the trace's manifest, never guessed.** The Mooncake
format carries **no block-geometry field** — upstream's 512 is implicit — so a
wrong value produces a file whose lengths are silently off by a constant
factor. `convert` reads `manifest.json`, refuses if it cannot, and the
conversion **reports** the block size it used as one of its declared losses
(FR-077).

**A backwards timestamp is refused.** Upstream's `timestamp` is non-decreasing,
and a reader that sorts on it would silently reorder the workload rather than
fail.

**Both entry points wired and verified byte-identical**: `emit --mooncake`
writes in
the same pass, `convert --to mooncake` reads a stored trace, `cmp` reports
identical. A real emitted line:
`{"timestamp":6,"input_length":7872,"output_length":64,"hash_ids":[0,1,...]}`
with 492 identifiers and 492*16 = 7872 tokens.

- [x] T062d [P] [US2] Implement the libCacheSim CSV writer in
  `crates/workload-trace/src/cachesim.rs` as `(time, obj_id, size)` rows, and
  have `convert --to cachesim` **print the `--trace-type-params` string** for
  the layout it wrote — that reader's columns are configurable, so the layout
  is only meaningful alongside its parameter string. Traps from upstream:
  numeric ids require `obj-id-is-num=1` or the reader errors, and the CSV
  reader is **ASCII-only**
- [x] T062e [US2] Read the `oracleGeneral` struct layout from libCacheSim's
  reader source and record it in `contracts/trace-interop.md` **before**
  writing any bytes. Upstream documents the four fields (time, obj-id, size,
  next-access-time in reference count) in prose only; widths and endianness are
  **unverified**. This task is the verification, not the writer
- [x] T062f [US2] Implement the `oracleGeneral` writer in
  `crates/workload-trace/src/cachesim.rs` once T062e has pinned the layout,
  computing next-access-time in one **backward pass** over the plan. An emit
  run knows the whole future of its own trace, so this is the one thing we can
  give a cache simulator that a real trace cannot — it makes Belady/optimal
  baselines available **T062d-T062f notes. T062e paid for itself immediately.**
  The prose in libCacheSim's own docs describes `oracleGeneral` as storing
  "time, obj-id, size, next-access-time", which invites assuming 64-bit fields.
  The **verified** layout, read from
  `traceReader/customizedReader/oracle/oracleGeneralBin.h`, is 24 bytes
  **packed**:

```text
offset 0   clock_time         uint32_t
offset 4   obj_id             uint64_t
offset 12  obj_size           uint32_t
offset 16  next_access_vtime  int64_t
```

Four things the prose did not say, each fatal: `clock_time` and `obj_size` are
**32-bit**, so writing 8 bytes for the timestamp shifts every later field; the
record is **packed** (`obj_id` at offset 4 is not 8-aligned) and the reader
casts pointers at fixed offsets, so there is no padding to reproduce; byte
order is **native**; and `next_access_vtime` has **sentinels** (`-1` and
`INT64_MAX` both mean "never again"), so a writer must use one rather than
invent its own.

**Also verified: `obj_id_t` is `uint64_t`** (`cacheObj.h`). Had it been signed,
half our key space would have wrapped and we would have needed dense
renumbering. A test pins a key of `u64::MAX - 1` surviving both containers.

**`clock_time` being 32-bit is a real ceiling**, not a formality: millisecond
timestamps overflow after **49.7 days** of virtual time. The writer **refuses**
past that, naming the figure, because wrapping would make the trace appear to
jump backwards and the reader would accept it. The CSV container has no such
limit — its `clock_time` is `int64_t`.

**The oracleGeneral writer cannot stream, and that is inherent.** Next-access
requires the future, so it buffers the whole reference stream and resolves it
in one backward pass. Peak memory is proportional to the **run** rather than a
constant — 20 bytes per reference, so the shipped example's 18.6M references
cost ~370 MB. It is the only writer here with that property, so a test asserts
the buffering explicitly rather than leaving it to be discovered from a memory
graph.

**libCacheSim's CSV columns are configurable, so a CSV file cannot say what its
own columns mean.** Every conversion prints the ready-to-run `-t "time-col=1,
obj-id-col=2, obj-size-col=3, obj-id-is-num=1"` command. The two verified traps
are handled: numeric ids need `obj-id-is-num=1` or the reader errors, and the
CSV reader is ASCII-only (a test asserts every byte we write is ASCII).

**Verified end to end**: `emit --cachesim` and `convert --to oracle-general` on
the
shipped example produced 1 929 451 accesses over 1 072 096 distinct objects,
and the binary came to 46 306 824 bytes — **exactly 1 929 451 x 24**, which
independently confirms the record size against real output.

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

- [x] T063 [P] [US3] Implement the 12-byte frame header and body codecs in
  `crates/workload-wire/src/frame.rs` per `contracts/node-agent-wire.md`, all
  integers little-endian, rejecting an oversized `len` without allocating
- [x] T064 [P] [US3] Write the wire conformance tests **before** the client and
  server, in `crates/workload-wire/tests/conformance.rs`, covering all six
  cases listed in `contracts/node-agent-wire.md`

**T069 done, and the file it names deliberately does not exist.** The task asked for
`workload-node-agent/src/payload.rs`, but the agent already uses
`workload_gen::payload::PayloadBuffer` (T039), and a second copy would be exactly the
duplication T068a exists to prevent: two pre-filled buffers could drift in their block
layout, and a divergence there would look like a cache returning wrong data. So the agent
reuses it, and the task is recorded as done by reuse rather than by a new file.

**The real gap was the other half: the stamp was written and never read.** Only keys
crossed the network already — the buffer is pre-filled locally and the key stamped at a
known offset. But nothing ever checked a *loaded* block against the key it was loaded
for, and the pre-fill is one repeated byte, so **every block in the buffer was
interchangeable**: a cache returning the wrong block, or no block, would have produced a
run indistinguishable from a correct one. A stamp nobody reads proves nothing.

Added `MEMCPY_DEVICE_TO_HOST`, `PayloadBuffer::read_stamp`, and `--verify-payload` on
both the generator and the agent. Verified live against a cold cache:

```
cold  missing 8, granted 8, blocks_written 8
warm  resident 8, blocks_read 8
verified 8 blocks, 0 mismatches
```

The counters carry `payload_mismatches` with `payloads_verified` beside it as its
denominator, and the test asserts that **every** loaded block was checked rather than
some — a verification that silently no-ops is worse than none, because the run then
*claims* the data was checked.

**A mismatch invalidates the run**, unlike every other cache-facing figure. A miss, a
declined reserve and an eviction are outcomes; a block carrying the wrong key is Certus
returning wrong data, so the report leads with `WRONG DATA` before any throughput and
`is_valid()` is false. It is also independent of Certus's own `integrity-check` feature
by design: a check sharing an implementation with the thing it checks shares its bugs.

Two caveats stated rather than buried: verification **implies stamping** and wants a
**cold** cache, since a block stored by a non-stamping run holds the fill byte and
checking it would report a mismatch that is the instrument's own fault; and it costs a
device-to-host copy per key, so it is opt-in for FR-070's reason.

**Narrowed the provenance hash while here.** Editing a *test* was invalidating deployed
agents, which is friction with no safety in it — a file compiled into a separate test
binary is never linked into the agent. `tests/` and `benches/` are now excluded, taking
the hashed set from 61 files to 44. Inline `#[cfg(test)]` modules inside `src/` are still
covered, since a path cannot tell them from the code around them.

**T068/a/b/c done. The agent drives a real Certus, end to end.**

**T068a came first, because it is the load-bearing part.** The reactive rule was inside
`live::consume`, so an agent written beside it would have been a *second*
implementation — free to drift, with a fix applied on one side and forgotten on the
other surfacing as a local/remote difference that looked like a property of the
network. It is now `workload-gen`'s `exec::TurnExecutor`, and both paths call it. The
local path was re-measured after the extraction and is **byte-identical** to before
(140 resident / 88 miss of 228, 76 written, 12 reserve declines), so the refactor is
behaviour-preserving rather than merely compiling.

`LaneStats` now *holds* `workload_wire::frame::Counters` instead of duplicating those
fields, so a local figure and a remote figure are the same measurement rather than two
that happen to be named alike (T068c).

**Measured against a live agent and a live server** (`tests/live_agent.rs`, `--ignored`):

```
cold  missing: 8, granted: 8, blocks_written: 8
warm  resident: 8, blocks_read: 8
counters: 7 requests, 56 refs, read 8 blocks, wrote 8 blocks
  CHECK 30us  TOUCH 14us  RESERVE 33us  COPY_TO_STORE 663us  COMMIT_STORE 34us  LOOKUP 237us
```

Eight fresh keys all miss and are stored; the same path again comes back **fully
resident and read back**, which is the assertion that distinguishes "the agent sent
frames" from "the agent reached a cache". The per-operation shape matches the local
path's — control operations in the tens of microseconds, the two data movers far higher.

**Three defects that only running it could have found:**

1. **`Counters.blocks_read`/`blocks_written` were fields nothing populated.** The
   counters said 0 while the per-turn outcomes said 8. They duplicated what
   `lookup_hits` and `transfers_attempted − declined` already say, so they are now
   **derived methods**: two representations of one quantity can disagree, one cannot.
2. **A closed connection never returned its mailbox channel.** An agent could serve
   `--lanes` connections *in its whole lifetime* and then refuse every client with a
   connection reset — found by running the test twice. Channels and payload slots now
   go back to the pool on drop, including when `accept` fails part-way.
3. **The provenance check fired on a genuinely stale binary** — mine, two minutes old
   (`70e22ba5…` against `448629b9…`), refused by name. Good demonstration, and also the
   friction predicted: the agent must be rebuilt after any source edit.

`Stats` returns serialized V2 histograms plus the counters (T068b), and the test
deserialises them, checks each histogram's length against its reported request count,
and confirms a serialize/deserialize round trip preserves quantiles — because a
multi-node merge that silently changed the numbers would be worse than no merge.

**T067 done, 12 handshake tests; 51 in the crate.**

**`build_id` had to become a *source* identity, not a binary digest.** The contract
called it "a 32-byte digest of the agent binary", but the generator and the agent are
different binaries, so their digests differ by construction and the check could never
pass. **FR-051 asks the right question** — whether the daemon "was built from the same
**sources** as the generator" — so the identity is a build-time source id: commit,
plus a hash of the tracked diff, plus a hash of the porcelain status.

It is computed **once**, in `workload-wire`'s build script, and both binaries obtain
it by depending on that crate. Two build scripts computing it independently could
disagree — a stale cache on one, a different working directory on the other — and a
provenance check that can disagree with itself is worse than none.

**The first version asked git, and that was the wrong instrument.** Asking "what
happens to a build from a tarball?" exposed three faults at once, all fixed by
hashing the source files directly instead:

1. **A tarball has no git.** The build still succeeded, but the identity fell back to
   `unknown` and every multi-node run from a release tarball would have been refused —
   a cost imposed by the mechanism rather than by the requirement.
2. **Untracked files were invisible.** A new file on one node and absent on the other
   changed behaviour without changing the identity, which is precisely the divergence
   the check exists to catch.
3. **Cargo could not know when to re-run the script.** `.git/index` moves on
   `git add`, not on a bare edit, so an uncommitted change could leave a stale
   identity compiled in.

The identity is now a digest of every `.rs` and `Cargo.toml` under
`apps/workload-generator/crates/`, sorted by path, each file contributing its path as
well as its bytes so that moving code between files changes it. Each file is declared
to cargo, which closes fault 3. **Verified on a git-free copy**: the build succeeds
and produces a real identity (`src:58:948426:…`) rather than `unknown`, a source edit
of ten bytes moves the fold, and restoring the file brings it back.

**The boundary that is not covered** is workspace dependencies outside this
application — a node running a different `shmq-dispatcher` would not be caught. That
is deliberate: hashing the whole repository would make the identity change on every
unrelated edit, and a check that fires constantly gets ignored within a week.

**Cost, measured rather than assumed.** It hashes 58 files and 948 KB in **1.2 ms**
(0.5 ms walking, 0.7 ms folding), and only when one of those files changes, since each
is declared to cargo — a no-op rebuild re-runs nothing, and an edit that does trigger
it already costs ~0.43 s of recompilation, so the hash is ~0.3% of a bill the edit was
paying anyway. It does **not** touch the Certus source. For scale: every `.rs` and
`Cargo.toml` in the whole repository is 410 files / 4.6 MB at **189 ms**, and including
SPDK's C and the Python is 9 819 files / 153 MB at **447 ms** — of which 181 ms is the
directory *walk* against `deps/` rather than the hashing. So widening the scope is
affordable; the reason to stay narrow is identity churn, not milliseconds.

**It is strict, and the cost is honest**: a comment change anywhere in these crates
invalidates a deployed agent, so a multi-node run needs the agent redeployed after any
edit. That is the price of a check that cannot be talked out of firing by "it is only
a constant", which is how staleness gets in.

**Unknown provenance is refused, not assumed** — it now means the source files could
not be read at all, which is a real problem rather than a portability case.
`WORKLOAD_SOURCE_ID` overrides everything, for a packager with a better answer.

**Both ends check**, which is what makes a refusal legible: the agent replies with a
non-zero status *and always sends its own identity even while refusing*, so the
generator's message can name both sides. Every refusal names the node, since on a
cluster the useful part is which machine is wrong. The protocol version is checked
**first**, because if the versions differ the rest of the reply may not mean what it
appears to and reporting a build mismatch would send an operator after the wrong
problem.

Capacity is checked too: more lanes than channels is refused because the mailbox is
depth-1 per channel, so over-subscription does not fail — it **serialises silently**,
which reads as a slow server rather than a misconfigured run. A block-size
disagreement is refused for the same shape of reason.

Not cryptographic, and does not need to be: the failure being prevented is an
accident — a node left running yesterday's build — and anyone able to replace the
binary can replace the identity it reports.

**T066 done, 10 server tests; 37 in the crate.** Transport only — it reads a
frame, dispatches and replies, and knows nothing of mailboxes or keys. The work is
a `Service` the agent implements, which is what lets the whole protocol be driven
in-process with no agent and no accelerator, including a full client↔server
conversation over an in-memory pipe and a live one over TCP.

**`&mut self` is how the causal ordering rule became structural.** `Service` takes
`&mut self` and is built per connection by `ServiceFactory::accept`, so a handler
cannot be shared across a connection's frames without the compiler objecting — the
thread pool that would silently reorder a session's turns cannot be written by
accident rather than merely being warned against. Per connection also because a
connection is a lane and a lane claims its own mailbox channel, which is depth-1.

**A design flaw found by a test that hung.** `serve` joined its connection threads
on the way out, but a connection thread blocks reading the next frame, so one idle
peer prevented teardown indefinitely — and FR-053 requires teardown to be *verified*,
a teardown bug having already invalidated an A/B series in this repository. Accepted
sockets now carry a read timeout, and a timeout **between** frames is the moment to
check whether the run stopped, while a timeout **part-way through** a frame is
retried because the peer is mid-send and giving up there would make a slow network
indistinguishable from a broken one. The same test now finishes in 0.01 s instead of
never.

A protocol violation closes the connection rather than replying: there is no error
frame, and inventing one would let a peer keep a connection alive by sending
nonsense. A close *between* frames is how a run ends; a close *mid-frame* is
`UnexpectedEof`, and conflating the two would end a run quietly as though the
generator had finished. `Shutdown` is answered **before** the loop returns, since a
close is what FR-064 reads as a lost node and an orderly stop would otherwise be
reported as a failure.

**T065 done, 12 client tests.** Two properties are asserted rather than assumed.
Pipelining depth is proved not to change what is submitted — the frames at depth
1, 8 and 32 are byte-identical bar their correlation ids, which is FR-072 in its
transport form. And a reply is **matched by correlation id rather than by arrival
order**, tested with replies queued newest-first, because the contract permits a
multiplexed connection and an in-order stream must not become an unstated
assumption.

`TCP_NODELAY` is checked on a real loopback socket, since it is the one property an
in-memory transport cannot answer. A synchronous call with turns still in flight is
**refused**, because the next frame off the socket would be a `TurnOutcome` and
would be decoded as whatever was asked for — a silent misread rather than an error.
A clean close is reported as `Closed` distinctly from an I/O error, since a lost
node must abort the run and be named (FR-064).

**Ordering: the generator owns one causal rule, and nothing more.** Turn *n+1* of a
session contains the blocks turn *n* stored, so two turns of one session must not be
in flight together. Pipelining preserves that because the agent takes a connection's
frames in arrival order and a lane owns a fixed set of sessions on its own
connection. The symptom of breaking it is quiet rather than loud: turn *n+1* would
check a prefix turn *n* had not stored yet, find it absent and store it again, so the
run completes with inflated store counts and a depressed hit rate.

**Reordering after submission is Certus-internal and not a generator concern.** The
mailbox has parallel channels and the server runs multiple threads, so two submitted
requests may be processed in either order, and preventing that would mean one channel
and no parallelism — removing the thing the measurement exists to exercise. No
end-to-end ordering claim is made on either path, and neither should be changed to
enforce one. `CHECK`'s `PENDING` state is the evidence that Certus already expects
concurrent stores of one key and reports them.

**T063/T064 done, 15 conformance tests, no agent and no accelerator needed.**
Two allocation hazards are guarded rather than one: `len` is refused against a
bound before anything is sized from it, and `SubmitTurn`'s **key count** is
checked against the bytes actually present before the vector is reserved — a
peer-supplied `u32` would otherwise let a 14-byte frame ask for 32 GB. Truncation
is tested **exhaustively at every prefix length** of every body rather than at one
illustrative cut, because a decoder that stops early returns a value with its
remaining fields zeroed, and a half-decoded `SubmitTurn` submits a turn whose keys
the generator never sent.

`PROTO_VERSION` is pinned at 2 with a test that says why: version 1 sent one
operation per frame, and an agent speaking it would read a key *path* as an
operation's key list and issue something plausible.
- [x] T065 [US3] Implement the client half in
  `crates/workload-wire/src/client.rs`: `TCP_NODELAY`, configurable pipelining
  depth independent of lane count, correlation ids
- [x] T066 [US3] Implement the server half in
  `crates/workload-wire/src/server.rs`
- [x] T067 [US3] Implement the `Hello` handshake in
  `crates/workload-wire/src/client.rs` and `server.rs`, **fail-closed** on
  `proto_version` and `build_id`, and carrying `channels` and `block_bytes`
  back so the generator can check its lane count against the node's real
  capacity
- [x] T068 [US3] Implement the node agent binary in
  `crates/workload-node-agent/src/main.rs` and `agent.rs`: attach to the local
  mailbox and serve `SubmitTurn` by applying FR-072a's rule against it — check the
  path, load what is resident, store what is absent. It decides no *workload*:
  which keys, which session and which virtual time all come from the generator.
  Exit non-zero if the mailbox is absent
- [x] T068a [US3] Depend on `workload-gen`'s library for the split and encoders so
  the reactive rule has **one** implementation. Two would let the local and remote
  paths diverge and make FR-072's guarantee unverifiable. It cannot live in
  `workload-wire`, a CUDA-free default member, since depending on `shmq-dispatcher`
  there would unify `interfaces/spdk` into the default build
- [x] T068b [US3] Serve `Stats` returning **serialized histograms** per `op_kind`,
  never percentiles: the median of two nodes' medians is not a median, so merging
  percentiles yields a number belonging to no distribution. Needs
  `hdrhistogram`'s `serialization` feature
- [x] T068c [US3] Have the **mailbox-facing code be the only collector** of latency
  and bandwidth, on both paths, and ship its `Counters` back with `Stats`. Only the
  agent is near a remote mailbox, so only it can time a `LOOKUP` or count a block
  that moved; a figure derived from wire timings would describe the transport. The
  counters are **for reporting only** — they may not gate validity, steer
  submission, or re-enter the workload
- [x] T065 [US3] *(also covers the depth-invariance property)* — see below
- [x] T069 [US3] Implement the agent's pre-filled reusable payload buffer in
  `crates/workload-node-agent/src/payload.rs`, reconstructing block payloads
  from the key so only keys cross the network
- [x] T070 [US3] Implement node placement and migration in
  `crates/workload-model/src/sim.rs`: new sessions placed uniformly, a
  migrating session moved uniformly among the others, its stored blocks left
  where they were, and migration inert with fewer than two nodes

**T070 done, 7 tests; 223 in `workload-model`.** `migration_interval` was already in
the schema per session class, and the example input notes the node *list* is
deliberately outside the description — so the workload says how often a session moves
and the deployment says where it can go.

**"Its stored blocks stay where they were" is honoured by doing nothing to them**, which
is the point rather than an omission. A migration changes one field — the session's node
— and nothing else: same chain, same growth, same turn schedule. The next turn simply
offers the same path to a different node, where it misses and is fetched or re-stored.
Moving blocks would model a data migration Certus does not perform, and tracking *which*
node holds each block would duplicate state the cache owns and would be wrong the moment
it evicted one. The test asserts it directly: the keys read with migration are
**identical** to the keys read with it inert.

**Uniform among the others, not among all** (FR-048): drawing over every node would leave
a session where it was with probability `1/nodes`, which is not a migration. Implemented
as `(from + 1 + rand(nodes-1)) % nodes`.

**Migrations are applied lazily, at turn time, and that is exact rather than approximate.**
A session's node matters only when it takes a turn, so a migration between turns is
invisible except through where the next turn goes. Evaluating it there avoids a second
event heap and keeps the ordering `plan.rs` asserts in one place. Two details make it
faithful: the loop applies *every* elapsed interval, because "uniform among the others"
excludes a different node each time and collapsing two migrations into one would land the
session on the wrong node; and the next interval is measured from the **due** time rather
than from when it was noticed, or an idle session's interval would stretch by however
long it was idle.

**The node draw is taken even on a single node**, so the workload does not depend on the
deployment: a description run on 1, 2, 3 and 8 nodes produces the *same* key sequence,
asserted. FR-049's inertness is therefore a matter of taking no migration draws at all
rather than of skipping placement.

`Simulation::migrations()` is reported because an interval long relative to session
lifetime performs none, and a multi-node measurement that meant to exercise migration
would otherwise look like one that did.
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
