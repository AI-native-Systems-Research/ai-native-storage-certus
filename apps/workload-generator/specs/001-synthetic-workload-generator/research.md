# Phase 0 Research: Synthetic Workload Generator

**Feature**: `001-synthetic-workload-generator` | **Date**: 2026-09-15

Every decision below was checked against the repository rather than assumed.
Where a claim rests on a measurement, the measurement is named.

## D1. Does the emitted trace have to feed `apps/eviction-replay-benchmark`?

**Decision**: Yes, through a **documented projection an emit run writes**
(`--qwen-bailian`), not by teaching the simulator anything new.

**Rationale**: The simulator does not read the trace-IO schema. Its loader
(`apps/eviction-replay-benchmark/src/replay.rs:88`) reads a much simpler JSONL
record — `{chat_id, parent_chat_id, hash_ids: [u64], type}` — and derives a
session id by walking `parent_chat_id` to the conversation root
(`replay.rs:117-129`). It drops records with empty `hash_ids` and treats every
listed key as one access (`Op { method, keys, session_id }`, `replay.rs:58`).

Two facts make the conversion a pure projection needing no information our
trace lacks:

- Its `hash_ids` are already `CacheKey` u64 values, **not** dense mint-order
  integers. So carrying chained keys rather than dense ones (spec FR-057) is
  compatible with this consumer — a point worth recording, because it would
  have been the obvious objection to that decision.
- Our per-turn records carry the full ordered prefix and the session lineage,
  so `chat_id` = turn identity, `parent_chat_id` = previous turn (or −1 at a
  session root), `hash_ids` = the turn's full input block list **followed by
  the blocks it generated**.

  **Amended.** This originally mapped `hash_ids` to the input list alone, on
  the argument that a turn's output reappears in the next turn's prefix, so no
  key is lost. Two things defeat that argument. A real deployment stores a
  generated block **when it is generated** — vLLM's offloading connector calls
  `prepare_store` after each forward pass (`knowledge/kv_IO_pattern.md`) — so
  input-only put every store one turn later than it happens. And the **final
  turn of every session** generates output nothing reads again, so input-only
  omitted it entirely while a real cache still held it. Both understate
  capacity pressure, which is the quantity this consumer exists to measure.
  The same correction applies to the libCacheSim projection. Mooncake keeps
  the prompt alone, because `len(hash_ids) == ceil(input_length / block_size)`
  is a conformance invariant there.

**Alternatives rejected**:

- *Name the flag for the consumer (`--simulator`).* Says who reads the bytes
  here rather than what they are, and invites the reader to think the shape is
  ours. It is Alibaba's Bailian usage-trace format, which several published
  captures are in.
- *Teach the simulator to read a Certus-private schema.* Modifies another app,
  so this feature's correctness would depend on a change outside it, and it
  would mean defining such a schema at all. Rejected as scope creep.

**Consequence for the plan**: one projection writer, and a quickstart step that
runs a generated file through the simulator to prove the projection is
faithful.

## D2. Generator ↔ daemon transport

**Decision**: Plain TCP with length-prefixed framing, `TCP_NODELAY`, and a
configurable number of in-flight batches per node.

**Rationale**: The traffic is tiny and the problem is purely round-trip latency
hiding. Only keys cross the wire (spec FR-047), so a 64-key batch is ~512 bytes
plus a small header. The arithmetic settles it: sustained batch rate equals
pipelining depth divided by round-trip time, so at 1M keys/second with 64-key
batches — well beyond anything measured on this hardware — the requirement is
15,625 batches/second, which at a 50 µs round trip needs **0.78 batches in
flight**. A depth of 8 leaves an order of magnitude of headroom. Bandwidth is
never the constraint: 1M keys/second is 8 MB/s.

For calibration, the recorded remote penalty on this hardware is ~219 µs *per
batch* rather than per key, and that is Certus's own remote path, not this
transport — so the transport is not close to being the limit.

**Alternatives rejected**:

- *RDMA.* Couples the tool to mlx5 hardware for a control path carrying 8 bytes
  per key. The measured need is under one batch in flight; RDMA solves a
  bandwidth and CPU-overhead problem this transport does not have.
- *The in-repo `zyre` component.* It provides zero-configuration peer discovery
  and group messaging (`components/zyre/src/lib.rs:1-6`), but node targets are
  supplied explicitly on the command line, so discovery is unwanted. It is also
  a component-framework component wrapping a C library, which Principle II
  directs this app away from depending on.
- *One connection per lane with no pipelining.* Works, but ties transport
  concurrency to lane count, which FR-072 requires stay independent of the
  workload.

**Precedent**: `std::net` TCP is already used in-repo
(`components/remote-lookup/tests/mesh.rs`,
`apps/certus-server-yaml/src/metrics.rs`).

## D3. Crate layout, and how FR-072 is enforced structurally

**Decision**: Five crates. The three libraries are CUDA-free and are workspace
**default members**; the two binaries link CUDA and are **not** default
members, matching `apps/remote-lookup-bench`.

| Crate | Kind | Default member | Contents |
| --- | --- | --- | --- |
| `workload-model` | lib | yes | YAML schema, distributions, populations, selection, key chaining, session/turn simulation, the plan |
| `workload-trace` | lib | yes | the per-turn record and the three projection writers |
| `workload-wire` | lib | yes | generator↔daemon framing and both halves of the TCP transport |
| `workload-gen` | bin | no | the CLI; live path behind a default-on `live` feature |
| `workload-node-agent` | bin | no | the per-node daemon |

**Rationale**: FR-072 requires the plan and trace to be identical across live
and emit runs. A comment cannot enforce that; a crate boundary can.
`workload-model` owns the simulation and knows nothing about shmq, CUDA,
sockets, or output formats, so both execution paths are *obliged* to consume
the same plan rather than merely intended to. Two parallel paths would have to
duplicate a library to diverge, which is visible in review.

The CUDA split also serves User Story 2, whose independent test requires no
accelerator. Keeping the model, trace, and wire crates CUDA-free means the
whole determinism and distribution test surface runs under a plain `cargo test
--all` on any machine, with no GPU and no server.

**Alternative rejected**: *one crate.* It would put the simulation core behind
CUDA linkage, making US2 untestable without CUDA present and leaving FR-072 as
an honour-system property.

## D4. Does System need a trace container of its own?

**Decision**: No. Every output is a **projection** onto a published format that
a real consumer already reads (`--mooncake`, `--libcachesim`,
`--qwen-bailian`), and System defines no interchange format of its own.

**Rationale**: A workload is repeated from its description and seed (FR-072), so
a stored copy is never the way to repeat one — which removes the only reason a
private container would have to exist. What remains is interoperability, and a
projection serves that directly and at a fraction of the size: the shipped
example's 30-second span is 12.1 MB as Mooncake against tens of gigabytes as a
full-fidelity container at a legal span.

A private format would also have to be earned: specified, versioned, and given
importers, with nothing outside this repository able to read it in the
meantime. That is a real cost against a benefit that only materialises if
someone adopts it, and adoption is not this feature's to decide.

**Alternatives rejected**:

- *A columnar container (parquet) for the block-list columns.* The compression
  argument is sound in isolation — those columns repeat the whole prefix per
  turn — but it is an argument about how to store a format, and the decision
  above is that there is no such format to store. It also pulls the arrow-rs
  family into a workspace that has no other user for it.
- *A JSONL container of the full record schema.* Cheaper, and still a format
  with no reader: the three projections between them carry everything any known
  consumer reads.

**Consequence for the plan**: `workload-trace` writes projections only, and the
per-turn record (`contracts/trace-io.md`) exists as the single in-memory shape
all three are written from (FR-056) rather than as anything that reaches disk.

## D5. Reproducibility: RNG and key chaining

**Decision**: `rand` 0.8 with **`rand_chacha::ChaCha20Rng`** for all sampling,
and a **specified splitmix64-based chain** for keys, with no hashing
dependency.

**Rationale**: FR-012, FR-034, and FR-072 require bit-reproducibility, and two
of the obvious choices silently break it:

- `rand::rngs::StdRng` is explicitly **not** reproducible across `rand`
  releases, and `SmallRng` is not either. `ChaCha20Rng` is documented as
  stable, so it is the only correct choice for a seed that must reproduce a
  plan.
- `std::collections::hash_map::DefaultHasher` (SipHash) is not stable across
  Rust versions, and `ahash` is not stable across its own versions. Either
  would make keys differ between toolchains, so a trace would be verifiable
  only by the exact binary that wrote it (FR-029, FR-034). This is *not* a
  cross-node argument: one generator owns the whole simulation, so nodes
  cannot disagree about a key however keys are chosen.

Since Certus treats `CacheKey` as an opaque u64
(`components/interfaces/src/idispatch_map.rs:6`), the generator is free to
choose its own key function. A documented splitmix64 mix over (parent key,
salt) is ~1 ns, allocation-free, has no dependency, and — being written down in
a contract — can be reimplemented identically by any other tool. That last
property matters more than speed: it is what lets a trace's keys be verified
independently.

`rand` 0.8 matches the version already used in-repo
(`apps/iops-benchmark/Cargo.toml:15`).

## D6. Latency histogram

**Decision**: `hdrhistogram`, used only in `workload-gen` (a non-default
member).

**Rationale**: FR-070 requires p50/p90/p99 and max from per-request timing, and
high percentiles are exactly where a naive bucketing loses accuracy. Confining
it to the non-default-member binary means the default workspace build does not
pay for it. A hand-rolled log-bucket histogram was considered and is ~60 lines,
but Principle V's requirement for statistically defensible measurement argues
for the mature implementation, and FR-070's cost constraint is about *where*
timing happens (per request, not per key), which the crate choice does not
affect.

## D7. YAML parsing

**Decision**: `serde_yaml` 0.9, matching
`apps/certus-server-yaml/Cargo.toml:58`, **with a caveat recorded**.

**Rationale**: Repository consistency, and it is the only YAML crate already
present. The caveat: `serde_yaml` 0.9 was archived by its author upstream and
receives no further releases. That is acceptable here — the input schema is
small, fully specified by `contracts/workload-input.example.yml`, and parsing
YAML is not a moving target — but it is a known future migration
(`serde_norway` and `serde_yml` are the live forks) and should not be
discovered later as a surprise. If it is ever migrated, it should be migrated
repo-wide rather than only here.

## D8. Standard trace formats: which to speak, and which not to emit

**Added during Phase 1**, when it was asked whether an emit run should write a
standard format instead of one of our own.

**Decision**: Write **only** published formats — the **Mooncake writer**, which
is the standard block-level format, the **libCacheSim CSV writer**, which
serves this feature's stated purpose of evaluating eviction, and the
**Qwen-Bailian writer** D1 settled. Do **not** add an OpenTelemetry writer, a
WekaTrace writer, or an arrival-only export, and defer reading third-party
traces. All specifics are in `contracts/trace-interop.md`.

**Rationale**:

First, "adopt the standard" is not an available move, because there is **no
standards-body format at this level at all**.
OpenTelemetry GenAI semantic conventions are the only governed standard nearby
and they describe requests and text, not blocks. The choice is only which
de-facto formats to speak.

Second, two properties decide any candidate, and both were learned by measuring
a format that fails one:

- **A run-global clock**, because cross-session interleaving *is* the workload
  for a cache — it is what makes two sessions contend for one shared object and
  what sets the concurrent footprint.
- **A run-global key scope**, because shared-object pools exist to produce
  reuse *between* sessions.

Mooncake satisfies both, verified on 2 328 upstream rows: per-request lines on
a millisecond global clock, and `hash_ids` globally dense over the whole file
(0..44 683, 44 684 distinct) with identifier 0 opening every row as a shared
system prefix. WekaTrace fails both: `first t = 0.0` for every session with no
field for session start, and `hash_id_scope: "local"`.

**Alternatives rejected**:

- *Write WekaTrace.* Considered seriously and rejected on the measurements
  above: it would discard the interleaving, and being grouped by
  session it also cannot be streamed — a session's line is unwritable until the
  session ends, so emitting it means holding every in-flight session in memory
  (hundreds of megabytes at this feature's concurrency) to produce a file we
  are streaming anyway. Its first line is 2.76 MB for 135 requests.
- *Treat a Mooncake file as the artifact a run is reproduced from.* It is lossy
  — its ceil convention discards `partial_final_valid` — so a round trip could
  not serve as a determinism check. That is FR-075b: a projection is not a
  trace, and reproduction is from the description and seed.
- *Emit OpenTelemetry.* The format itself is capable (absolute ISO 8601
  timestamps carry interleaving fine), but it holds no block identity, so the
  cache structure would have to live in fabricated text whose every block
  tokenises to exactly `block_size` tokens — pinning one tokenizer and chat
  template into the artifact, at roughly 50× the size, to arrive back at the
  keys we started from. Worth it only to drive a real inference engine.
- *Wait for a standard to emerge.* Rejected: a projection target is cheap
  precisely because it is a function of the plan. Adding one later costs a
  writer, not a redesign.

**One rule was over-applied and is recorded so it is not reimposed.** This
decision first said every target must be a conversion of a stored Certus trace,
which took D1's concern — keep consumer-specific shapes out of what a turn
*means* — and turned it into a requirement to write an intermediate. The cost
was real: a legal span on the shipped example is roughly 27 GB as a
full-fidelity container, so obtaining a Mooncake file of a few hundred
megabytes meant paying that first, plus a possible free-space refusal. What
actually needed protecting is what FR-075b now says: a projection is a function
of the **plan**, never a second simulation, and it is not a trace. Writing it
directly satisfies both and avoids a parse step that could itself be wrong.

So the projections live in `workload-trace` as writers fed by one record stream
(FR-056), with no stored intermediate anywhere in the path.

**Consequence for the plan**: `emit` grows one flag per projection; three
writers become tasks; and the `oracleGeneral` binary layout needs reading from
upstream source before any bytes are written, because upstream documents those
fields in prose only.

**Unexpected bonus, recorded because it is the strongest reason the libCacheSim
target is worth carrying**: libCacheSim's preferred `oracleGeneral` format
wants next-access-time, which a real trace can only obtain by a full offline
pass. An emit run knows the entire future of its own trace, so it is one
backward pass — making optimal-policy (Belady) baselines available for eviction
comparisons.

## D9. Open questions carried into Phase 1

None. Every Technical Context field is resolved; no `NEEDS CLARIFICATION`
markers remain.

D8 was added *during* Phase 1 rather than before it, because the question was
not asked until the writers were about to be built. Its facts are all
measurements rather than recollections, and `contracts/trace-interop.md` names
the exact upstream path, row count and figure behind each one so any of them
can be re-derived.

One honesty note on Principle IX's traceability rule: those measurements are
traceable to **upstream** bytes at named paths, not to a measurement stored in
this repository. That is weaker than the rule asks for. It is acceptable here
because the claims are structural facts about third-party file formats rather
than statistical claims about our own machinery, and because the contract
records the procedure rather than only the conclusion — but if any of these
formats becomes load-bearing for a *result*, the measurement should be landed
as a script the way `research/population/` was.

## D10. Replaying a trace against a cluster: where placement lives

**Deferred, not scheduled.** Recorded here because the design was settled while
FR-081 was being built, and the conclusion is not obvious.

### The gap

The trace record carries no node or instance field, so a trace is already
independent of the hardware — which is right, and FR-005's rule in the other
direction. But only the live driver has a node count to give the simulation, so
an `emit` run passes one, where migration is inert (FR-049) and takes no draws.

So the loss is sharper than "a trace does not say which instance served a
turn". **A description with a `migration_interval` emits a trace in which
migration never happened at all** — not only the target is missing but the
event, and the event is what makes a session's prefix cold.

### The decomposition

Three layers. The middle one has no home today:

- **Workload**: whether and how often a session migrates. Already in the
  description.
- **Trace**: *that* session S migrated before turn k. Missing.
- **Playback**: *which* instance it moves to. Resolved at replay time from the
  instance list, so the file stays hardware-free.

### The mechanism: a placement epoch, not a target

Record a per-session `placement_epoch` on each turn, incremented on each
migration. Preferable to a `moved` flag on three counts:

- **Stateless**: a replayer reading record k knows where to send it without
  having read k-1, which a parallel or resumable replay needs.
- **Replayable at any instance count**, one included, where it is inert.
- **Self-checking**: the summed per-session maximum equals the `migrations`
  counter the report already prints, so a projection that dropped migrations is
  caught rather than assumed.

Playback routes by `instance = f(session, epoch, n)`: epoch 0 uniform over `n`,
later epochs uniform among the others, which is FR-048's rule evaluated late.

### What has to change first, and why it is worth doing anyway

For that `f` to exist, **placement must become a derivation rather than a draw
from a shared stream.** `place` and `migrate_due` currently draw from the `sim`
substream interleaved with turn decisions, so a session's node cannot be
recomputed without replaying the whole simulation. Deriving it as keys already
are — `splitmix64` over a salt of `(session, epoch)` — buys three things:

1. Placement becomes recomputable at playback from the trace alone.
2. It would **guarantee** `the_workload_is_the_same_whatever_the_node_count`,
   which today passes as a property of how the stream happens to be laid out
   rather than by construction. A later edit could break it, and the test would
   then be reporting luck.
3. It settles an asymmetry: `place` draws even at one node "so the sequence
   does not depend on the deployment", while `migrate_due` returns early
   without drawing "so a single-node run does not consume randomness a
   multi-node run would spend elsewhere". Two opposite conventions, each
   justified by similar-sounding reasoning. A derivation makes it moot.

### The larger prize

There is **no replay path at all** today: the live driver runs from the
simulation, never from a trace. So this is a new capability rather than a fix,
and its value is not replaying our own `emit` output — it is that a
**captured** trace becomes driveable against Certus. A captured trace has no
instance assignment either, so one derivation serves both.

### Two traps to carry into the work

- **A replay report MUST record the instance count and the derivation salt**,
  or two replays are incomparable. Same argument as the hardware file's digest.
- **The loss is at the emit boundary, not the projection boundary.** This was
  first written here as "placement is a declared loss for the projections", and
  that was **wrong**: a projection's `declared_losses()` names what the
  *record* carries and the target cannot, and the record never carried
  placement, so declaring it there puts the loss one stage later than it
  happens. What is missing from an emitted trace is the migration **event**,
  and the emit path now declares that — FR-077 applied where the loss occurs.
  If the placement epoch above is ever built the projections *will* then drop
  it and a `declared_losses()` entry becomes correct, but not before.

## D11. Dense block identifiers instead of chained hashes: DEFERRED

**Decision**: keep chained `u64` keys for now. The design below is viable and
is deferred on **priority**, not because it was refuted — the user's words:
"I don't think redesigning the key space is our highest priority, so I'm fine
with deferring it. But I wanted to have the discussion and record the
outcomes."

**What was proposed.** Dense identifiers allocated as **ranges** rather than
tracked per block: one contiguous range per shared object, one per live
session chain, released when the session ends. The allocator is a bump
pointer — take the current value, add the range length. Cross-run
disjointness comes from a per-run **offset**.

### Four objections raised against it, and what became of them

1. *"It needs a global trie, which is what chaining exists to avoid"*
   (`trace-io.md` § Keys are chained u64). **Answered by the ranges**: the
   state is O(shared objects + live sessions), not O(blocks).
2. *"A global allocator reintroduces an ordering dependency, so FR-072
   breaks."* **Wrong, and withdrawn.** Allocation happens on the generation
   side, which is single-threaded — there is no `thread`, `rayon`, `Mutex` or
   atomic anywhere in `workload-model` — and `--lanes` / `--batch-keys` act
   only on the *consumption* of a finished plan. The allocator is a plain
   `&mut self` field, not even an atomic.
3. *"A growing session chain cannot stay contiguous."* True only if the space
   must have no gaps. A range per session, over-reserved and abandoned at
   session end, still yields small integers, and the range representation
   stays compact either way.
4. *"Identifiers would alias across runs, so a warm server would serve false
   hits."* Real — and **answered by the per-run offset**.

### What dense identifiers would buy

- **The birthday collision disappears.** It is a documented,
  deliberately-unhandled risk today (`crates/workload-model/src/keys.rs:216`
  quotes 10^-6 per run) with **no detection**: a collision silently merges two
  blocks into one false cache hit. That quoted rate corresponds to about
  6x10^6 distinct blocks and scales as n^2/2^65 — roughly 3x10^-4 at 10^8
  blocks and **2.7% at 10^9**.
- **Identifiers fit `i64`**, which is what the Qwen-Bailian `chat_id` field
  requires and what any index-based reader wants. The Mooncake projection's
  dense renumbering (FR-078) would become unnecessary rather than
  load-bearing.
- **FR-072's semantics and its test would coincide.** Keys are arbitrary
  labels, so the honest requirement is that two plans be **isomorphic** —
  equal up to renaming — while the requirement as written asks for
  byte-identity, which is strictly stronger. Under a deterministic allocator
  the allocation order *is* a canonical labelling, so the two collapse into
  one property. The relabeling freedom that makes byte-identity too strong
  exists only because keys are hashes.

### What the present scheme buys, and what a replacement must preserve

- **A key is a pure function of coordinates, not of allocation history.**
  `shared_salt` packs `class_id` (12 bits), `instance_index` (26) and
  `block_ordinal` (24) and hashes the result, so the same logical block has
  the same key in every run of a description, and a different description
  yields different keys. That is what makes a **warm server** safe: a hit
  against a previous run's cache is a true hit, not an alias. Note the
  coordinates are *already* an ordinal range per shared object — the present
  scheme hashes a range rather than using it directly.
- **The offset must come from somewhere that outlives a run**: a persisted
  counter, an operator-supplied flag, or a digest-derived base with a generous
  per-run reservation. The last trades a block-level collision probability for
  a much smaller run-level one, and unlike the birthday case it is checkable.

**Open if this is picked up**: whether FR-072 should be reworded to require
isomorphism, with byte-identity named as the sufficient check it is tested by.
Recommended, and deliberately not done here — it is a spec change, and the
current wording is sound as a test even where it overstates the semantics.
