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
version reported by the run, never an edit.

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
question was which trace formats an emit run should write. **There is no
standards-body format at this level**, so the answer is to write several
published de-facto ones rather than a format of our own. **WekaTrace was
rejected as an output and deferred as an input**: every session in it starts at
`t = 0.0` with no field for session start, and its identifiers are
session-scoped, so it can carry neither cross-session interleaving nor
cross-session reuse. **Mooncake is the standard export**, verified to satisfy
both. An OTel writer is out of scope with the reasoning recorded so it need not
be re-derived.

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
rather than only that it never quite failed. Same principle as FR-046 excluding
the startup cache clear: a run properly begins once its pipeline is full.

Streaming also **bounds memory**: the 20-second unbounded run above held 13.3
MiB RSS across 28 420 batches, where pre-building would have retained every
turn's prefixes. That is what makes `--until` genuinely optional (no
`unwrap_or(60.0)` fallback) and an interrupted run a valid result under FR-074.

**On pacing — I mislabelled this, and the correction matters.** Earlier notes
called "the run does not pace to virtual time" an open limitation. It is not:
it is the specified design. **FR-031** says virtual time "MUST exist only to
decide the order of operations, and MUST NOT be mapped to wallclock time", and
the clarification record settles it — virtual time orders, wallclock measures,
and their ratio is itself a reported metric (FR-066). So `virtual/wallclock` of
430 is a *capability* figure, and FR-062's plan-queue check exists precisely to
prove the generator was not the bottleneck while that ceiling was measured.
Pacing is therefore a **change of experiment**, not a missing feature, and it
is discussed under "If pacing is ever adopted" below rather than tracked as a
task.

Latency did rise from p50 16 µs (pre-built) to p50 62 µs with a ~13 ms p99,
which is the producer thread's contention and is the honest cost of not lying
about the queue.

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
158 267 accumulated store-backpressure events and 113 772 memory-tier
evictions. Restarting the server gave 5544 keys/s and a 1627 µs max, an **81x**
difference. The run was reported **valid** both times, because FR-062's
validity is a property of *our* queue and says nothing about the state of the
server. A future task should have a live run record the server's tier-event
counters at start and end, so a contaminated baseline declares itself instead
of being quoted.

**T051: the mock is a protocol checker, not a recorder.** Writing down `Check,
Touch, Load, Reserve, Transfer, Commit, Poll` and asserting the encoder
produces it would be vacuous — both sides from the code under test, passing
equally well when the mapping is wrong, which this feature has already been
bitten by three times. So every rule the mock enforces is sourced outside the
crate: the `IDispatcher` contract's error conditions (`KeyNotFound` without a
pending write, `InvalidParameter` on size 0), `check_duplicate_keys` in
`shmq-dispatcher::translate`, and FR-040/043/044/069/072. It decodes with the
server's own `wire::Reader`. 11 tests, one of which
(`the_mock_can_actually_fail`) provokes four violations deliberately, because a
checker that cannot object is decoration.

**A second inert option found: `--clear-cache` did not exist.** T050 is ticked
and lists it, but nothing wired it. Now implemented, issued once on its own
path before the timed window opens (FR-046) — `OpStream` still refuses the
opcode, as a clear inside the operation stream would be the generator evicting
on the policy's behalf.

**The per-key result bytes were being thrown away.** Every one of `CHECK`,
`TOUCH`, `RESERVE`, `COPY_TO_STORE`, `COMMIT_STORE` and `LOOKUP` answers with
one byte per key, and the client checked only the overall status — so a run in
which *every* store was declined reported full throughput. They are now counted
and reported with denominators. They are **outcomes, never generator errors**:
we do not know when Certus will evict anything, and a block stored earlier and
absent later is eviction working, which is the behaviour under measurement.
Equally, it is not the generator's job to be gracious about a server that
declines a store — the report gives the count and refuses to guess the cause,
because the wire carries no reason code.

Measured on node2 with **exactly one** server running:

| run | reserves declined | commits declined | hit rate |
| --- | --- | --- | --- |
| cold cache | 0 of 72 | 0 of 72 | 38.5% |
| same seed again | 72 of 72 | 72 of 72 | 38.5% |
| different seed | 66 of 72 | 66 of 72 | 36.0% |
| warm, `--clear-cache` | **0 of 72** | **72 of 72** | 19.2% |

A cold cache declines nothing, so the store path is sound. A warm one declines
because the key is already there (`create_memory_tier_entry` answers
`AlreadyExists`). The last row matters most: `CLEAR_MEMORY_TIER` frees the
memory tier so `RESERVE` succeeds, but the dispatch map keeps its disk-backed
entries so the commit still cannot land — **`--clear-cache` is not a cold cache
and is no substitute for restarting the server.** A different seed still
declines 91.7% because shared-prefix keys largely survive a seed change.
Transfers never decline because `copy_gpu_to_memory_async` needs no pending
write, which is why the counts read 72 / 0 / 72.

Certus's disk-tier eviction is not yet implemented, so a long streaming run may
eventually see stores fail for that reason; that would be a genuine limitation
rather than intended behaviour, and it is not distinguishable on the wire from
the rows above. Nothing measured here reached it — these runs move about 2.25
MiB against a 3.7 GiB device.

**A trap of my own making, recorded because the report cannot detect it.** Two
`certus-server-yaml` processes were left polling the same mailbox and device
file. Each keeps its own `pending_stores`, so a `RESERVE` answered by one and a
`COMMIT_STORE` answered by the other finds no pending write, and every decline
figure taken that way is meaningless. Verify with
`ps -eo args | awk '$1 ~ /certus-server-yaml$/'` before trusting a number — and
note that `pkill -f <pattern>` matches the invoking shell's own command line,
which killed this session's shell twice.

**A DEFECT FOUND BY QUESTIONING THE HIT RATE: shared-prefix blocks are read but
never stored.** The reported hit figures did not reconcile with the declined
stores, and chasing that turned up two things.

First, a real bug in the new accounting: `op_check` answers a **three-valued**
`check_state` (`MISS` 0, `RESIDENT` 1, `PENDING` 2) while `op_lookup` answers a
binary flag, and both were counted in one arm as "1 means hit". That made every
`PENDING` key a miss, although `wire.rs` says explicitly that `byte != 0` is
the correct existence test. `CHECK` and `LOOKUP` are now counted and reported
separately, `PENDING` in its own column — excluded from the hit numerator
because the key is not loadable at that instant, and kept in the denominator
because the cache does hold it.

Second, and structural. Measured on one fresh server, a **cold** and a **warm**
run of the same seed report byte-identical results — 60 resident, 0 pending, 96
miss of 156 checks — which cannot happen if the cache retains what the previous
run stored. The plan explains it exactly:

```
312 read refs = 192 refs to 4 shared keys nothing ever stores  (always miss)
              + 120 refs to session-private keys stored earlier (always hit)
```

and the measured misses are `96 + 96 = 192`. Every miss in the run is a
reference to a shared-object block, and **no operation ever reserves one**. So
cross-session prefix sharing — the phenomenon this generator exists to exercise
— contributes **zero** cache hits, only intra-session prefix reuse does, and
every live hit rate is structurally understated. Equilibrium seeding places
shared objects as though they already existed, but nothing puts them into
Certus.

`spec.md`'s own race note ("two sessions race to mint the same shared prefix,
both may miss and both store") presumes sessions *do* store shared prefixes, so
this contradicts the spec rather than implementing it. The fix cannot be "store
on miss", because a plan that reacts to run-time outcomes is no longer a
function of description and seed (FR-072). The deterministic form is available:
**the session that mints a shared instance stores its blocks**, minting already
being a plan-time event on `SharedInstance::mint`. Later sessions read and hit.
Left unfixed pending a decision, because it changes which operations a plan
contains.

**Keys derive from structural position, not from the seed.** A different seed
declined 66 of 72 reserves, which measured out as exactly 66 of 72 stored keys
shared between seed 7 and seed 99 (91.7%). Not a collision — keys are 64-bit
`splitmix64` outputs and the salt encodes `(tag, class, instance_index,
block_ordinal)`, all structural counters with no seed component, so the same
coordinate yields the same key in every run. The seed changes which coordinates
are used and when. Consequence for experiment hygiene: **a new seed does not
give a fresh key space against a warm server.**

Declines and hit rate were never comparable quantities, which was the smell
worth following: declines concern the 72 session-private keys the run stores,
misses concern the 4 shared keys it never stores. Disjoint populations.

**THE LIVE PATH IS NOW REACTIVE (FR-072a), which fixes the shared-block defect
and one more nobody had raised.** A turn offers every key from the **root** of
its prefix through the end of its new growth, then touches and loads what came
back resident and stores what came back absent. Cache outcomes change the
*interaction* and never the workload: the virtual clock, the session
interleaving, the turn schedule and the key path remain functions of
description and seed, which holds structurally because the producer builds
turns from the simulation without seeing any response.

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

**Two properties a fixed operation list could not express**, both now guarded
by tests:

1. **An evicted block is stored again.** Certus may evict at any time; without
   a re-store a run's hit rate can only decay and the generator measures a
   cache it never refills.
2. **Shared prefix blocks are stored at all.** No turn mints them, so the
   plan's own `Reserve` operations exclude them and nothing stored them ever.

The mock mailbox now answers `CHECK` from its own committed and pending sets,
as `op_check` does, so the resident branch is actually exercised — a mock that
always said `MISS` would store on every turn and test nothing. It also objects
to a `LOOKUP` for a key it never committed, since the executor must load only
what `CHECK` reported resident. 15 tests.

**US3 architecture settled (FR-072b): the key path crosses the wire, not the
operations.** The reactive rule costs several round trips per turn — fine at
`/dev/shm` latency, unacceptable over a fabric — so `workload-node-agent`
receives the turn's path and does the check, loads and stores against its own
local Certus. The wire carries only what is deterministic (paths, session
identity, virtual timing) and never cache outcomes, which keeps FR-072 intact
across nodes while keeping the chatter host-local.

**PACED MODE IS BUILT (FR-080).** Done as one unit with FR-079, which is what
the note below recommended: pacing changes *when* a request is submitted, so
with two drivers it would have been built twice.

**The requirement was renumbered FR-078 -> FR-080.** FR-078 was already taken
by the conversion's dense-identifier rule, which is referenced from
`workload-trace/src/mooncake.rs` and its test; two requirements sharing one id
is a defect, and renumbering the unbuilt one was the cheaper half.

- [x] T088 [US1] Add the paced mode to the driver: hold each turn until its
  virtual time is due, `due = t0 + (virtual timestamp - virtual start) / rate`,
  with a `--rate` multiplier defaulting to 1.0. **Due times are absolute from
  `t0`, never relative to the previous submission** — otherwise a slow server
  stretches think time and dilates the workload it is being judged on
- [x] T088a [US1] Make **paced the default** and the selector positive:
  `--pacing real|none` defaulting to `real`, with `--rate` valid only under
  `real`. `--unpaced` was considered and rejected — a negative flag cannot be
  read without knowing the default, and `--unpaced --rate 10` is a combination
  that would have to be refused. **SUPERSEDED by T099**: `--rate inf` expresses
  the mode, so `--pacing` is withdrawn and the combination that had to be
  refused becomes unsayable. Paced-by-default survives unchanged as `--rate
  1.0`
- [x] T088b [US1] Project a paced run's **wallclock cost** before starting,
  symmetrically with FR-073's size projection: at rate 1.0 a run costs its
  virtual span, so `--until 3600` is an hour and silence would look like a hang
- [x] T088c [US1] Make `--rate` a **calibration** control, in virtual seconds
  per wallclock second, CLI only and never a description field. Assert the plan
  fingerprint is independent of the rate
- [x] T088d [US1] Report the measured `virtual/wallclock` against the requested
  rate as a **cross-check**: a shortfall is accumulated lateness arriving by a
  second route, and the two must agree
- [x] T088e [US1] Add a **rate sweep** to find capacity:
  `scripts/rate-sweep.sh`
- [x] T089 [US1] Replace the validity metric under pacing with **lateness**:
  `lateness = submitted - due`, positive only, reported as percentiles with its
  request count (FR-066a)
- [x] T090 [US1] Make the report name the mode, since a paced throughput and a
  work-conserving one are not comparable
- [x] T091 [US1] Scope FR-031 **and FR-062** to the work-conserving mode

**Done, 16 tests** (9 in `pacing.rs`, 3 more in `loopback.rs`, 4 in `live.rs`).
The schedule is computed from the plan's virtual timestamps and the wallclock
and depends on nothing a cache answers, so a loopback stub is the *right*
instrument rather than a weakened one, and the whole of it runs in the ordinary
gate.

**The metric replacement is the substance, and the test that shows why is the
interesting one.** A node four times too slow to keep the schedule leaves the
producer comfortably **ahead** of the lanes — so FR-062 sees a healthy queue
with **zero** underruns while the run is failing to keep the very schedule it
set itself. Measured, not argued:
`a_slow_node_makes_the_run_late_and_that_invalidates_it`. Had pacing been added
as a delay on top of the existing check, every paced run would have looked
valid.

**One assertion of mine was wrong, and the fix made the test better.** I
expected a paced run's queue to run dry and asserted `underruns() > 0`. It does
not: under pacing the producer runs ahead and fills the queue, which is
precisely why the queue carries no information about a paced run. The assertion
is now `underruns() == 0`, which states the point instead of contradicting it.

**Three refusals rather than three silent accommodations.** `--rate 0` would
make every turn due at `t0`, which is work-conserving wearing a rate's name;
`--pacing none --rate 10` asks for two different things; a non-finite rate has
no meaning. All three are exit 2, before anything is launched.

**FR-062 needed scoping too, not just FR-031.** The original note named only
FR-031. FR-062 is the load-bearing half: under pacing it is not merely
uninformative but actively wrong, per the slow-node test above.

**If pacing is ever adopted, it replaces a metric rather than adding a delay.**
Today's run is work-conserving: it issues as fast as the mailbox allows and
measures the ceiling. A paced run would issue each turn at a wallclock time
proportional to its virtual timestamp, so think time becomes real waiting and
the offered load equals the workload's own rate. That answers a different
question — "can Certus serve *this* workload with acceptable latency?" instead
of "how fast can Certus go?" — and it would buy realistic concurrency (today
the in-flight session count is whatever the lanes allow, not arrival rate x
duration) and put the server's time-dependent behaviour on the right timeline
(eviction, background write-through and the 30-second checkpoints we observed
all run on wallclock, and compressing 60 virtual seconds into 0.14 s means the
cache never sees the idle periods the workload describes).

The catch is FR-062. Under pacing an **empty plan queue is the normal, intended
state** — no work is due yet — so "a lane found its queue empty" stops meaning
"the generator was the constraint". The validity metric would have to become
**schedule lateness**: how far past its due time each turn was actually issued.
Adopting pacing therefore means replacing the underrun check, not adding a
sleep, and it makes every experiment cost real time equal to its virtual span.

**Bandwidth and latency were both measuring the wrong thing (FR-066, FR-066a,
FR-066b).** Capturing hit/miss turned out to be *required* for bandwidth, not
merely informative.

`bytes_per_second` was `key_references * block_bytes`, which charges a full
block to every key of every request — but `CHECK`, `TOUCH`, `RESERVE` and
`COMMIT_STORE` move **no payload at all**. Under the reactive rule a resident
block produces three key references and transfers one block, a missing one
produces four and transfers one, so the figure overstated bandwidth **4.9x**
(219.5 -> 45.1 MiB/s on the same run) and conflated reads with writes. Payload
now comes from the only place the information exists — the hit/miss results —
and is reported per direction: **read 32.7 MiB/s (140 blocks), write 17.8 MiB/s
(76 blocks)**.

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

The aggregate p50 of 243 us is between `CHECK` at 79 and `COPY_TO_STORE` at
435, describing the mix rather than the server — and it would move with the hit
rate even if Certus did not. `TOUCH` and `LOOKUP` show 20 requests to the
others' 24 because they are issued only when something is resident, and the
first turn of each session has nothing.

**RETRACTED: `TOUCH` is not slower than `CHECK`.** The table above appeared to
show it three times slower, which was a p50 over **20 requests** — noise. At
8000 requests per operation the two are within a microsecond, and `TOUCH` is
marginally the faster in most runs:

| lanes | CHECK | TOUCH | TAKE_EVENTS | LOOKUP |
| --- | --- | --- | --- | --- |
| 1 | 25 | **22** | 18 | 274 |
| 4 | 291 | **292** | 290 | 534 |
| 6 | 465 | **448** | 441 | 727 |

A second 24-request run made `TOUCH` look four times *faster* than `CHECK` (58
against 233), which is the same noise in the opposite direction.

What the adequate sample does show is worth more than the false finding was:

* **Every control operation costs the same** — `CHECK`, `TOUCH`, `RESERVE`,
  `COMMIT_STORE` and `TAKE_EVENTS` all sit at ~20 us uncontended. None of them
  is distinguishable by its own work.
* **Only the payload operations have a distinct cost.** `LOOKUP` is 274 us at
  one lane against `CHECK`'s 25 — about eleven times — and that gap is the DMA.
* **Control latency is dominated by queuing, and scales with lane count**: 25,
  291 and 465 us at 1, 4 and 6 lanes. So a control-operation latency is a
  statement about concurrency at the server, not about the operation.

The report now prints each operation's request count and marks a count too
small to quote (`QUOTABLE_REQUESTS = 100`), which is what caught this — the
count column was already there, and reading it is what turned a plausible
finding into a retraction.

**A third defect the split exposed: we were transferring into reservations we
did not hold.** 88 blocks written against 12 declined reserves — `RESERVE`
answers per key and the executor ignored it, so 12 blocks of payload went to no
slot and their commits then failed for want of a pending write. With the
filter: 76 written, **0 of 76 commits declined**, and the only remaining
declines are the 8 reserve declines that pair with the 8 `PENDING` — FR-036's
mint race.

**US1 is complete.** Nothing is open against it.

**Checkpoint**: US1 is functional against a live local server for the control path.

---

## Phase 4: User Story 2 — Emit a workload trace to a file (Priority: P2)

**Goal**: write the same workload to a file in the trace format of whichever
tool will read it, so a generated workload is an input to third-party analysis
without that tool needing to learn anything new.

**Independent Test**: emit the shipped example to each supported format with a
fixed seed and no server present; every output receives a record per turn
simulated, each satisfies its target format's documented invariants, and
repeating reproduces the output byte for byte.

- [x] T052 [P] [US2] Implement the per-turn record in
  `crates/workload-trace/src/record.rs` per `contracts/trace-io.md`: the single
  shape every projection writer reads, so no two of them can describe the same
  turn differently (FR-056), with the trailing-partial-block convention and
  `partial_final_valid` honoured
- [x] T053 [P] [US2] Implement the Mooncake writer in
  `crates/workload-trace/src/mooncake.rs` per `contracts/trace-interop.md`,
  emitting **one row per invocation** so the file streams in virtual-time order
  and nothing is buffered, renumbering keys densely across the **whole output**
  (FR-078) and declaring what the format cannot carry (FR-077)
- [x] T054 [P] [US2] Implement the libCacheSim writers in
  `crates/workload-trace/src/cachesim.rs`: the CSV form, printing the
  `--trace-type-params` string its reader needs, and the binary `oracleGeneral`
  form at the 24-byte layout verified against the reader source
- [x] T055 [P] [US2] Test in `crates/workload-trace/tests/`: every writer
  receives a record per turn and its output satisfies that target's documented
  invariants, asserted per format rather than by comparing our writers to each
  other (SC-004)

**A turn's prompt is not a turn's reads.** `Turn` separates them because FR-025
defines the read set as the prefix *before* the turn, but a record's
`full_input_blocks` is the request's prompt — that prefix **plus** the turn's
own new input, since the new user text is part of what gets sent. Conflating
them would understate every prompt by one turn's growth and would break the
`len(full_input_blocks) * block_size == input_length` invariant.

**Lengths are tokens, not blocks.** An earlier revision of
`contracts/trace-io.md` had `input_length` in blocks and `block_size` as a
count of blocks. That was wrong and is fixed: every target format states these
in tokens, so converting at the boundary would mean each projection re-deriving
the same figure and being able to disagree about it.

**`request_end` and `partial_final_valid` are null, not zero.** A turn occupies
a single instant because no service time is modelled, so a zero duration would
be a measurement never made; and this generator mints whole blocks, so there is
no trailing partial. Both are **present and null** — the same rule as FR-071's
omitted report fields.

**Row verification is on by default** (FR-058), including the append-only check
against the previous record of the same session, which is the one invariant a
single record cannot express. Two tests build a broken record by hand, because
the simulation cannot produce one.

- [x] T056 [US2] Implement the emit report in
  `crates/workload-gen/src/report.rs`: completeness only — sessions started and
  completed, turns and blocks emitted, virtual-time span, records per output,
  plus reproduction parameters. Latency, lane utilisation, and the
  virtual-to-wallclock ratio are **omitted, not zeroed**
- [x] T057 [US2] Wire the `emit` subcommand in
  `crates/workload-gen/src/cli.rs`: `--until` **required**, and one flag per
  output format with its own destination, at least one of which must be given
- [x] T058 [US2] Wire the projection into `emit` in
  `crates/workload-gen/src/cli.rs`: project before writing, compare against
  free space via `statvfs`, and refuse past the free space or a documented
  ceiling, naming both figures. `--force` overrides the ceiling but never the
  free-space check
- [x] T059 [US2] Wire the `validate` and `plan` subcommands in
  `crates/workload-gen/src/cli.rs`: `validate` runs the load-time checks and
  reports effective distributions and the projection without writing; `plan`
  writes the canonical plan serialisation
- [x] T060 [P] [US2] Implement the Qwen-Bailian writer in
  `crates/workload-trace/src/qwen.rs`, projecting each record into the
  `{chat_id, parent_chat_id, hash_ids, type}` shape
  `apps/eviction-replay-benchmark` reads, and wire it as `--qwen-bailian`
- [x] T061 [P] [US2] Test in `crates/workload-trace/tests/qwen.rs`: the
  projection loads in the simulator with distinct-key and session counts
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

**The writer refuses a broken parent chain** rather than emitting it, for the
same reason.

**Named for the format, not the consumer.** The flag is `--qwen-bailian` rather
than `--simulator`: the shape is Alibaba's anonymized Bailian usage trace, and
naming it for whoever happens to read it here would invite a reader to think
the shape was ours. Its output names what the projection drops (FR-077).

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

Every one of these is a **projection of the plan** written in the emit pass
(FR-075), so none of them touches the simulation. `contracts/trace-interop.md`
carries the verified upstream details and is normative for all of them. All are
[US2]-scoped: no hardware, no server.

- [x] T062x [US2] Make each target a **function of the plan's record stream**,
  so that nothing downstream of the simulation can reshape a workload. A
  projection is refused as an input to the determinism check (FR-075b)

  **Achieved per format rather than in a shared module.** Each format owns one
  private `write_parts` holding the projection itself, reached through
  `write_record(&InvocationRecord)`. A `project.rs` was considered and **not
  built**: what it would move is the fan-out in `cli.rs`'s emit loop, not the
  projection logic, which is already single-sourced. Weighed against that, a
  fourth format could be wired into the loop and forgotten — judged cheap to
  catch by eye while the flags sit adjacent in one struct, and to be revisited
  if a fourth target lands.

  The narrow per-format reader is a **decision, not drift**: four fields out of
  seventeen for the Qwen target, and a reader that cannot be broken by a change
  to a field it does not use.

  **FR-075b's refusal has no reproducibility-check call site to guard**,
  because no subcommand ingests a trace and checks one: `plan` takes a
  *description* and writes the canonical serialisation. The requirement is
  therefore a rule about what may be claimed, enforced by there being no path
  that would accept a projection as a substitute.
- [x] T062y [US2] Give every output format its own flag and destination in
  `crates/workload-gen/src/cli.rs` — `--mooncake <file>`, `--libcachesim
  <file>`, `--qwen-bailian <file>` — with **at least one required** and none
  privileged. The pre-flight must size only the outputs requested (FR-073).

  **Reshaped by the user mid-task, and the final shape is better than what was
  specified.** The task originally kept `--output <dir> --format
  jsonl|parquet|both` for a Certus-private container beside per-projection
  flags, and asked only that `--output` become optional. Two objections, both
  right:

  1. **`--format both` does not survive contact with four formats.** `--format`
     claimed the general word for a narrow thing, and `both` is a two-valued
     word that would break the moment a third output appeared.
  2. **A Certus-private container was not entitled to be the privileged
     output.** Nothing outside this repository reads one. A flag layout that
     made it the default destination encoded an importance it had not earned —
     and following that objection to its end is what removed the format
     altogether (`research.md` D4).

  So `--format` is **deleted** rather than renamed, and presence-of-flag is the
  selection. An intermediate design — `--format` as a comma-separated list —
  was considered and dropped: with a destination needed per format anyway, the
  selector had nothing left to do.
- [x] T062a [P] [US2] Implement the Mooncake writer in
  `crates/workload-trace/src/mooncake.rs`, emitting `{timestamp, input_length,
  output_length, hash_ids}` one document per line per **request**, `timestamp`
  in true milliseconds off the virtual clock (not quantised — upstream's 3 s
  tick is that capture's property, not the format's), and `len(hash_ids) ==
  ceil(input_length / block_size)` per upstream's ceil convention. Wire as
  `emit --mooncake`
- [x] T062b [US2] Implement dense renumbering for the Mooncake writer in
  `crates/workload-trace/src/mooncake.rs`: one dense identifier per distinct
  key across the **whole output** (FR-078). **This is the one mistake that
  would look like success** — renumbering per session yields a file that loads
  and replays while cross-session reuse has silently vanished
- [x] T062c [P] [US2] Test in `crates/workload-trace/tests/mooncake.rs`: the
  projection reproduces the upstream invariants measured on
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

**Block size is taken from the description, never guessed.** The Mooncake
format carries **no block-geometry field** — upstream's 512 is implicit — so a
wrong value produces a file whose lengths are silently off by a constant
factor. The projection **reports** the block size it used as one of its
declared losses (FR-077), because a consumer has no other way to learn it.

**A backwards timestamp is refused.** Upstream's `timestamp` is non-decreasing,
and a reader that sorts on it would silently reorder the workload rather than
fail.

**Verified on real output.** A line as emitted:
`{"timestamp":6,"input_length":7872,"output_length":64,"hash_ids":[0,1,...]}`
with 492 identifiers and 492*16 = 7872 tokens.

- [x] T062d [P] [US2] Implement the libCacheSim CSV writer in
  `crates/workload-trace/src/cachesim.rs` as `(time, obj_id, size)` rows, and
  have it **print the `--trace-type-params` string** for
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
renumbering. A test pins a key of `u64::MAX - 1` surviving both the CSV and the
binary form.

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
own columns mean.** The projection prints the ready-to-run `-t "time-col=1,
obj-id-col=2, obj-size-col=3, obj-id-is-num=1"` command. The two verified traps
are handled: numeric ids need `obj-id-is-num=1` or the reader errors, and the
CSV reader is ASCII-only (a test asserts every byte we write is ASCII).

**Verified end to end**: `emit --libcachesim` on the
shipped example produced 1 929 451 accesses over 1 072 096 distinct objects,
and the binary came to 46 306 824 bytes — **exactly 1 929 451 x 24**, which
independently confirms the record size against real output.

- [x] T062h [US2] Implement loss declaration for every projection (FR-077):
  each target names what it dropped (session grouping, input/output separation,
  `partial_final_valid`), and a projection is refused as an input to a
  determinism check

  **Only `mooncake` declared anything to begin with**, from a
  `declared_losses()` on its stats; the other two had their text hand-written
  in `cli.rs`. So the lists moved onto `CachesimStats` and `QwenStats` beside
  Mooncake's, and one function renders all three. The format owns the list — it
  alone knows what it dropped — while the rendering and the FR-075b line are
  shared, so two hand-written copies of something that must agree cannot drift.
  The FR-075b line is printed once per run rather than once per projection:
  three copies read as three claims about three files rather than one property
  of all of them.

  **Two losses were missing from the text that existed**, both understatements
  of the same fact. `cachesim` and `qwen` initially projected
  `full_input_blocks` only, so it was not that the input/output *distinction*
  was flattened — the output keys were **not referenced at all**. Mooncake
  keeps `output_length` as a token count and drops the identities. All three
  now say so in those terms.

  **The dropped-empty entry appears only when something was dropped.** A
  declaration that names a loss of zero rows trains a reader to skip the list;
  both branches are asserted.

  **FR-075b has no reproducibility-check call site to guard**, because no
  subcommand ingests a trace and checks one — see T062x. It is a rule about
  what may be claimed of a projection, and the refusal it demands is satisfied
  by there being no path that would accept one as a substitute for the
  description and seed.

  **T062i and T062j are DEFERRED, not scheduled.** Reading a third-party trace
  is a different axis from emitting one, and this feature does not need it.
  They are kept here with their measurements because the analysis was done and
  should not be repeated.

- [x] T062m [US2] Project the **generated run**, not the prompt alone, in the
  two cache-simulator targets: `--libcachesim` (both its CSV and
  `oracleGeneral` forms) and `--qwen-bailian` reference a turn's
  `full_output_blocks` after its `full_input_blocks`, at that turn's own time.
  Mooncake stays prompt-only

  **Raised by the user, and the deciding question was "what does a real
  deployment do".** Answer, from our own notes: vLLM's offloading connector
  calls `prepare_store` **after each forward pass**
  (`knowledge/kv_IO_pattern.md`), offloading newly-computed blocks rather than
  waiting for a reader. So a generated block is inserted at the turn that
  generated it.

  That makes the previous projection wrong rather than lossy. The old argument
  — recorded in `research.md` D1 and `simulator.rs` — was that a turn's output
  reappears in the next turn's prompt, so no key is lost. Two things defeat it:
  every store landed **one turn late**, and the **last turn of every session**
  generates output nothing reads again, so it appeared **nowhere** while a real
  cache still held it. Both understate capacity pressure and eviction
  opportunity, which is exactly what these two targets exist to measure.

  **Mooncake is the exception, on conformance grounds**: `len(hash_ids) ==
  ceil(input_length / block_size)` holds on 2 328 of 2 328 upstream rows, so
  appending generated ids yields a file any reader deriving prompt length from
  `hash_ids` reads wrongly. It keeps `output_length` as a token count and drops
  the identities, and that stays a declared loss.

  **Measured consequence on the shipped example at `--until 10`**: libCacheSim
  reaches 657 302 references over 417 764 distinct objects against Mooncake's
  651 893 and 413 044, the difference being exactly the 5 409 generated-block
  references. The two cache targets now agree with each other exactly
  (independent writers, one stream) while Mooncake sits below both by that gap
  — so `quickstart.md`'s cross-check changed shape rather than disappearing.

  **Two declared losses were reworded because they were misleading**, both
  prompted by the user. "The generated run" is no longer a loss for these two
  targets — it is carried — and what remains is the *read/store distinction*,
  which no cache can observe. And a trailing partial block was never unusable:
  its **key is kept and cached like any other**, and only how full it was is
  lost, which costs byte-capacity rounding in libCacheSim and a lossy round
  trip.

  Tests: a generated block is an access at its own turn and again as the next
  turn's prompt read; a final-turn output is referenced exactly once where it
  previously appeared not at all; a turn whose only blocks are generated is
  written rather than dropped-empty. Every affected figure in `quickstart.md`
  was re-measured, including the simulator's own agreement — at `--until 30`,
  `requests=5968 accesses=1936694 working-set=1074280` against the run's own
  reported counts.

- [x] T062n [US2] Rename the `--simulator` projection to `--qwen-bailian` and
  write the format's **full** eight-field record, not the four
  `apps/eviction-replay-benchmark` happens to read

  **Both halves were the user's calls**, taken after I reported the situation:
  the name, and completing the record.

  **The name was the worst in the set.** `--mooncake` and `--libcachesim` name
  formats; `--simulator` named one consumer of ours, vaguely, and the user's
  reaction was "I had no idea what that was". The shape is not ours at all — it
  is Alibaba's anonymized Bailian usage trace
  (`qwen-bailian-usagetraces-anon`), which the benchmark's own loader says in
  its first line and prints as `qwen-file`. Renamed throughout: flag, `--to`
  value, module `qwen.rs`, `QwenWriter`/`QwenStats`/`QwenRecord`,
  `tests/qwen.rs`, and the docs. No back-compat alias: nothing outside this
  repository had used the flag.

  **The record was a subset of one reader's needs.** The format carries
  `timestamp`, `turn`, `input_length` and `output_length` beside the four
  fields the benchmark reads, and that reader **ignores** them
  (`apps/eviction-replay-benchmark/src/replay.rs:18`) — a fact about the
  consumer, not the format. We dropped four fields we had. All eight are now
  written, in the format's own documented field order. `timestamp` carries full
  `f64` precision rather than the published capture's one-decimal style, on the
  same reasoning already recorded for Mooncake's 3 s tick: a captured file's
  precision is a property of that capture.

  **Consequences, measured rather than assumed.** The loader reads the fuller
  file unchanged — `requests=6109 accesses=1979669 working-set=1119473`,
  identical to before — which is the check that the extra fields are ignored
  rather than mis-parsed. The file grows from 40.8 MB to **41.3 MB** at
  `--until 30` (I had guessed 47.8 and had to correct it; the guess was in a
  document whose header promises measured figures). And **virtual time stopped
  being a declared loss**, because it is now carried — one fewer entry in this
  projection's `declared_losses`, asserted in both directions so a stale entry
  cannot survive.

  `contracts/trace-interop.md` gains a Qwen-Bailian section, which it lacked
  while the target existed: the contract is normative for every projection
  target, so its absence was already a gap rather than something this task
  invented.

- [ ] T062i [DEFERRED] Implement the WekaTrace reader in
  `crates/workload-trace/src/weka.rs`, reading one **session** per
  line into invocation records. Read-only by decision. Must **check**
  `hash_id_scope` rather than assume it — the published trace says `local`,
  and a revision declaring a global scope would change what an import means.
  Must not invent an input/output split, since the format does not distinguish
  them
- [ ] T062j [DEFERRED] Test in `crates/workload-trace/tests/weka.rs` against a
  small committed fixture, not the 1.85 GB upstream file: `len(hash_ids) * 64
  == in` on every main request, multi-megabyte lines parse (the upstream file's
  first line is 2.76 MB), and a `subagent` group's nested `requests` are read
  in trace order. A fixture whose sessions are prefix extensions of each other
  must import as **nested** chains, matching T028's property from the other
  direction
- [x] T062k [US2] Add a projection section to `quickstart.md`: emit the shipped
  example to Mooncake and to libCacheSim, and check the reuse-preserving
  assertion by hand. Needs only a Rust toolchain, so it belongs with scenarios
  1-3

  Scenario 3c, every number in it **run** rather than illustrated.

  **The projections cross-check each other.** The libCacheSim and Qwen writers
  agree exactly on distinct objects and accesses while sharing nothing but the
  record stream, and Mooncake is lower than both by exactly the generated run
  it cannot carry. Two independent agreements and one explained difference, so
  the comparison is a check rather than a restatement.

  **The hand check the task asked for had to change shape, because the obvious
  form of it is vacuous — measured, not suspected.** Comparing a shared key's
  identifier at its position in two sessions' rows proves nothing here: on this
  run **all 26 902** cross-session shared prompt keys sit at a *single* prompt
  position across every session that reads them, and **none** at differing
  positions, because shared objects are prompt prefixes and their position is
  fixed by the prefix layout. A per-session renumbering restarting at zero
  therefore reproduces the same number at the same position and passes. This is
  the same weakness already annotated on
  `the_prompt_prefix_structure_survives_the_conversion`, now with a figure
  behind it.

  So the documented check is the **global identifier count**: 413 044 distinct
  Mooncake identifiers, dense over the whole output, which is the figure the
  run itself reports. A per-session renumbering would give **1989** — the
  largest session's own distinct-key count — so the defect reads as a **208x**
  collapse that cannot be mistaken for noise.

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

**T069 done, and the file it names deliberately does not exist.** The task
asked for `workload-node-agent/src/payload.rs`, but the agent already uses
`workload_gen::payload::PayloadBuffer` (T039), and a second copy would be
exactly the duplication T068a exists to prevent: two pre-filled buffers could
drift in their block layout, and a divergence there would look like a cache
returning wrong data. So the agent reuses it, and the task is recorded as done
by reuse rather than by a new file.

**The real gap was the other half: the stamp was written and never read.** Only
keys crossed the network already — the buffer is pre-filled locally and the key
stamped at a known offset. But nothing ever checked a *loaded* block against
the key it was loaded for, and the pre-fill is one repeated byte, so **every
block in the buffer was interchangeable**: a cache returning the wrong block,
or no block, would have produced a run indistinguishable from a correct one. A
stamp nobody reads proves nothing.

Added `MEMCPY_DEVICE_TO_HOST`, `PayloadBuffer::read_stamp`, and
`--verify-payload` on both the generator and the agent. Verified live against a
cold cache:

```
cold  missing 8, granted 8, blocks_written 8
warm  resident 8, blocks_read 8
verified 8 blocks, 0 mismatches
```

The counters carry `payload_mismatches` with `payloads_verified` beside it as
its denominator, and the test asserts that **every** loaded block was checked
rather than some — a verification that silently no-ops is worse than none,
because the run then *claims* the data was checked.

**A mismatch invalidates the run**, unlike every other cache-facing figure. A
miss, a declined reserve and an eviction are outcomes; a block carrying the
wrong key is Certus returning wrong data, so the report leads with `WRONG DATA`
before any throughput and `is_valid()` is false. It is also independent of
Certus's own `integrity-check` feature by design: a check sharing an
implementation with the thing it checks shares its bugs.

Two caveats stated rather than buried: verification **implies stamping** and
wants a **cold** cache, since a block stored by a non-stamping run holds the
fill byte and checking it would report a mismatch that is the instrument's own
fault; and it costs a device-to-host copy per key, so it is opt-in for FR-070's
reason.

**Narrowed the provenance hash while here.** Editing a *test* was invalidating
deployed agents, which is friction with no safety in it — a file compiled into
a separate test binary is never linked into the agent. `tests/` and `benches/`
are now excluded, taking the hashed set from 61 files to 44. Inline
`#[cfg(test)]` modules inside `src/` are still covered, since a path cannot
tell them from the code around them.

**T068/a/b/c done. The agent drives a real Certus, end to end.**

**T068a came first, because it is the load-bearing part.** The reactive rule
was inside `live::consume`, so an agent written beside it would have been a
*second* implementation — free to drift, with a fix applied on one side and
forgotten on the other surfacing as a local/remote difference that looked like
a property of the network. It is now `workload-gen`'s `exec::TurnExecutor`, and
both paths call it. The local path was re-measured after the extraction and is
**byte-identical** to before (140 resident / 88 miss of 228, 76 written, 12
reserve declines), so the refactor is behaviour-preserving rather than merely
compiling.

`LaneStats` now *holds* `workload_wire::frame::Counters` instead of duplicating
those fields, so a local figure and a remote figure are the same measurement
rather than two that happen to be named alike (T068c).

**Measured against a live agent and a live server** (`tests/live_agent.rs`,
`--ignored`):

```
cold  missing: 8, granted: 8, blocks_written: 8
warm  resident: 8, blocks_read: 8
counters: 7 requests, 56 refs, read 8 blocks, wrote 8 blocks
  CHECK 30us  TOUCH 14us  RESERVE 33us  COPY_TO_STORE 663us  COMMIT_STORE 34us  LOOKUP 237us
```

Eight fresh keys all miss and are stored; the same path again comes back
**fully resident and read back**, which is the assertion that distinguishes
"the agent sent frames" from "the agent reached a cache". The per-operation
shape matches the local path's — control operations in the tens of
microseconds, the two data movers far higher.

**Three defects that only running it could have found:**

1. **`Counters.blocks_read`/`blocks_written` were fields nothing populated.**
   The counters said 0 while the per-turn outcomes said 8. They duplicated what
   `lookup_hits` and `transfers_attempted − declined` already say, so they are
   now
   **derived methods**: two representations of one quantity can disagree, one cannot.
2. **A closed connection never returned its mailbox channel.** An agent could
   serve `--lanes` connections *in its whole lifetime* and then refuse every
   client with a connection reset — found by running the test twice. Channels
   and payload slots now go back to the pool on drop, including when `accept`
   fails part-way.
3. **The provenance check fired on a genuinely stale binary** — mine, two
   minutes old (`70e22ba5…` against `448629b9…`), refused by name. Good
   demonstration, and also the friction predicted: the agent must be rebuilt
   after any source edit.

`Stats` returns serialized V2 histograms plus the counters (T068b), and the
test deserialises them, checks each histogram's length against its reported
request count, and confirms a serialize/deserialize round trip preserves
quantiles — because a multi-node merge that silently changed the numbers would
be worse than no merge.

**T067 done, 12 handshake tests; 51 in the crate.**

**`build_id` had to become a *source* identity, not a binary digest.** The
contract called it "a 32-byte digest of the agent binary", but the generator
and the agent are different binaries, so their digests differ by construction
and the check could never pass. **FR-051 asks the right question** — whether
the daemon "was built from the same **sources** as the generator" — so the
identity is a build-time source id: commit, plus a hash of the tracked diff,
plus a hash of the porcelain status.

It is computed **once**, in `workload-wire`'s build script, and both binaries
obtain it by depending on that crate. Two build scripts computing it
independently could disagree — a stale cache on one, a different working
directory on the other — and a provenance check that can disagree with itself
is worse than none.

**The first version asked git, and that was the wrong instrument.** Asking
"what happens to a build from a tarball?" exposed three faults at once, all
fixed by hashing the source files directly instead:

1. **A tarball has no git.** The build still succeeded, but the identity fell
   back to `unknown` and every multi-node run from a release tarball would have
   been refused — a cost imposed by the mechanism rather than by the
   requirement.
2. **Untracked files were invisible.** A new file on one node and absent on the
   other changed behaviour without changing the identity, which is precisely
   the divergence the check exists to catch.
3. **Cargo could not know when to re-run the script.** `.git/index` moves on
   `git add`, not on a bare edit, so an uncommitted change could leave a stale
   identity compiled in.

The identity is now a digest of every `.rs` and `Cargo.toml` under
`apps/workload-generator/crates/`, sorted by path, each file contributing its
path as well as its bytes so that moving code between files changes it. Each
file is declared to cargo, which closes fault 3. **Verified on a git-free
copy**: the build succeeds and produces a real identity (`src:58:948426:…`)
rather than `unknown`, a source edit of ten bytes moves the fold, and restoring
the file brings it back.

**The boundary that is not covered** is workspace dependencies outside this
application — a node running a different `shmq-dispatcher` would not be caught.
That is deliberate: hashing the whole repository would make the identity change
on every unrelated edit, and a check that fires constantly gets ignored within
a week.

**Cost, measured rather than assumed.** It hashes 58 files and 948 KB in **1.2
ms** (0.5 ms walking, 0.7 ms folding), and only when one of those files
changes, since each is declared to cargo — a no-op rebuild re-runs nothing, and
an edit that does trigger it already costs ~0.43 s of recompilation, so the
hash is ~0.3% of a bill the edit was paying anyway. It does **not** touch the
Certus source. For scale: every `.rs` and `Cargo.toml` in the whole repository
is 410 files / 4.6 MB at **189 ms**, and including SPDK's C and the Python is 9
819 files / 153 MB at **447 ms** — of which 181 ms is the directory *walk*
against `deps/` rather than the hashing. So widening the scope is affordable;
the reason to stay narrow is identity churn, not milliseconds.

**It is strict, and the cost is honest**: a comment change anywhere in these crates
invalidates a deployed agent, so a multi-node run needs the agent redeployed
after any edit. That is the price of a check that cannot be talked out of
firing by "it is only a constant", which is how staleness gets in.

**Unknown provenance is refused, not assumed** — it now means the source files
could not be read at all, which is a real problem rather than a portability
case. `WORKLOAD_SOURCE_ID` overrides everything, for a packager with a better
answer.

**Both ends check**, which is what makes a refusal legible: the agent replies
with a non-zero status *and always sends its own identity even while refusing*,
so the generator's message can name both sides. Every refusal names the node,
since on a cluster the useful part is which machine is wrong. The protocol
version is checked **first**, because if the versions differ the rest of the
reply may not mean what it appears to and reporting a build mismatch would send
an operator after the wrong problem.

Capacity is checked too: more lanes than channels is refused because the
mailbox is depth-1 per channel, so over-subscription does not fail — it
**serialises silently**, which reads as a slow server rather than a
misconfigured run. A block-size disagreement is refused for the same shape of
reason.

Not cryptographic, and does not need to be: the failure being prevented is an
accident — a node left running yesterday's build — and anyone able to replace
the binary can replace the identity it reports.

**T066 done, 10 server tests; 37 in the crate.** Transport only — it reads a
frame, dispatches and replies, and knows nothing of mailboxes or keys. The work
is a `Service` the agent implements, which is what lets the whole protocol be
driven in-process with no agent and no accelerator, including a full
client↔server conversation over an in-memory pipe and a live one over TCP.

**`&mut self` is how the causal ordering rule became structural.** `Service`
takes `&mut self` and is built per connection by `ServiceFactory::accept`, so a
handler cannot be shared across a connection's frames without the compiler
objecting — the thread pool that would silently reorder a session's turns
cannot be written by accident rather than merely being warned against. Per
connection also because a connection is a lane and a lane claims its own
mailbox channel, which is depth-1.

**A design flaw found by a test that hung.** `serve` joined its connection
threads on the way out, but a connection thread blocks reading the next frame,
so one idle peer prevented teardown indefinitely — and FR-053 requires teardown
to be *verified*, a teardown bug having already invalidated an A/B series in
this repository. Accepted sockets now carry a read timeout, and a timeout
**between** frames is the moment to check whether the run stopped, while a
timeout **part-way through** a frame is retried because the peer is mid-send
and giving up there would make a slow network indistinguishable from a broken
one. The same test now finishes in 0.01 s instead of never.

A protocol violation closes the connection rather than replying: there is no
error frame, and inventing one would let a peer keep a connection alive by
sending nonsense. A close *between* frames is how a run ends; a close
*mid-frame* is `UnexpectedEof`, and conflating the two would end a run quietly
as though the generator had finished. `Shutdown` is answered **before** the
loop returns, since a close is what FR-064 reads as a lost node and an orderly
stop would otherwise be reported as a failure.

**T065 done, 12 client tests.** Two properties are asserted rather than
assumed. Pipelining depth is proved not to change what is submitted — the
frames at depth 1, 8 and 32 are byte-identical bar their correlation ids, which
is FR-072 in its transport form. And a reply is **matched by correlation id
rather than by arrival order**, tested with replies queued newest-first,
because the contract permits a multiplexed connection and an in-order stream
must not become an unstated assumption.

`TCP_NODELAY` is checked on a real loopback socket, since it is the one
property an in-memory transport cannot answer. A synchronous call with turns
still in flight is **refused**, because the next frame off the socket would be
a `TurnOutcome` and would be decoded as whatever was asked for — a silent
misread rather than an error. A clean close is reported as `Closed` distinctly
from an I/O error, since a lost node must abort the run and be named (FR-064).

**Ordering: the generator owns one causal rule, and nothing more.** Turn *n+1*
of a session contains the blocks turn *n* stored, so two turns of one session
must not be in flight together. Pipelining preserves that because the agent
takes a connection's frames in arrival order and a lane owns a fixed set of
sessions on its own connection. The symptom of breaking it is quiet rather than
loud: turn *n+1* would check a prefix turn *n* had not stored yet, find it
absent and store it again, so the run completes with inflated store counts and
a depressed hit rate.

**Reordering after submission is Certus-internal and not a generator concern.**
The mailbox has parallel channels and the server runs multiple threads, so two
submitted requests may be processed in either order, and preventing that would
mean one channel and no parallelism — removing the thing the measurement exists
to exercise. No end-to-end ordering claim is made on either path, and neither
should be changed to enforce one. `CHECK`'s `PENDING` state is the evidence
that Certus already expects concurrent stores of one key and reports them.

**T063/T064 done, 15 conformance tests, no agent and no accelerator needed.**
Two allocation hazards are guarded rather than one: `len` is refused against a
bound before anything is sized from it, and `SubmitTurn`'s **key count** is
checked against the bytes actually present before the vector is reserved — a
peer-supplied `u32` would otherwise let a 14-byte frame ask for 32 GB.
Truncation is tested **exhaustively at every prefix length** of every body
rather than at one illustrative cut, because a decoder that stops early returns
a value with its remaining fields zeroed, and a half-decoded `SubmitTurn`
submits a turn whose keys the generator never sent.

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
  mailbox and serve `SubmitTurn` by applying FR-072a's rule against it — check
  the path, load what is resident, store what is absent. It decides no
  *workload*: which keys, which session and which virtual time all come from
  the generator. Exit non-zero if the mailbox is absent
- [x] T068a [US3] Depend on `workload-gen`'s library for the split and encoders
  so the reactive rule has **one** implementation. Two would let the local and
  remote paths diverge and make FR-072's guarantee unverifiable. It cannot live
  in `workload-wire`, a CUDA-free default member, since depending on
  `shmq-dispatcher` there would unify `interfaces/spdk` into the default build
- [x] T068b [US3] Serve `Stats` returning **serialized histograms** per
  `op_kind`, never percentiles: the median of two nodes' medians is not a
  median, so merging percentiles yields a number belonging to no distribution.
  Needs `hdrhistogram`'s `serialization` feature
- [x] T068c [US3] Have the **mailbox-facing code be the only collector** of
  latency and bandwidth, on both paths, and ship its `Counters` back with
  `Stats`. Only the agent is near a remote mailbox, so only it can time a
  `LOOKUP` or count a block that moved; a figure derived from wire timings
  would describe the transport. The counters are **for reporting only** — they
  may not gate validity, steer submission, or re-enter the workload
- [x] T065 [US3] *(also covers the depth-invariance property)* — see below
- [x] T069 [US3] Implement the agent's pre-filled reusable payload buffer in
  `crates/workload-node-agent/src/payload.rs`, reconstructing block payloads
  from the key so only keys cross the network
- [x] T070 [US3] Implement node placement and migration in
  `crates/workload-model/src/sim.rs`: new sessions placed uniformly, a
  migrating session moved uniformly among the others, its stored blocks left
  where they were, and migration inert with fewer than two nodes

**T070 done, 7 tests; 223 in `workload-model`.** `migration_interval` was
already in the schema per session class, and the example input notes the node
*list* is deliberately outside the description — so the workload says how often
a session moves and the deployment says where it can go.

**"Its stored blocks stay where they were" is honoured by doing nothing to
them**, which is the point rather than an omission. A migration changes one
field — the session's node — and nothing else: same chain, same growth, same
turn schedule. The next turn simply offers the same path to a different node,
where it misses and is fetched or re-stored. Moving blocks would model a data
migration Certus does not perform, and tracking *which* node holds each block
would duplicate state the cache owns and would be wrong the moment it evicted
one. The test asserts it directly: the keys read with migration are
**identical** to the keys read with it inert.

**Uniform among the others, not among all** (FR-048): drawing over every node
would leave a session where it was with probability `1/nodes`, which is not a
migration. Implemented as `(from + 1 + rand(nodes-1)) % nodes`.

**Migrations are applied lazily, at turn time, and that is exact rather than
approximate.** A session's node matters only when it takes a turn, so a
migration between turns is invisible except through where the next turn goes.
Evaluating it there avoids a second event heap and keeps the ordering `plan.rs`
asserts in one place. Two details make it faithful: the loop applies *every*
elapsed interval, because "uniform among the others" excludes a different node
each time and collapsing two migrations into one would land the session on the
wrong node; and the next interval is measured from the **due** time rather than
from when it was noticed, or an idle session's interval would stretch by
however long it was idle.

**The node draw is taken even on a single node**, so the workload does not
depend on the deployment: a description run on 1, 2, 3 and 8 nodes produces the
*same* key sequence, asserted. FR-049's inertness is therefore a matter of
taking no migration draws at all rather than of skipping placement.

`Simulation::migrations()` is reported because an interval long relative to
session lifetime performs none, and a multi-node measurement that meant to
exercise migration would otherwise look like one that did.
- [x] T071 [US3] Implement agent lifecycle in
  `crates/workload-gen/src/agents.rs`: start before the run and stop after,
  both outside the timed window; idempotent startup that replaces a leftover
  from a crashed run; verified teardown that releases resources even when the
  generator exits abnormally

**T071 done, 9 lifecycle tests + 3 unit; 139 in gen+wire.**

**FR-052 and FR-053 turn out to be two halves of one mechanism**, and reading
them separately makes the second look impossible: a generator killed with
`SIGKILL` runs no code, so no teardown it contains can run. Together they work
— teardown covers the ordinary exit and the panic (a `Drop` guard on `Agents`),
and **leftover replacement is the backstop for everything else**. A generator
that dies without stopping its agents leaves them running, and the next run
finds them, stops them and starts fresh ones. Treating them as separate
features is how a teardown bug survives, and this repository has already lost
an A/B series to one that failed nondeterministically rather than visibly.

**A leftover is replaced even when it is the current build.** The provenance
check makes reusing a *stale* agent impossible but says nothing about a current
one — same sources, still listening, left from a crashed run. That agent holds
mailbox channels claimed for the previous run, a device allocation filled for
its geometry, and counters that would be reported as this run's. FR-052 says
"replaced, never silently reused", and the operative word is *silently*: reuse
would produce a run whose numbers included another run's. One that answers is
asked to stop, which lets it release its channels in order; one that will not
answer is killed.

**`Launcher` is a trait, which is what makes the policy testable.** Ssh appears
in exactly one impl and nowhere else (FR-054), so detect-replace-verify runs
here against a real agent on a real loopback port — including a leftover of the
current build, a stale leftover, a silent squatter, a node that never comes up,
and a `Drop` with no explicit stop. A policy exercisable only on a cluster
would be exercised rarely.

**Teardown is verified rather than assumed** (FR-053):
`Teardown::port_released` records whether the port actually went quiet. An
agent that acknowledged a shutdown and kept its channels would leave the next
run unable to claim them, and that failure would present as a mailbox problem
rather than a teardown one. A busy port is reported, not raised — the run's
measurements are already taken, so the caller should see the whole picture.

**A defect the test found: the client could hang forever.** `Client` had no
read timeout, so a peer that accepted a connection and never answered blocked
the generator indefinitely — the run neither finished nor failed. That
contradicts FR-064 in substance: a node becoming unreachable must abort the
run, and **a hang is not an abort**. Added `DEFAULT_READ_TIMEOUT` (30 s,
generous because a turn does real work on the far side) plus
`with_read_timeout` for probes, which the leftover check uses at 2 s — a
squatter must be discovered in a moment, not after the default.
- [x] T072 [US3] Implement node-loss handling in
  `crates/workload-gen/src/agents.rs`: abort the whole run, report it invalid,
  name the lost node, exit 3. Never continue on the survivors — a lost node
  makes its sessions' prefixes unreachable, shrinks the migration target set,
  and redistributes load, so any later number describes a different experiment

**The agent now exits on disconnect, not on idleness** — raised as a question
about paced mode and it found a real gap.

Two things were checked before changing anything. An established connection has
**no idle timeout**: `read_header` treats a read timeout with nothing received
as a gap, polls the stop flag and loops, so arbitrarily long think time is
fine. And the client's 30-second timeout only applies while a read is
outstanding, which in paced mode it is not during think time. So a quiet node
was never at risk.

The gap was the other end. `serve` looped on `accept` until stopped, so when
the control process died the connection threads saw EOF and exited while **the
agent kept listening forever**, holding its mailbox channels and device
allocation. Only the next run's leftover replacement cleaned that up, which
means FR-053's "release resources even when the generator exits abnormally" was
*deferred* rather than satisfied.

A closed socket is the right signal, and better than a timeout: TCP reports
that the control process is gone — exited, panicked or killed — without anyone
having to guess how long silence is acceptable. The agent now exits when every
connection it once had has closed, with two guards so it cannot fire wrongly: a
`--linger-secs` grace (default 5) for a generator opening its lanes one at a
time, and the rule applying only *after* a first connection, so a freshly
launched agent is not killed for having no clients yet. That case has its own
much longer `--first-connect-secs` (default 300), because an agent nobody
connects to was launched for a run that never came.

Verified live: an agent whose peer vanished without a `Shutdown` logged *"every
connection closed and none reopened within 3s; the control process is gone, so
releasing this node's resources"* and exited. FR-053 amended to require it, and
to forbid an idle timeout.

**T073b done: a cluster run works end to end.** Driven over TCP through the
agent into a real Certus, exit 0:

```
check   152 resident, 0 pending, 76 miss of 228 (66.7% resident)
lookup  152 returned data (100.0% hit)
CHECK 12us  TOUCH 15  RESERVE 23  COPY_TO_STORE 186  COMMIT_STORE 26  LOOKUP 208
nodes 1 →  127.0.0.1   166 requests, 152 blocks read, 76 written
```

The two moves came first, as recorded: the description is now loaded and the
stop handler installed **before** the node check, and `attach` happens only on
the local branch — so a remote run no longer requires a local mailbox, which is
what FR-079's off-cluster goal needs. `live_report` is extracted, so both paths
render one report and neither can say something the other would not.

**Two gaps the first live attempt exposed.** The generator tried to **ssh to
localhost** and was refused on host-key verification — correct behaviour, exit
4, but it showed there was no way to use an agent somebody else is managing.
`--no-launch` now covers that, with `NoLaunch` skipping both the launch and the
leftover replacement: shutting an agent down and then being unable to start one
would leave the run with nothing to talk to. It weakens FR-052 deliberately,
for the case where the caller owns the daemon's lifecycle, which is why it is
opt-in.

And **`kill -9` on an agent leaks its mailbox channel claims.** The server
cannot know the holder is gone, so they stay claimed until it restarts — the
next agent reported "could only claim 0 of 2 channels". That is the operational
reason FR-052's replacement path asks a leftover to stop rather than killing
it, and a reason not to `-9` an agent.

Also fixed a wrong number rather than a missing one: the remote report printed
`1 of 0 channels` because the node capacity was passed as zero. It is now the
sum the nodes reported, captured before they are stopped.

**T073a done: the remote driver, 3 tests.** Written in the shape FR-079's
single driver will take, not as a counterpart to `live::run`. The producer is
now **shared**, with routing as a closure — locally session-id-modulo-lanes,
remotely `session.node()` — because the only thing that differs between the
paths is where a turn is sent. Scoped threads hand each lane a distinct `&mut
Agent`, so the compiler establishes what a raw-pointer split would have had a
comment promise.

**Two defects the tests caught, both invisible in a single-node run:**

1. **`produce` created the simulation without the node count**, so every
   session was placed on node 0 and a three-node run drove one node. Placement
   belongs to the simulation (FR-048) and the router only reads the answer —
   the node count had to be passed in.
2. **Each node's counters were merged into a local total but never written into
   the lane stats the report reads**, so a completed cluster run reported
   having moved nothing.

Counters sum and **histograms merge**: three stubs recording three samples each
give a merged histogram of nine, asserted — not three, which taking one node's
would give, and not an average. Losing a node mid-run aborts and names it.

**Not yet wired to the CLI (T073b).** `--node` still starts, verifies and
refuses. Two things must move first: `attach` runs before the node check, so a
remote run would still demand a local mailbox — contradicting FR-079's
off-cluster goal — and the description is loaded after the point the remote
branch needs it. `live_report` is extracted ready for it, so both paths render
one report. Attempting the move with little room left produced a tangle that
was reverted rather than committed half-done.

**T073 done: the flags and the exit code, and a gap in this list named rather
than papered over.** `--node` (repeatable), `--agent-port` and `--agent-binary`
are in, and a refused peer is **exit 4**, distinct from a completed-but-invalid
run (3) because the actions differ: an invalid run may be worth repeating,
while a refused peer means the deployment is wrong — a stale agent, a lane
count the node cannot serve, a block size disagreeing with the description —
and repeating it will fail identically. `exit`'s `dead_code` allowance is gone,
since `PEER` is now used.

**No task covered the remote driver**, which is an omission in this list rather
than a deliberate deferral: T063-T074 cover the wire, the handshake, the agent,
placement, lifecycle, node loss, the CLI and an equivalence test — but nothing
routes turns to nodes. It is now T073a, and FR-079 makes it the *single*
driver, so it should be built once in the shape the local path will adopt
rather than as a second driver beside one that is about to be deleted.

`--node` therefore starts and verifies the agents — startup is outside the
timed window (FR-050) — and then **refuses plainly**, naming T073a. A flag that
connected and drove nothing would report a run that never happened, which is
the one outcome worse than an error. The agents are torn down on the way out,
so the refusal leaves nothing holding channels.

**T072 done, 3 more tests; 142 in gen+wire.** `NodeLost` carries the node, what
the run was doing, and the transport's own words, and its message spells out
all three reasons continuing is wrong — because "carry on with the survivors"
is the tempting thing to do and it produces a *plausible* number for a
different experiment.

**Every transport failure counts as losing the node, with no retry.** A clean
close, a timeout, a protocol error: to a run they all mean the node is not
usable, and a retry only extends the window in which the run is measuring a
cluster it no longer has.

`lost_node` is a **separate report field** rather than only prose in
`invalid_reason`, so a sweep driver can act on it without parsing English: a
lost node is worth retrying the run for, whereas an underrun means the
generator itself needs looking at.

**This is where T071's read timeout pays off.** A node that accepts and stops
answering is now detected in bounded time and asserted as such — before the
timeout it would have hung, and a run that neither finishes nor fails cannot
even be *reported* as invalid. A failed run still tears its remaining agents
down, because a run that failed must still leave nothing holding mailbox
channels.

**Two fixture bugs worth recording, both mine.** The first test claimed a dead
node went unnoticed; it had killed the agent but not waited for it to actually
go down, and a connection already accepted served one more frame. The second
surfaced only after that fix: `LocalLauncher::kill` stopped *every* agent, so
starting a second one killed the first — because replacing a non-existent
leftover calls `kill`. The real `SshLauncher::kill` is port-scoped precisely so
it cannot kill another run's agent on the same host, and the fixture had to
model that or it would pass tests the production launcher would fail.
- [x] T073 [US3] Add `--node` and `--agent-port` to
  `crates/workload-gen/src/cli.rs`, with exit code 4 for a refused peer
- [x] T073a [US3] **The remote driver itself**, which no task covered — an
  omission in this list rather than work anybody chose to defer. It routes each
  turn to `session.node()`'s agent, collects `TurnOutcome`s, and merges each
  node's `Counters` and **histograms** at the end (counters sum; quantiles do
  not, so the merge is of distributions and the percentiles are taken after).
  Under FR-079 this **is** the single driver, so it should be built once, in
  the shape the local path will adopt, rather than as a second driver beside
  it. It also needs the producer currently private to `live.rs` — that should
  be shared rather than copied, for T068a's reason
- [x] T073b [US3] Wire `remote::run` into `live_run`. Two things must move for
  it: `attach` currently runs **before** the node check, so a remote run would
  still demand a local mailbox — which contradicts FR-079's off-cluster goal —
  and the description is loaded after the point the remote branch needs it. The
  report builder is already extracted for this (`live_report`), so both paths
  render one report
- [x] T074 [P] [US3] Test in `crates/workload-wire/tests/loopback.rs`:
  submitting a plan through a loopback agent stub yields the same operation
  sequence as the local path — the transport-level form of FR-072

- [x] T092 [US3] Proxy the **local** node through an agent too, so there is one
  transport, one driver and one cleanup mechanism. The generator launches a
  local agent itself without ssh, so `run description.yml` still needs no
  setup; `CLEAR_MEMORY_TIER` became the `ClearCache` frame; the direct mailbox
  path is deleted and `workload-gen` drops its CUDA and `shm-queue`
  dependencies — which also stops it enabling `interfaces/spdk` transitively,
  and makes it a workspace default member
- [x] T092a [US3] Re-point the loopback-equivalence test (T074) at the
  *previous* behaviour as a regression guard

**T092/T092a done, together with FR-080 as the note recommended.**
`workload-gen` links no CUDA — `ldd` on the binary names no `cudart` — attaches
to no mailbox, and is a workspace **default member**, so `cargo build` and
`cargo test --all` now reach it.

**A lane is a connection, and that turned out to be load-bearing.** `Agents`
opened one connection per node, while the local path got its concurrency from
claiming `--lanes` mailbox channels directly. Keeping one connection per node
would have collapsed a four-lane local run to one lane and reported the
throughput as though nothing had changed. It is now `lanes` connections per
node, routed `session % lanes` within the node placement chose — exactly what
the deleted path did — and `loopback.rs` asserts that four lanes submit what
one lane does, per session.

**Where the mailbox-facing code went, and why the dependency arrow flipped.**
`exec`, `opstream`, `payload`, `cuda`, the mailbox `attach` and the CUDA build
script all moved into `workload-node-agent`, which now has a library half.
FR-072a's rule still has exactly one implementation, but it is now
*structural*: the generator has no mailbox path, so there is nowhere for a
second one to live. The agent's dependency on `workload-gen` — documented as
the wrong direction and accepted for the stronger property — is simply gone.

**`op_kind` had to move too, and it exposed a spec/code mismatch.** The
contract said `op_kind` enumerates the plan's kinds numbered from zero; the
code has always sent the **mailbox's** opcode. With `shmq-dispatcher` gone from
`workload-gen` the numbers could not stay where they were, so they live in
`workload-wire` beside the field, and `workload-node-agent` — the only crate
that sees both — pins them to the dispatcher in `tests/op_kind.rs`. The
contract now says what is true.

**Five defects, all mine, and the last three came only from running it.**

1. Per-node counters were grouped by **hostname**, which merged three nodes
  into one whenever they shared a host on different ports — which is how the
  driver tests are written, and how a second run on one host is kept separate.
2. `Agents::stop` returned on the first node that could not be asked to stop,
  leaving the rest of the cluster holding its mailbox channels: FR-053's
  failure by another route.
3. **The agent never released its mailbox channels.** A claim lives in the
  shared segment and outlives the process, so before FR-079 the generator
  released the local ones and the agent — started once per deployment — did
  not. Starting and stopping an agent for *every* run turned that into a leak
  of `--lanes` claims per run: four two-lane runs against an eight-channel
  mailbox and the fifth cannot start, with no agent running and nothing to
  kill. Found by running one smoke test five times.
4. **A refused handshake left its agent listening**, so it waited out its
  linger while the local launcher's backstop killed it first — a SIGKILL
  mid-release, leaking the claims. Startup now stops what it launched, asking
  before killing, and *only* for a launcher that owns the lifecycle: under
  `--no-launch` the operator's own daemon is left alone.
5. **The kill grace was shorter than the agent's own linger** (3s against 5s),
  so the backstop killed healthy agents. It is now derived from
  `DEFAULT_LINGER`, and a test asserts the relationship rather than the number.

**Hardware-verified against a scratch `certus-server-yaml`** — its own mailbox
and device file, so the box's existing server was untouched. Six consecutive
local runs valid; real cache traffic (18 blocks read, 18 written, 47%
resident); `--clear-cache` reporting 18 memory-tier entries dropped;
over-subscription refused with the mailbox still usable afterwards; and the two
modes' validity rules visibly different on one workload — paced keeps its
schedule and is valid, work-conserving reports 33% underruns on the same toy
description and is invalid.

**A trap worth recording, because it cost two runs.** The source-id digest
covers every crate under `crates/`, so editing *any* source file changes it —
and a generator rebuilt after its agent is refused for provenance, correctly.
Build `-p workload-gen -p workload-node-agent` together, every time.

- [x] T092b [US3] Settle the pipelining depth: **no `--pipeline-depth` flag**,
  and the missing half of `node-agent-wire.md`'s requirement asserted instead

**T092b done, 2 tests.** The contract listed `--pipeline-depth` and the binary
never accepted it. Resolved by removing the flag from the contract rather than
implementing it, on three grounds: depth 8 at a ~30 µs loopback round trip is
about 260 000 turns/second against roughly 1 000 observed, so the headroom is
not close; a finite pacing rate makes the window nearly irrelevant, since turns
go out when they are due rather than as fast as possible; and FR-072's
requirement is that depth never reach the producer, which a test establishes
and a flag does not.

**What depth actually buys is hiding round-trip latency, not concurrency.** The
agent handles one connection's frames one at a time in arrival order, so a
deeper window only keeps the socket from idling between turns — which is also
why depth is safe for FR-035 even though two turns of one session may be in
flight on the wire.

`loopback.rs` now varies depth (1, 2, 8, 32) and the four combinations of depth
against lane count, comparing the turns submitted per session. Depth 1 is the
load-bearing end: `Client::submit` drains a reply whenever the window is full,
so every submit after the first waits for the previous outcome and the
transport is round-trip bound rather than pipelined.

**A flake of mine, found and fixed while doing this.** One `pacing.rs` run
failed and then passed seven times, including under load. From the cumulative
counts it was that suite — the one with wallclock bounds, all written today.
Three of its assertions were absolute millisecond bounds, which are statements
about how busy the machine running the test is rather than about pacing. They
are now either **hang guards** (loose, where the load-bearing claim is the
*lower* bound: the run must have waited) or **ratios measured against a
rate-1.0 baseline in the same test** (where the claim is that a faster rate
costs less, which is a comparison and not a duration). The achieved-rate window
is one-sided for the same reason: overshooting the requested rate means the
schedule was not kept, while a loaded machine may legitimately undershoot.

**One stale justification corrected.** `DEFAULT_DEPTH`'s doc said a bigger
window "would only add queueing delay to a latency measurement". FR-066 makes
the mailbox-facing code the sole collector and FR-079 makes that always the
agent, timing its own mailbox requests — so wire queueing is outside every
reported latency figure and the claim is withdrawn. The honest reason not to
raise it is weaker: there is nothing to gain.

**Checkpoint**: all three execution paths work; US1 and US2 are unaffected by
US3.

---

## Phase 6: User Story 4 — Compare eviction policies (Priority: P4)

**Goal**: the same workload under two Certus configurations produces hit-rate
curves that actually discriminate between eviction policies.

**Independent Test**: sweep cache size against hit rate under two policies and
under both ranking modes; the curves are smooth and concave rather than
step-shaped, and the two ranking modes reorder the policies.

- [x] T075 [US4] Implement the `selection` distribution over instance index in
  `crates/workload-model/src/selection.rs`, so its spread sets working-set size
  independently of the pool's key-space size
- [x] T076 [US4] Implement `rank_by: slot` and `rank_by: recency` in
  `crates/workload-model/src/selection.rs`: a slot's popularity is inherited by
  each new occupant, while recency ranking makes heat decay as newer instances
  arrive. Index space stays bounded — numbering by a monotonic mint counter is
  wrong and would mint objects nothing selects
- [x] T077 [P] [US4] Test in `crates/workload-model/tests/selection.rs`: under
  a concentrated `selection`, the realised reference distribution is skewed as
  configured; under `rank_by: recency` an instance's reference rate decays with
  age, and under `slot` it does not
- [x] T078 [US4] Add the sweep-supporting fields to the structured report in
  `crates/workload-gen/src/report.rs`: hit/miss/pending split, distinct keys
  touched, and the effective working-set size, so a sweep can be aggregated
  without scraping terminal text
- [x] T079 [P] [US4] Implement the sweep check in
  `crates/workload-gen/tests/sweep.rs` (or a script under `scripts/`): across
  at least five capacity points spanning a hundredfold range, hit rate rises
  monotonically with **no single step contributing more than half the total
  rise** — and the same check **fails** when `selection` is removed from every
  class, which is what proves the check has teeth

**T079 done, 5 tests (1 `#[ignore]`d); 386 across the three crates.** The check
replays the plan's own reference stream through an LRU simulator at five
capacities, so it needs no server, no accelerator and no cluster and runs in
the ordinary gate. That is deliberately not a test of Certus's policy: it is a
test of whether the *workload* produces a reference pattern two policies could
differ over at all — a necessary condition for the real measurement.

**Getting it to have teeth took three corrections, all found by measuring, and
SC-005 and Scenario 6 are amended accordingly.** Each one had the check passing
a uniform workload, which is the failure this task exists to prevent:

1. **The overall hit rate is the wrong curve.** The reactive rule (FR-072a)
   re-offers a session's whole prefix every turn, so most references are a
   session re-reading what it stored moments ago; they miss below the
   concurrent footprint and hit above it. That is a cliff — measured, ~85% of
   the rise in one step — and it is a cliff whether popularity is concentrated
   or flat, so the overall curve fails for *both* descriptions and separates
   neither. The swept curve is now the **cross-session** hit rate: references
   to a block some other session brought in, which are exactly the references a
   replacement decision decides. 40% in its largest step concentrated, 94%
   uniform.
2. **The ladder must stop below the key space.** The first version ran 1%-100%
   of the key space. At 100% nothing is ever evicted, so the hit rate is 1.0
   for any workload at all, and both curves were pinned there — which flattered
   every shape that reached it and is how a uniform workload first passed. It
   is also the one capacity at which no policy can differ from another, so it
   is the least informative point available. Now 0.1%-10%, still a hundredfold,
   wholly inside the region where eviction bites.
3. **A rise must also be substantial, not merely smooth.** "No step over half"
   is scale-free and so passes a curve that barely moves; a 6-point rise across
   a hundredfold capacity range gives two policies almost nothing to differ
   over. `MINIMUM_RISE` is 10 points.

**Two findings about descriptions came out of it, both now in the quickstart.**
Concentration is not enough — it has to be *scale-free*: an `exponential`
selection produces one hot band and therefore one cliff, while an `empirical`
ladder `[1, 4, 16, ...]` with `interpolate: true` puts equal mass in each
octave of rank, which is a power law over instance index. And popularity being
scale-free is not sufficient either: with `turns` and `uses.count` held
constant every session has the same footprint, so the aggregate working set has
one characteristic size and the curve cliffs there however the pool is
selected. The working set has to be scale-free too.

**"A uniform workload fails by construction" is withdrawn as too strong.** It
fails by *measurement* — at five seeds of five, with the largest step steady
near 90-94% of the rise against 40% concentrated. Checked across seeds rather
than assumed, because recorded history on this hardware includes n=3 sampling
producing conclusions that later measurement reversed; the seed sweep is
`#[ignore]`d for runtime and named in the module docs.

Not a finding, but worth recording since it cost time: clippy 1.96 added
`manual_checked_ops`, which fires on
`components/interfaces/src/iblock_device.rs` (lines 243 and 252) and so aborts
`cargo clippy -D warnings` for any crate that compiles that module — which
under `--features live` includes `workload-gen`. That masked several new lints
in this app until `--no-deps` was used. Both are pre-existing repo code,
unrelated to this branch.
- [x] T080 [US4] Document the policy-comparison procedure in `README.md`,
  including that hit-dependent comparisons need repetition with a stated
  significance test because mint races are preserved deliberately, and that n ≥
  8 is the recorded floor on this hardware

**T080 done. US4 is complete.** `README.md` did not exist — T081 writes its
orientation half — so this created it with the comparison procedure and a note
saying what is still missing, rather than waiting for T081 and leaving the
measurement undocumented in the meantime.

Four sections, because the task's two requirements are only half of what makes
a policy comparison trustworthy here:

- **Why hit-dependent comparisons are not reproducible**, stated as a
  consequence of a deliberate decision rather than a caveat: the plan is
  deterministic but mint races are preserved (FR-035, FR-036), so a run's hit
  rate is a sample. Hence repetition, a significance test stated *before*
  looking at the numbers, and **n >= 8** — justified by the two recorded
  incidents rather than asserted, since a floor with a reason attached survives
  contact with a deadline better than one without.
- **The four checks before quoting a figure**, each of which corresponds to a
  measurement this project has already got wrong: two servers on one mailbox, a
  percentile without its n, a warm tier from the previous arm, and a stale
  agent build.
- **Sweeping capacity**, pointing at T079's check and carrying its three
  corrections forward, so someone writing a description to sweep does not
  rediscover them.
- **Figures that do not combine across nodes** — counters sum, percentiles must
  be merged as histograms, distinct keys must not be added at all. Worth a
  table because the last two are the same arithmetic error one type apart, and
  the report's shape (merged histograms, `distinct_keys` as the generator's own
  count) is otherwise unexplained.

Also ticked T074, whose checkbox was missed when it was committed as
`2e5e7626`; its progress note was already in place.

**T078 done.** The hit/miss/pending split was already there from FR-072a's
work, so what this added were the two fields a sweep cannot do without.

**`distinct_keys` is the generator's count, not the nodes'.** Distinct counts
do **not** sum: a shared prefix block referenced by sessions on two nodes is
one key, and adding each node's count would double it — the same arithmetic
error as averaging two nodes' percentiles, one type along. So it is counted
once, by the producer, over the paths it built. It is `None` unless asked for,
because maintaining the set costs an insert per key reference on the producer's
own path: a throughput run should not pay that, and a sweep needs it, since a
hit rate is only interpretable against the size of the key space it was
measured over.

**`working_set` reports what the `selection` spread actually covers**, per
shared class, as the spread's p90 after truncation — the rank band carrying
most of the mass. Deliberately *not* a count of ranks ever drawn, which would
grow with the run's length and so describe the run rather than the description;
and p90 rather than the mean, because a concentrated distribution's mean sits
near rank 0 and says nothing about how far its references reach.

`None` there is the case a sweep must be able to recognise rather than infer:
it means selection is uniform, so the working set *is* the key space, the curve
will step rather than slope, and the run cannot discriminate between policies
however it is swept. Reporting that as a field is what stops it being guessed
from the shape of the answer.

**T075-T077 done, 5 tests; 228 in `workload-model`.** The `selection`
distribution is over **rank**, so a spread narrower than the pool makes the
working set smaller than the key space while the key space stays as large as
the pool. That is the knob a capacity sweep needs: under uniform selection the
working set *is* the whole key space, so a cache either holds all of it or
thrashes, the curve has a step rather than a slope, and no two eviction
policies can be distinguished — which is the measurement US4 exists to make
possible.

**T075 and T076 were one change, not two.** `rank_by` was inert *by
construction* rather than by omission: a uniform draw is the same distribution
however the ranks are numbered. Only a concentrated distribution over rank
makes `slot` and `recency` differ — under `slot` a position is hot and each new
occupant inherits that heat, under `recency` rank 0 is the newest instance so
heat decays as newer ones arrive. Measured both ways.

**Rejection has a budget, and running out is counted.** Sampling without
replacement from a concentrated distribution has no closed form, and the
obvious alternative — drawing into the shrinking list of remaining candidates —
reshapes the distribution as the list shrinks, so a concentrated draw would
flatten exactly when asked for the most instances. A duplicate rank is
therefore redrawn, and when the budget runs out the remainder is filled **in
rank order**, the least distorting completion available.
`SelectionStats::exhausted_retries` counts it, because that case means the
description asked one session for more instances than its spread covers and the
symptom would otherwise be a hit-rate curve that merely looked a little flat.

**Why the draw is sorted into slot order — the reason that was not written
down.** Asked directly, and the recorded reason (a session's prefix must not be
rearranged as recency ranks change) is the lesser one. Keys are a rolling
prefix, so two sessions share a prefix only if they lay the same instances down
in the same *order*. Returning them in sampled order would have two sessions
holding the same set of *k* instances agree only when their orderings coincided
— `1/k!` — so the shared pools would produce almost no reuse while appearing to
be shared. **A canonical order takes that from `1/k!` to certain**, and it
makes partial sharing systematic: `{3,7}` and `{3,9}` share slot 3 and diverge
after, because the common part of their sets is a common prefix once sorted.
Slot order in particular interacts with `rank_by: slot`, where a concentrated
selection favours low ranks — so the hottest instances also sit at the *front*
of a chain, putting the heaviest sharing at the prefix root, which is the shape
a real workload has. That was not designed; it falls out of the two choices,
and it is now recorded before either is changed.

**Checkpoint**: all four stories are independently functional.

---

## Phase 7: Polish & Cross-Cutting Concerns

- [x] T081 [P] Write `apps/workload-generator/README.md`: purpose, the five
  crates, the emit-only build, and a pointer to `quickstart.md`
- [x] T082 [P] Document `crates/*/src/` at the granularity each crate's role
  earns, and confirm `cargo doc --no-deps` is warning-free:

  - **The three library crates** (`workload-model`, `workload-trace`,
    `workload-wire`) get a runnable crate-level `# Examples`, a runnable
    module-level example in every public module, and a runnable example on each
    principal public type and the entry points a caller drives it through.
  - **The two binaries** (`workload-gen`, `workload-node-agent`) get enough
    detail to read the program: module headers saying what the module is for
    and doc comments on its items. Their `pub` surface exists for reuse
    *inside* this application — `workload-node-agent` depending on
    `workload-gen`'s library so FR-072a has one implementation — not as an API
    for anyone else, so an example is added where it teaches something and not
    as a rule.
  - **Not required anywhere**: an example per trivial accessor, per byte-level
    primitive (`Writer::u8`), or per constant. FFI declarations cannot carry a
    runnable example and are exempt.

  **This deviates from the root `CLAUDE.md`'s "public APIs require doc comments
  with runnable examples", by the user's decision, and the reason is worth
  keeping**: that convention comes from the component framework, where an
  interface is a contract between independently developed components and an
  example is how the contract is pinned. Nothing here is published for others
  to build against — these five crates are one application, and its `pub`
  keywords are mostly crate-boundary plumbing. Taken literally the rule wanted
  552 new examples, including ones for `const HELLO` and for `extern "C"`
  declarations that cannot run, which would have added a large gate cost and a
  lot of near-duplicate snippets to document a contract that has no second
  party.
- [x] T083 [P] Add a doc test asserting that
  `specs/001-synthetic-workload-generator/contracts/workload-input.example.yml`
  parses and validates, so the shipped example cannot drift from the parser
- [x] T084 Run every scenario in `quickstart.md` end to end and correct any
  drift between the guide and the implementation
- [x] T085 [P] Record a Criterion baseline for the per-key plan-generation
  benchmark and note it in `README.md`, so later regressions are detectable
- [x] T086 Final gate: `cargo fmt --check`, `cargo clippy -- -D warnings`,
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

**T062y done, and it found a second wrong check in the same code.** One flag
per format with one destination each, at least one required; `--format` and
`--output` are gone from `emit`, and clippy is clean in both feature
configurations with `--all-targets`.

**FR-055 is amended, deliberately and narrowly.** It said the system MUST emit
the trace "in two containers", which made a Certus-private format mandatory on
every emit run. It now requires the **capability** to write a workload at that
level of abstraction and says the delivery is a projection — which is what
removed the private format altogether (`research.md` D4).

**One file per output, and that is not a convention.** A projection is not a
trace (FR-075b): it is one other tool's shape with nothing beside it to
describe itself, so it is one file and never a directory. A run leaves **no
directory at all** — not even an empty one, which would look like an
interrupted run.

**THE SECOND DEFECT: the pre-flight was sizing the wrong artifact.**
`check_size` used `projection.plan_bytes` — the size of the **canonical plan**,
which only the `plan` subcommand writes — for a run that writes projections.
Measured against what `emit` actually produces, that figure is **2.8x low for
JSONL and 3.9x low for libCacheSim CSV** (8.2 bytes per key reference assumed,
against 23 and 32 measured). Low is the dangerous direction for a check whose
stated job is to stop a run filling a filesystem: between roughly 400M and 2.7G
key references it would admit a run that then ran out of disk. Not exercised,
because the arithmetic was never compared against a real output.

Now sized per output from constants calibrated on a measured 30-second run, and
**checked per filesystem**, grouping destinations that share a device. Grouping
is the only version right in both directions: summing three destinations
against one mount refuses a run that fits, and checking each alone admits two
large outputs that together overflow a mount they share. Both are now tests.

**The measured per-reference costs are worth recording, because one is
counter-intuitive**: Mooncake 7, Qwen-Bailian 21, libCacheSim CSV 32.
libCacheSim CSV — a standard format — is the **most expensive of the three**
while carrying the least, because it writes a row per block reference rather
than per request. Rounded up in every case: an estimator that reads low fails
at the one job it has.

**Free space stayed injectable.** `check_sizes` takes the free-space lookup as
a parameter, because on a box with less free space than the 32 GiB ceiling the
free-space branch always fires first and the ceiling branch would never be
reached by a test. That property was already there and would have been quietly
lost by reading `statvfs` inside the check.

**`free_bytes` no longer creates the directory it measures.** It did, to give
`statvfs` a path that exists — which meant a *refused* run left an empty output
directory behind, indistinguishable from an interrupted one. It now measures
the nearest existing ancestor, and a refusal is verified to leave nothing.

**T086 done. The gate passes, and Phase 7 is complete.** Run on 2026-09-17:

| Check | Result |
| --- | --- |
| `cargo fmt --check` | clean for this feature; **38 files elsewhere in the repo are unformatted**, none of them touched by this branch (checked by intersecting the diff-against-`origin/unstable` file list with the drift list — the intersection is empty) |
| `cargo clippy --no-deps -- -D warnings` (default members) | clean |
| `cargo clippy --no-deps --all-targets -- -D warnings` on `-p workload-gen` in all three feature configurations and `-p workload-node-agent` | clean |
| `cargo doc --no-deps` | clean for this feature; **2 pre-existing warnings** in `lib/shm-queue` (a private intra-doc link) and `lib/component-core` (an unresolved `deactivate` link), neither in a file this branch touched |
| `cargo test -- --test-threads 1` (default members) | **all pass**, 922 tests over 62 binaries |
| `cargo test -p workload-gen --features live -- --test-threads 1` | **all pass**, 104 tests, 1 ignored (the 5-seed sweep) |
| `cargo test -p workload-gen --no-default-features` | **all pass**, 25 tests |
| `cargo test -p workload-trace` | **all pass**, 60 tests |
| `cargo test -p workload-node-agent` | **all pass**, 2 ignored — both need a live server, a live agent on 7420 and a GPU, and say so in their `#[ignore]` reasons |

**T005's two expected pre-existing failures behaved differently than predicted,
and the difference is worth recording.** It warned that `cargo test --all`
fails in `dispatcher-p2p` on `libgdrapi.so.2` and that `cargo clippy -p
workload-gen -- -D warnings` fails on two `manual_checked_ops` lints in
`components/interfaces`. Now:

- The `manual_checked_ops` problem is **real and still latent**, and it is why
  every clippy invocation above carries `--no-deps`. Without it the lint fires
  on repo code this feature does not own and aborts the run, which silently
  masked several of this app's own lints earlier in the branch.
- `cargo test --all --no-run` **builds clean** — the GDRCopy link failure is a
  runtime one, so `--all` still cannot be the gate on a box without the
  library. The gate this feature holds is plain `cargo test` plus explicit `-p`
  runs, as T005 concluded.

The two `cargo doc` warnings were **not** predicted by T005 and are also not
this feature's; both are in `lib/` code last touched by the crate-move commit
`dcbfc4c2`. Recorded rather than fixed, on the same principle: a gate that
fixes unrelated code stops being a gate on this feature.

**T085 done, and the baseline is a range rather than a table of digits, because
one run of this benchmark is not a measurement.** Recorded in `benches/plan.rs`
and summarised in `README.md`. Per key reference, three consecutive runs:
`short_sessions` 11.4–12.3 ns, `long_sessions` 4.2–5.7 ns, `many_shared`
5.25–5.45 ns, `projection_10k_sessions` 97.5–112 µs.

**The headline claim survives the spread easily**: against Certus's measured
~5.6 µs/key, plan generation is 0.08% to 0.22% of the budget, so one thread has
450x to 1300x of headroom. Principle I's "never the bottleneck" is quantitative
now, and a plan queue that reaches zero means something is wrong rather than
that generation is slow. The benchmark's own prediction also holds in every
run: long sessions are 2.0–2.9x *cheaper* per key than short ones.

**The finding worth keeping is that Criterion's change verdict is unusable
here.** Three identical runs, no code change between them: `long_sessions`
reported +21% and then +12%, `short_sessions` −0.6% then +7.0%, every one at p
= 0.00. The first run of a session is the worst — 13% and 47% outlier rates,
and a `long_sessions` confidence interval eleven times wider than the next
run's. So the recorded procedure is: run twice, discard the first, treat under
~35% on `long_sessions` or ~8% on the others as noise, and for a real
before/after use repetition with a stated test. That is the same n ≥ 8 rule the
README already states for hit rates, arrived at independently on a benchmark
with no server, no cache and no network in it — which is worth knowing, because
it means the rule is about this hardware and not about mint races.

**The 2026-09-15 baseline was stale rather than wrong** and is superseded:
`short_sessions` 19.0 → ~11.5 ns and `many_shared` 7.9 → ~5.3 ns, about 1.5x
faster, from US4's selection work rather than from anything aimed at this
benchmark. A baseline recorded once and never re-run had drifted into being
misleading in two weeks, which is the argument for T085 asking for it in the
README where someone will see it.

**T084 done, and it found a real defect rather than only guide drift.** Every
command in `quickstart.md` was executed on 2026-09-17 except Scenario 5 (needs
a cluster) and Scenario 4's *successful* case (see below). The guide now says
at the top which commands were run, and every number in it is measured.

**THE DEFECT: `--format` was parsed and never plumbed through.** It was used
for exactly one thing — checking that the build supported the container it
selected — and then the emit path wrote its default unconditionally, so
selecting the other container produced no such file and left its record count
permanently null.

This is the third instance in this feature of the same failure shape — a flag
accepted, validated, and not plumbed through (`--batch-keys` was the first,
`rank_by` the second) — and it survived for the same reason each time: **no
test drove the flag**. `emit_determinism.rs` exercised `emit` extensively and
never passed `--format`.

`--format` is gone now, replaced by one flag per format with its own
destination (T062y), which removes the selector that could be half-wired at
all: a flag that names its own destination cannot be "accepted but not plumbed
through" without the destination staying empty, which a test notices. The
generic lesson stands and is the reason every output flag in
`emit_determinism.rs` is now driven by name: **a boundary is an argument, not a
check.**

**Drift corrected in the guide, scenario by scenario. Every item was a command
that failed or a number that was wrong** — not one was cosmetic:

- **Scenario 1**: the effective mean was quoted as ~5.57 and the refusal's as
  4.218. Both are the *continuous* truncated means; the tool reports the means
  of the **rounded** variable, 5.1435 and 3.9515, because a count is integral.
  The example file's own comment already recorded this correction and the guide
  had not been updated. The guide now states both and says why they differ.
- **Scenario 2**: `plan` requires `--until`, which the guide omitted, so its
  first command failed. Worse, the third command passed `--batch-keys` and
  `--lanes` to `plan`, **which has neither flag** — an `unexpected argument`
  error, not a passing comparison. That half of FR-072 cannot be checked
  through `plan` at all, and pretending it could was the guide asserting a
  property it never tested. It now points at
  `op_stream::batch_keys_changes_the_requests_but_not_the_workload`, which does
  test it, and says why the CLI cannot. Also: `plan` prints a fingerprint, so
  `cmp` is the expensive form of the check, and these files are 98 MB at
  `--until 60` and 535 MB at 300 — worth saying before someone runs it four
  times.
- **Scenario 3**: `--until 3600` on the shipped example projects a **7.33 GiB**
  plan; the guide now uses 30 seconds and shows `validate --until` as the
  pre-flight. And `eviction-replay-benchmark`'s flag is `--file`, not
  `--trace`.
- **Scenario 4**: `--lanes 16` against this host's 8-channel server is refused
  (correctly, exit 2, naming both figures). A **debug** build did not finish a
  5-virtual-second run in ten minutes, so the guide now says `--release` —
  every figure this scenario prints is a throughput, and a debug throughput is
  a figure about the debug build.
- **Scenario 5**: `--node` takes a **bare hostname**. The guide passed
  `node5:/dev/shm/certus-shmq`, which would be used verbatim as an ssh host;
  the mailbox comes from `--shm-path` and the port from `--agent-port`. The
  agent's flags are `--bind` and `--port` (default **7420**, not the 9420 the
  guide showed), and the hand-start line contradicted its own comment saying
  the launcher does it — it belongs to `--no-launch`, which is now what it
  says.
- **Scenario 6**: correct as written. 25 s for the gate, 122 s for the seed
  sweep, matching its "~2 min".

**Scenario 4's successful case could not be completed, and why is worth
recording: a generator that dies mid-run leaks its mailbox channel claims.**
The host's server had 4 of 8 channels already held by a dead client before this
session touched it; the debug run that had to be killed took the other 4. The
claim lives in the mailbox's shared memory and nothing reaps it, so the count
only recovers when the server restarts. The generator's *refusal* is correct
and well-worded ("could only claim 4 of 8 channels; another client holds the
rest") but does not hint that the holder may be dead, which is the state an
operator will actually be in. Documented in the guide, including that the wrong
fix is to lower `--lanes` — that measures a different concurrency from the one
asked for, which is exactly what the lane check exists to prevent. **Not filed
as a task: whether the mailbox should reap a dead client's claims is a
`shm-queue` question, not this feature's.**

**T083 done**, as a doc test on `WorkloadDescription::from_path` that loads the
normative example through `CARGO_MANIFEST_DIR` and asserts it validates with no
refusals. `tests/description.rs` already asserted the same thing, so this is
the *documentation* half the task asked for: the example is what `from_path` is
for, and reading the real file rather than inlining a copy is the point — an
inlined copy is the drift the task exists to prevent.

**T082 done, 62 doc tests** (41 `workload-model`, 10 `workload-trace`,
11 `workload-wire`, 1 `workload-gen`), and `cargo doc --no-deps`
is warning-free on all five crates.

**An audit came first, and it is what produced the scoping decision above.**
Every public item already had a doc comment — `#![warn(missing_docs)]` is on in
four crates, and is now on in the fifth (`workload-node-agent`'s `main.rs`,
which had been missed) — so what was missing was *examples*: 552 of 583 public
items had none. The gap was not evenly spread. **22 modules had no runnable
code at all**, including the crate root of all four library-bearing crates and
every one of `workload-wire`'s four modules, while `workload-model` was already
largely covered. So the work was module-shaped rather than item-shaped.

What was added, by crate:

- **`workload-model`**: a crate-root example — parse, simulate, record — plus
  the seed-reproducibility pair, since determinism is the property the whole
  crate exists to have. Its modules were already covered.
- **`workload-trace`**: a crate-root example that projects a run onto a target
  format in the same pass, because "a projection is not a trace" is the thing a
  reader most needs and cannot see from the module list. Module examples for
  `record`, `mooncake`, `cachesim` and `qwen`.
- **`workload-wire`**: a crate-root example running a real client↔server
  conversation over loopback, and module examples for all four modules —
  `frame` (round trip plus both refusals), `handshake` (both ends, and two
  refusals), `client` (the pipelining window), `server` (an accept loop, and a
  frame before `Hello`).
- **`workload-gen`**: one example, on `EmitReport::render`, because the report
  is the artifact everyone reads and its shape is otherwise only asserted in
  tests.

**Three of the new examples are the documentation, not decoration.** `client`'s
shows that the *d+1*-th `submit` returns a displaced outcome, which is the
API's one surprising signature; `server`'s shows a pre-`Hello` frame closing
the connection; and `report`'s asserts that the emit report's JSON contains no
latency or lane field at all — FR-071's omitted-not-zeroed rule, checked rather
than described.

**Two mechanical traps, recorded because both cost a cycle.** `Writer::bytes`
returns `&mut Self` while `Writer::frame` takes `self`, so the natural
one-liner does not compile; and the `jsonl` example's first assertion guessed 2
sessions from `pool: {size: {exact: 2}}` and measured 9 — `pool.size` is how
many sessions are *live at once*, not how many a span contains. The corrected
example says so, since that is exactly the misreading a description author
would make.

`workload-trace`'s crate-root example writes through `&mut Vec<u8>` and takes
the bytes back after `finish()` releases the borrow, rather than through a
`sink_mut()` accessor added for the example's convenience — a doc example
should not grow the API it documents.

**T081 done.** The four sections the task asked for now sit above T080's
comparison procedure, and the placeholder blockquote saying orientation was
missing is gone.

Two of them say more than "what this is", because the orientation a reader
needs here is not a feature list:

- **Purpose** is stated against the alternative — replaying a captured trace —
  since that is the thing a reader arriving at a workload generator will assume
  it competes with. The two consequences are the design: a small description
  expands into an unbounded plan, and the plan is deterministic while the
  *outcome* is not. That asymmetry is stated up front and linked forward to the
  comparison section, because it is simultaneously the property that makes A/B
  work possible and the trap that makes it easy to get wrong.
- **The crate table** leads with the default-member column, because the split
  is not organisational and a reader who reads it as organisational will put
  the wrong code in the wrong crate. The CUDA-free boundary is what makes
  FR-072 compiler-enforced: the simulation core cannot see a mailbox, a device
  or a container, so a second execution path cannot silently diverge from the
  first.

Also added a stated non-goal — it is not a trace replayer and claims no
fidelity to any particular captured trace — because the branch's own history is
largely a record of trying to make that claim and failing, and a reader who
infers it from "synthetic workload generator" will over-trust the output.

**Verified rather than described**: `cargo build -p workload-gen
--no-default-features` was run, and its `--help` confirms the emit-only build
drops `run` and keeps exactly `emit`, `validate` and `plan`. The
"where to go next" links were checked against what is actually on disk — the
description schema is in `data-model.md`, not in `contracts/`, and `research/`
holds only `population/`.

---

## Phase 8: Per-instance addressing and the hardware file (FR-081, FR-082)

**Goal**: a run can name the Certus instance it drives, so a machine holding
several — one per NUMA domain, or one per NVMe as `deploy/multi-instance/` does
— can be driven as the several independent caches it is.

**Why this exists**: FR-079 collapsed every node onto one agent transport and
left the addressing as it was — a bare hostname, with one global `--shm-path`
and one global `--agent-port` applied to every entry. So `--node node5 --node
node5` produces two *identical* specs, the second agent's startup takes the
first for a leftover and shuts it down, and a two-instance run drives one
instance while reporting two. The deployment is not merely awkward to express;
it is unrepresentable.

**Two things to notice before starting.** The pre-existing `cli.md` already
said `--node <host>:<mailbox>` and was right; the implementation shipped a bare
hostname and T092's documentation pass then edited the *contract* down to match
the code, which is the wrong direction to reconcile a mismatch. And the
hostname-keyed per-node counter defect found during T092 was this same finding
arriving early — it was fixed as a grouping bug without drawing the conclusion.

**Independent test**: two Certus instances on one host, different ports and
different mailboxes, both driven in one run; per-instance figures
distinguishable in the report; and a session migrating between them recorded as
a real cache miss.

- [X] T093 [US3] Replace `--node`/`--shm-path`/`--agent-port` with a repeatable
  `--instance <host[:port][:mailbox]>` in `crates/workload-gen/src/cli.rs`. A
  component that parses as a `u16` is the port, one starting with `/` is the
  mailbox, so all four shapes are unambiguous without an empty middle field.
  `--agent-binary` stays global: the handshake requires every instance to run
  the same build (FR-051), so a per-instance binary is a misconfiguration the
  protocol already refuses
- [X] T094 [US3] Refuse a duplicate `(host, port)` across instances, naming
  both entries. Without it the second agent's startup shuts the first down as a
  leftover (FR-052) and the run reports two instances while driving one — the
  failure this phase exists to prevent, and it is silent
- [X] T095 [P] [US3] Make per-instance reporting distinguishable: `NodeLost`
  and the report's per-node rows must name the instance, not the host, or two
  rows read identically. Note the counters are already keyed by index rather
  than by hostname — T092 fixed that — so this is the presentation half only
- [X] T096 [US3] Read the hardware file (`contracts/hardware.md`):
  `./cluster.yml` by default, `--hardware <file>` to name another, built-in
  defaults when neither exists. Refuse an unknown `version` (FR-006's
  reasoning) and an unknown field, so a typo is not silently a default
- [X] T097 [US3] Announce an implicitly picked-up `./cluster.yml` on the error
  stream, and record the file's path and digest in the report under FR-063.
  Together these are what keep an implicit file from changing a run's meaning
  invisibly; they are required rather than courtesies
- [X] T098 [US3] Command line overrides the file **per field**, with
  `--instance` replacing the whole instance list rather than adding to it. The
  case that forces this is a rate sweep, which varies the rate while holding
  the deployment fixed
- [X] T099 [US1] Make `--rate` the only pacing knob: accept `inf` (`.inf` in
  the file), derive the mode from it, and **delete `--pacing`** along with the
  refusal that policed `--pacing none --rate 10`, which becomes unsayable
- [X] T099a [US1] **`inf` must switch the schedule off, not divide by
  infinity.** Left to the arithmetic, `due = t0 + virtual/inf` gives `due ==
  t0`, so every request is recorded as late by however long the run has been
  going, lateness grows without bound, and every work-conserving run reports
  *itself* invalid. Test that a work-conserving run has an empty lateness
  histogram and is judged on the plan queue instead
- [X] T099b [P] [US1] Document that a large finite rate is **not** `inf` —
  `--rate 1e12` keeps a schedule the machine cannot meet and is correctly
  invalid for lateness. Somebody will type a large number meaning "flat out",
  and the report's existing wording is honest but not obvious
- [X] T100 [P] [US3] Tests: `hardware.example.yml` parses (the example cannot
  rot); two instances on one host are both driven; a duplicate `(host, port)`
  is refused; `--instance` replaces the file's list and `--rate` overrides its
  rate; and nothing belonging to the description is accepted in the hardware
  file
- [X] T101 [US3] Re-verify on hardware. The scratch-server recipe from T092 is
  the cheap version: a second `certus-server-yaml` on its own `--shm-path` and
  its own 16 GiB device file leaves an existing server untouched. Two scratch
  servers on one host is the case this phase is about
- [X] T102 [P] Update `scripts/rate-sweep.sh` for `--rate inf` and the hardware
  file, and re-point its `--node` usage at `--instance`

**Two findings from the implementation, both recorded in the contracts.**

First, **T094 was one refusal short.** FR-081 names only a duplicate `(host,
port)`, and a duplicate `(host, mailbox)` is the same failure with a quieter
symptom: nothing collides, both agents start, and the run measures one cache
under two names — so the simulation places sessions across what it believes are
two independent caches and a migration between them is served as a hit where
FR-048 intends a miss. Both are now refused, naming the pair.

Second, **the trace path has no home for placement at all.** Only the live
driver has a node count to give the simulation, so an `emit` run passes one,
where migration is inert — so a description with a `migration_interval` emits a
trace in which migration never happened. The trace is rightly free of
hostnames, but it is also missing the event. The agreed design (a per-session
placement epoch on each record, targets resolved at playback, and placement
derived rather than drawn from a shared stream) is written up as **D10 in
`research.md`** and is **deferred, not scheduled** — it is a new capability
whose real value is making a **captured** trace driveable, not replaying our
own output.

**T101 verified on hardware, 2026-09-18.** Two co-resident instances on
node2, one per NUMA domain with four NUMA-local drives each (the
`deploy/k8s/certus-server-numa.yaml.tpl` selection rule), plus a separate
cross-machine run against node2 and node5. Both valid. Per-instance rows are
distinguishable (`127.0.0.1:7420` / `:7421`, each with its own mailbox and its
own counters), which is the deployment that was unrepresentable before FR-081.

The migration check is a controlled comparison rather than an inspection: the
same description and seed against **one** instance gives 14150 resident of
15758 (89.8%), against **two** gives 11847 of 15722 (75.4%) — misses rise from
1608 to 3875 on freshly formatted drives. The migrated prefixes really are cold
on arrival, and nothing moved the blocks.

Two defects fell out of the run, both fixed in `620e696f`: `SshLauncher::kill`
self-killed its own remote shell and made every multi-node run impossible, and
`migrations` never reached either report form. A third thing to know: the miss
comparison needs `--format`, not `--clear-cache` — a clear drops the memory
tier only, and disk-backed entries survive, so a repeat run reads 100%
resident.

**Terminology, settled**: "node" is kept for the concept because Zyre already
uses it for an instance regardless of machine, and the model's placement is
written in those terms — `session.node()`, FR-048, FR-049 and FR-064 all stay.
The *flag* is `--instance` because on a command line the ambiguity with
"machine" is expensive, and it is the mistake that produced this phase. FR-081
says this once, explicitly, so a reader does not have to infer it.

**Deferred deliberately, and named in `hardware.md`**: per-instance CPU/NUMA
affinity, which needs an affinity option on `workload-node-agent` first, and
per-instance GPU device. The affinity one is a measurement hazard rather than
tidiness — an agent serving a NUMA-local mailbox can land its threads and
device buffer in the wrong domain, and cross-socket DMA has measured 16%
run-to-run variance on this hardware.

**One pre-existing mismatch recorded rather than fixed**: `cli.md` lists
`--pipeline-depth <n>`, which the binary does not accept. The depth exists and
defaults to 8; only the flag is missing. `cli.md` now says so.

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
Task: "T052 per-turn record in crates/workload-trace/src/record.rs"
Task: "T053 Mooncake writer in crates/workload-trace/src/mooncake.rs"
Task: "T060 Qwen-Bailian writer in crates/workload-trace/src/qwen.rs"
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
