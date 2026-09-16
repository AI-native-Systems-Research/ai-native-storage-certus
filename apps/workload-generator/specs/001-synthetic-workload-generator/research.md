# Phase 0 Research: Synthetic Workload Generator

**Feature**: `001-synthetic-workload-generator` | **Date**: 2026-09-15

Every decision below was checked against the repository rather than assumed.
Where a claim rests on a measurement, the measurement is named.

## D1. Does the emitted trace have to feed `apps/eviction-replay-benchmark`?

**Decision**: Yes, but through a **documented converter subcommand**, not by
adding a third output shape to the published trace format.

**Rationale**: The simulator does not read the trace-IO schema. Its loader
(`apps/eviction-replay-benchmark/src/replay.rs:88`) reads a much simpler JSONL
record — `{chat_id, parent_chat_id, hash_ids: [u64], type}` — and derives a
session id by walking `parent_chat_id` to the conversation root
(`replay.rs:117-129`). It drops records with empty `hash_ids` and treats every
listed key as one access (`Op { method, keys, session_id }`, `replay.rs:58`).

Two facts make the conversion a pure projection needing no information our
trace lacks:

- Its `hash_ids` are already `CacheKey` u64 values, **not** dense mint-order
  integers. So dropping the trace-IO density requirement (spec FR-057) is
  compatible with this consumer — a point worth recording, because it would
  have been the obvious objection to that decision.
- Our per-turn records carry the full ordered prefix and the session lineage,
  so `chat_id` = turn identity, `parent_chat_id` = previous turn (or −1 at a
  session root), `hash_ids` = the turn's full input block list.

**Alternatives rejected**:

- *Emit the simulator's shape as a third container.* Contradicts FR-055's "two
  containers holding identical records" and would put a consumer-specific
  format in the published contract.
- *Teach the simulator to read trace-IO.* Modifies another app, so this
  feature's correctness would depend on a change outside it. Rejected as scope
  creep.

**Consequence for the plan**: one converter subcommand, and a quickstart step
that round-trips a generated trace through the simulator to prove the
projection is faithful.

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
| `workload-trace` | lib | yes | trace-IO writer (JSONL always; parquet behind a feature) and the D1 converter |
| `workload-wire` | lib | yes | generator↔daemon framing and both halves of the TCP transport |
| `workload-gen` | bin | no | the CLI; live path behind a default-on `live` feature |
| `workload-node-agent` | bin | no | the per-node daemon |

**Rationale**: FR-072 requires the plan and trace to be identical across live
and emit runs. A comment cannot enforce that; a crate boundary can.
`workload-model` owns the simulation and knows nothing about shmq, CUDA,
sockets, or output containers, so both execution paths are *obliged* to consume
the same plan rather than merely intended to. Two parallel paths would have to
duplicate a library to diverge, which is visible in review.

The CUDA split also serves User Story 2, whose independent test requires no
accelerator. Keeping the model, trace, and wire crates CUDA-free means the
whole determinism and distribution test surface runs under a plain `cargo test
--all` on any machine, with no GPU and no server.

**Alternative rejected**: *one crate.* It would put the simulation core behind
CUDA linkage, making US2 untestable without CUDA present and leaving FR-072 as
an honour-system property.

## D4. Parquet

**Decision**: Use the `parquet` crate, gated behind a **non-default** cargo
feature on `workload-trace`, which `workload-gen` enables.

**Rationale**: There is **no parquet dependency anywhere in this repository
today** — not in Rust, not in Python — so this is a genuinely new dependency
and Principle VII requires it be justified. It is: FR-055 makes parquet a
required output container, and the trace's block-list columns repeat the whole
prefix per turn (spec's own note), so it is exactly the case where columnar
compression matters. Hand-rolling a parquet writer is not a serious option.

Gating it matters because `parquet` pulls the arrow-rs family. Without a
feature gate, every `cargo build` in the workspace would pull arrow, since
`workload-trace` is a default member. With the gate, the default build is
unaffected and only the non-default-member binary pays.

**Alternative rejected**: *JSONL from Rust plus a Python parquet converter.*
The repository does have Python tooling, but SC-004 requires the two containers
to yield identical records, and a cross-language pipeline makes that assertion
span two runtimes instead of one test.

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

**Added during Phase 1**, after the emitted trace's schema was questioned:
whether it is a standard, and whether we should emit a standard one instead.

**Decision**: Keep the adopted schema as the **only** thing an emit run writes,
and reach every other format through `convert`, as D1 already established for
the simulator. Add exactly **two**: a **Mooncake writer**, which is the
standard block-level format, and a **libCacheSim CSV writer**, which serves
this feature's stated purpose of evaluating eviction. Do **not** add an
OpenTelemetry writer, a WekaTrace writer, or an arrival-only export, and defer
reading third-party corpora. All specifics are in `contracts/trace-interop.md`.

**Rationale**:

First, what the schema is. It is **not a standard and not adopted from one** —
it is the normalisation layer another team built over the public formats, and
the evidence is in the corpus's own manifests: each of the 24 traces has a
different `source_url` (Azure `AzurePublicDataset`, BurstGPT, Mooncake FAST'25,
qwen-bailian, ragbench, SWE-agent, WildChat), and `field_status: native |
reconstructed | unavailable` exists *only* because those sources disagree about
what they carry. A standard would not need that field; a normaliser cannot work
without it. Being a superset of seven upstream formats is exactly the property
that makes it the right thing to emit and the right thing to convert *from*.

Second, there is **no standards-body format at this level at all**.
OpenTelemetry GenAI semantic conventions are the only governed standard nearby
and they describe requests and text, not blocks. So "adopt the standard" is not
an available move; the choice is only which de-facto formats to speak.

Third, two properties decide any candidate, and both were learned by measuring
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

- *Replace our schema with WekaTrace.* Considered seriously and rejected on the
  measurements above: it would discard the interleaving, and being grouped by
  session it also cannot be streamed — a session's line is unwritable until the
  session ends, so emitting it means holding every in-flight session in memory
  (hundreds of megabytes at this feature's concurrency) to produce a file we
  are streaming anyway. Its first line is 2.76 MB for 135 requests.
- *Emit Mooncake as a third container.* Same objection D1 made about the
  simulator's shape: it contradicts FR-055's "two containers holding identical
  records" and would put a consumer-specific format in the published contract.
  Also lossy — its ceil convention discards `partial_final_valid`, so a round
  trip could not serve as a determinism check.
- *Emit OpenTelemetry.* The format itself is capable (absolute ISO 8601
  timestamps carry interleaving fine), but it holds no block identity, so the
  cache structure would have to live in fabricated text whose every block
  tokenises to exactly `block_size` tokens — pinning one tokenizer and chat
  template into the artifact, at roughly 50× the size, to arrive back at the
  keys we started from. Worth it only to drive a real inference engine.
- *Wait for a standard to emerge.* Rejected: `convert` targets are cheap
  precisely because they are projections of a superset. Adding one later costs
  a writer, not a redesign.

**Correction applied after this decision was first written.** It initially said
every target must be a *conversion of the emitted trace*, which over-applied
D1. D1's concern was keeping consumer-specific shapes out of the published
trace contract — not forbidding a direct writer — and the stronger rule imposed
a real cost: a legal span on the shipped example is roughly 27 GB as a native
trace, so requiring it as an intermediate meant paying that, plus a possible
free-space refusal, to obtain a Mooncake file of a few hundred megabytes. What
actually needed protecting is narrower and is now what FR-075b says: a
projection is a function of the **plan**, never a second simulation, and a
projection is not a trace. Direct emission satisfies both, and avoids a JSONL
parse step that could itself be wrong.

So the projections live in `workload-trace` as functions, with **two entry
points calling the same function**: `emit` applies them in-stream, and
`convert` applies them to a stored trace. Keeping `convert` is not redundancy —
its input is the *schema*, and the 24 real traces in `traces/` are in that
schema, so it is the only way to push a real workload and a generated one
through an identical transformation. That is the comparability goal, and it
cannot be reached from the emit path at all.

**Consequence for the plan**: `emit` grows one flag per projection, and
`convert` grows a `--to` selector; two writers become tasks; and the
`oracleGeneral` binary layout needs reading from upstream source before any
bytes are written, because upstream documents those fields in prose only.

**Unexpected bonus, recorded because it is a capability the native format does
not give anyone**: libCacheSim's preferred `oracleGeneral` format wants
next-access-time, which a real trace can only obtain by a full offline pass. An
emit run knows the entire future of its own trace, so it is one backward pass —
making optimal-policy (Belady) baselines available for eviction comparisons.

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
