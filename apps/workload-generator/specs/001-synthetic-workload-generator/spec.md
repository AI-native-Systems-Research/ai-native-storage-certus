# Feature Specification: Synthetic KV Workload Generator

**Feature Branch**: `001-synthetic-workload-generator`
**Created**: 2026-08-04
**Status**: Draft
**Input**: A workload generator for measuring Certus cache **performance** and **hit rate**
under inputs with controlled statistical properties — including cross-node
(remote-lookup) traffic — and for comparing **cache replacement algorithms**. The input is
YAML describing *statistical properties* of the workload (timing of puts, entry lifetime,
timing of gets, expected gets per key, prefix-sharing structure, node placement), not an
enumerated access trace. The generator draws a random but reproducible stream of cache
operations over time and across nodes from that model.

The design constraint that shapes everything below: **if the complexity of the input
approaches the complexity of a KV access trace, the generator has no value.** A useful
input is O(100 lines) of fitted parameters that expands into O(10^8) operations.

## Scope and boundary

This app is a **lab measurement tool**, in the same category as
`apps/remote-lookup-bench` and `apps/iops-benchmark`: it is not part of the Certus data
path and is not a `cargo test` target in its hardware mode.

It is split into two crates so that the model, the plan format, and the plan statistics are
testable in CI without SPDK, CUDA, or RDMA:

| Crate | Binary | Deps | Workspace membership |
| --- | --- | --- | --- |
| `apps/workload-generator` | `certus-workload` (`plan`, `report`, `fit`) | `serde_yaml` and a hashing crate. **No dependency on `interfaces`, on `IEvictionPolicy`, or on any policy component** — with cache simulation deferred there is nothing here that needs them, and nothing in the generator that knows what a cache is (FR-018a, FR-034) | **`default-members`** — built and tested by `cargo test --all` |
| `apps/workload-runner` | `certus-workload-run` (`run`) | above + `tonic`, local CUDA externs | `members` only — needs hardware |

`apps/workload-runner` follows `apps/remote-lookup-bench`'s precedent of declaring its
handful of CUDA entry points locally rather than depending on `gpu-services`, so that it
never forces `interfaces/spdk` onto a default build.

### Relationship to existing benchmark tooling

- **`apps/remote-lookup-bench`** — the *client architecture* this app reuses: one CUDA
  allocation for the process exported as one IPC handle addressed per entry via
  `IpcHandle.offset`, many keys per RPC, many RPCs in flight (`workers` × `inflight`), no
  host/device copy on the measured path. That app issues a *dense key range*; this one
  issues a *modelled key stream*. Long-term, `remote-lookup-bench`'s `lookup` subcommand
  is expressible as a trivial workload YAML, and consolidating is a follow-up, not part of
  this feature.
- **External LLM trace corpora** — the input to `certus-workload fit` and the reference for
  validating that synthetic output resembles real behaviour. What this feature depends on is the
  **format**, specified in `contracts/trace-io.md`: a self-describing `manifest.json` plus
  block-level invocation records. Trace collections in that format are **not part of this
  repository** — they are large, they come from many sources under differing licences, and which
  ones are on hand will change. Any particular collection is therefore a sample, not a dependency:
  a path is supplied at the command line, `fit` reads the manifest to learn what that trace can
  support, and no requirement here may rest on a specific trace existing.
- **`benchmarks/kv-offload-replay`** — a secondary, vLLM-mediated view. Its `traces/sharegpt/`
  files remain useful as an end-to-end cross-check. Note that the **`*.mgr.jsonl`** files are the
  KV key trace (`{ts, method, keys: [<hash>…]}`); the **`*.handler.jsonl`** files are GPU↔CPU
  *transfer* records (`medium`, `block_ids`, `group_sizes`) and are **not** a key trace. An earlier
  draft of this spec named the handler file as `fit` input, which was wrong.
- **`components/eviction-policy-lru`** — **not used by this feature at all.** An earlier draft
  had the generator grade it through `IEvictionPolicy`; with cache simulation deferred (FR-034)
  there is no dependency in either direction. It remains one implementation among the more
  sophisticated ones expected later, and nothing here may be specified in terms of any of them.
  What this feature gives a future policy comparison is the part that makes one valid: identical
  streams, provable by digest (FR-036), and the capacity-free statistics that say what the
  workload was (FR-034a).

## Clarifications

### Session 2026-08-04 (initial design — resolved)

- Q: How can a compact YAML input generate realistic KV **prefix trees** without becoming a
  trace? → A: `CacheKey` is an opaque `u64` that, in the vLLM path, is a rolling hash over
  the block chain. A prefix tree is therefore a **trie whose node identity is the hash of
  the path to it**; two requests sharing a 10-block prefix emit *the same first ten u64s*
  and then diverge. No text, tokens, or content need be modelled. The trie is never
  materialised: `child_id = H(parent_id, child_index)`, so memory is O(active paths), not
  O(trie). *(The sampling half of this answer — a Pitman–Yor branching process over per-depth
  bands — was superseded on 2026-08-05; see the session below.)*
- Q: Should the YAML specify puts (populates) as well as gets? → A: **No.** The plan
  contains **requests** — "request R needs this ordered list of keys, at these sizes" — and
  the executor issues lookups and populates *whatever actually missed*. Puts are therefore
  a consequence of the system under test, not an input. Specifying puts would assume the
  hit rate the experiment exists to measure, and the same YAML would stop meaning the same
  thing under two different replacement policies.
- Q: Where is per-key reuse specified — a top-level `gets_per_key`/`lifetime`, or inside the
  workload model? → A: **Once per kind of reuse, and there are two kinds.** *Intra*-session
  reuse (turn N+1 re-reading turn N's blocks) lives in `workload.sessions`; *inter*-session
  reuse (different sessions walking the same trunk) lives in `corpus.trees`. A top-level
  reuse block, or a popularity or path length restated inside `workload.mix`, would
  over-determine the model and the two specifications would silently disagree.
- Q: Open-loop (absolute arrival schedule) or closed-loop (bounded concurrency, reactive)?
  → A: **Both, selectable, with open-loop the default.** Under closed-loop, arrival times
  depend on how fast the system responds, so two replacement policies see *different key
  streams* and any hit-rate comparison between them is confounded. Open-loop with absolute
  timestamps keeps the key stream identical across arms. Closed-loop is for throughput and
  saturation measurement, where queueing is the phenomenon of interest.
- Q: How are hardware asymmetries (e.g. a NIC on socket 0 and a GPU on socket 1, previously
  measured as cv 16% vs 2%) handled — by steering requester/holder roles onto the
  favourable node? → A: **No role steering. Symmetry is a stipulated precondition,
  verified by preflight.** In practical remote lookup, KV copies between nodes are
  essentially symmetrical, so a generator that assigns asymmetric roles would be modelling
  the lab rather than the deployment. Placement stays uniform and every node both requests
  and holds. Instead, `certus-workload-run preflight` inspects every node and **refuses to
  run a comparative measurement** when the cluster is not symmetric, naming what differs.
  This converts a silent measurement confound into a loud, actionable error.
- Q: Can tiered hit rate be measured against the current gRPC surface? → A: **No, and this
  is a hard prerequisite.** `EntryResult` (`apps/certus-server-yaml/proto/dispatcher.proto`)
  carries only `success` + `error_code`; a successful `Lookup` could have been served by
  DRAM, local SSD, a peer's DRAM, or a peer's SSD, and nothing distinguishes them.
  `GetIoStats` gives only aggregate local-SSD bytes and latency clustering is too fragile to
  attribute per-key. A `served_by` enum must be added to `EntryResult` (see Dependencies).
  **Now specified**: `components/dispatcher/specs/002-served-by-tier-attribution/`, whose
  `contracts/served-by.md` is the normative taxonomy this spec consumes.
- Q: How many attribution values are there? → A: **Seven**, not the five originally assumed
  here: `DRAM | SSD | REMOTE_DRAM | REMOTE_SSD | MISS | SIZE_MISMATCH | ERROR`.
  `SIZE_MISMATCH` is separate from `MISS` because the key *is* present — just at another
  size — and folding the two would make size-mismatched keys remote-eligible, a behaviour
  change smuggled in under an attribution feature. `ERROR` exists because a lookup that was
  attempted and failed is neither a hit nor a miss, and without it the identity
  `hits + misses + errors = requests` cannot hold. The first four are hits; the last three
  are not.
- Q: Is `REMOTE_SSD` a stable property of a holder configuration? → A: **No — it is a
  first-touch property.** Serving from a peer's disk promotes the entry into that peer's
  memory tier, so the next remote fetch of the same key reports `REMOTE_DRAM`. Any test or
  report that expects a fixed `REMOTE_SSD` fraction from a fixed `holder_tier` is
  mis-specified. *(The underlying observation stands and is why US4 scenario 4 forbids reading
  a class share as stable. The `holder_tier` field itself was removed on 2026-08-06 — see the
  scope session below — so the generator no longer has any way to ask for a tier in the first
  place.)* (The fabric transfer itself is *always* out of the peer's DRAM;
  `REMOTE_SSD` means "a peer's disk read was on this request's critical path.")
- Q: Is the plan generated independently on each node, or generated once and distributed?
  → A: **Generated once on the orchestrator, content-hashed, and distributed**; each node
  verifies the hash before running its slice. Bit-identical independent generation would
  require pinning floating-point behaviour across compilers and CPUs, which is a hazard not
  worth taking on for a measurement tool.

### Session 2026-08-05 (corpus and session model — resolved)

- Q: Real traces show **several** prefix trees at the top levels, and each session always
  starts from the same root while a root serves many sessions. Can the committed corpus model
  express that? → A: **No, and it is replaced.** The Pitman–Yor model had exactly one implicit
  root — a forest was only approximated by a low concentration at band 0, so tree count was
  neither statable nor fittable — and it had **no concept of a session at all**, so a
  many-sessions-to-one-root binding was inexpressible in principle. It also descended the trie
  *per request*, whereas sticky binding requires the root be drawn once *per session*: a
  behavioural difference, not a notational one. Replaced by an explicit forest
  (`corpus.trees.roots`) plus sticky per-session root binding.
- Q: Why drop the per-depth sharing table rather than keep it alongside? → A: **Because it has
  degrees of freedom the key model cannot realise.** `CacheKey` is a rolling hash over the
  block chain, so **divergence is irreversible**: once two paths differ at depth *d*, every key
  below *d* differs too, whatever the content. Sharing is therefore necessarily a *monotone
  prefix* property, and a per-depth table can request "tight at 0, loose at 8, tight again at
  16", which no assignment of keys can satisfy. The only genuinely free quantity is **where
  sharing ends**, which is `shared_depth`. Keeping both models would mean one code path whose
  extra expressiveness was unimplementable.
- Q: Do the three archetypes survive? → A: **As presets, not as schema.** `conversation` /
  `one_shot` / `scan` become weighted parameter sets over a single session model
  (`turns: geometric(6)` / `turns: 1` / `turns: 1, private_depth: 4000`). This deletes three
  bespoke fields that each restated a `corpus` quantity — `popularity`, `novel_fraction`, and
  `length_blocks` — and leaves one sampling path and one fitting routine. The names remain
  useful vocabulary in reports and preset filenames.
- Q: What is gained, and what is lost? → A: Gained: **every parameter is one pass over a
  trace** (`roots.count` = distinct depth-0 keys; `roots.popularity` = sessions per root;
  `shared_depth` = longest common prefix against earlier requests; `private_depth` = depth
  minus that), so the model is parameterised in the same space FR-056 already validates it in,
  and `empirical` becomes the natural default rather than an escape hatch. Fitting Pitman–Yor
  would have been a nonlinear fit with no closed form. Lost: Pitman–Yor's principled
  nonparametric power-law backing — `branch_skew` is more ad hoc — and *(this half was WRONG and is
  reversed as of 2026-08-07: depth-varying **branching** is realisable and is what real traces show;
  only depth-varying **sharing** is unrealisable, and the argument below conflated them)* depth-varying branch
  skew, which by the argument above was unrealisable anyway. Parameter count drops from ~12 to
  ~8, and conceptual load considerably further.

### Session 2026-08-06 (minting rule and path length — resolved)

- Q: `branch_skew` concentrates descents "among the children that already exist" — so what
  *creates* a child? → A: **Nothing did; that was a hole.** Pitman–Yor's discount had been the
  minting rule and went out with the rest of PY, leaving trunk width described as "emergent"
  with no mechanism to emerge from. Fixed by an explicit `branch_factor`: a node's child count,
  drawn once from the node's own identity and the seed.
- Q: Why not restore a Chinese-restaurant rule, which is the standard way to mint? → A:
  **Because it makes the corpus depend on arrival order.** The trie would become a function of
  the request sequence, so `corpus` would no longer be orthogonal to `workload` (FR-002) —
  changing the request rate would change which keys exist — and a key's identity would no
  longer be computable from its path alone, defeating `child_id = H(parent_id, child_index)`,
  the O(active paths) bound, and independent per-node plan generation. Note this is a narrower
  objection than the one that killed the `by_depth` table: a CRP is *realisable*, just not
  orthogonal. Determinism in node identity buys all three properties back.
- Q: For a 6-turn session with `private_depth: 8` and `growth_per_turn: 6`, is the path 8 or
  38? → A: **38, and the two fields were never a double statement — one was misnamed.**
  `private_depth` is the turn-1 private path, `growth_per_turn` is the per-turn increment, and
  depth is `shared_depth + private_depth + Σ growth`. Turn N's path must be a strict prefix of
  turn N+1's anyway, because a rolling-hash key rehashes everything below a changed prefix.
- Q: Then which section owns `private_depth`? → A: **`workload.sessions`.** It is a property of
  a session, not of the shared structure, and moving it sharpens design rule 1 to `corpus` =
  what is *shared* / `workload` = how a session *traverses* it. It also resolves a live
  contradiction: `workload.mix` had to override `private_depth` for the `scan` preset while
  validation rule 3 forbade a path length in `mix`, so the committed worked example was invalid
  under its own rules. The honest cost is that distinct-key count now depends on a `workload`
  field — but it always did, since session count already determined how many private branches
  were minted.
- Q: Does anything become dead? → A: Yes — the edge case "`shared_depth` exceeding the
  session's total path" is now **vacuous**: depth is defined *as* `shared_depth` plus a
  non-negative remainder, and `branch_factor ≥ 1` makes the trunk unbounded in depth. The
  clamp and its counter are removed rather than left as unreachable schema.
- Q: Is the drawn `shared_depth` really the same number as the measured prefix-sharing depth,
  as § Fitting claims? → A: **No — it is an upper bound, and the gap is governed by one
  computable quantity.** A session cannot unilaterally share *s* levels; it shares only if some
  earlier session walked the same *s* steps. Two distinct statistics were being conflated:
  *pairwise* LCP is low (with `branch_factor` 1.25 and `branch_skew` 0.9 the per-level agreement
  probability is ~0.85, so median pairwise LCP is ~4 against a drawn median of 18), whereas LCP
  against the *union* of earlier requests equals the drawn `s` whenever trunk paths are occupied.
  The union statistic is the one FR-056 already measures and the one that matters for a cache —
  a hit needs *someone* to have touched the block, not one specific peer. So the fix is
  **trunk occupancy** (FR-009f), reported and validated, rather than a calibration loop, which
  would have reintroduced exactly the nonlinear-fit cost that justified dropping Pitman–Yor.
- Q: Did that check find anything? → A: **Yes, in the schema's own worked example.** At 12 roots
  and ~40 000 sessions per 60 s window there are ~3 300 sessions per root, but
  `branch_factor: 1.25` yields 7 500 trunk paths per root at depth 40 — occupancy 0.4. Since
  that example's `shared_depth` runs to 40, its deepest-sharing quartile silently failed to
  achieve the sharing it asked for. The bound is `branch_factor < (3 300)^(1/40) = 1.22`, so 1.25
  was just over the line. `branch_factor` therefore defaults to `auto`, resolving to ~1.18 by
  closed form (FR-009g).
- Q: Why is `branch_factor` a scalar mean rather than a distribution? *(Superseded 2026-08-07: the
  scalar became a per-depth `branching` profile once measurement showed real tries are flat for
  16-40 depths at a stretch and then jump. The reasoning below still governs each segment's fanout,
  which is a scalar mean for exactly these reasons.)* → A: Because child counts
  are **integers** and width grows as `branch_factor^depth`, so the mean is the only moment that
  matters — and a lognormal `median: 1.15, sigma: 0.4` rounds to an effective mean near 1.27,
  drifting well past the occupancy bound while appearing to be a conservative setting. A scalar
  mean, realised by randomised rounding to `floor(m)`/`ceil(m)`, is exact.
- Q: The plan's event record still carries an `archetype` byte that FR-014 abolished, and no
  session identity. What replaces it? → A: **`mix_index`, plus a stored `session_id` and
  `turn`**, widening the record from 32 to 40 bytes. `mix_index` keeps per-class reporting
  without an `archetype` field in the schema. `session_id` must be *stored*, not derived: turns
  are separated by `think_time`, so a session's requests are not contiguous and no grouping on
  `request_id` recovers them — and acceptance scenario 3's two assertions (every session starts
  from one root; sub-`shared_depth` keys are shared by no other session) are uncheckable without
  it. `turn` is load-bearing now that FR-014a makes depth a function of turn index, and turn 1
  versus turn *N* are qualitatively different cache events, so FR-044a requires the split.
- Q: Is 40 bytes affordable, and why those widths? → A: Yes — 10^7 events goes 320 MB → 400 MB,
  and FR-028 asks only that 10^7 be *routine*. Widths were chosen to keep every field naturally
  aligned with the record a multiple of 8, so an array needs no packed intermediate: `session_id`
  u32 (4.3×10^9 sessions, against ~40 000 per window), `turn` u16 (against a `turns` p99 of 31),
  `mix_index` u8, and 2 reserved bytes. `depth` was kept although it equals the key's ordinal
  within its request, because deriving it would mean scanning back to `REQUEST_START` and would
  defeat indexing by ordinal — and dropping it would not shrink the record, since 36 bytes would
  pad back to 40 anyway.
- Q: What stops this from breaking old readers? → A: `plan_format` in the manifest (FR-023b).
  The record has no length prefix, so the version field is a decoder's only signal of width. The
  dispatcher's own wire codec is the cautionary precedent — it frames by record count with no
  length prefix, so appending a field there mis-aligns an old decoder *silently*.

### Session 2026-08-06 (clarify pass — resolved)

- Q: US2's scenario 2 and SC-010 both require **two policies** to exist, but Out of Scope
  excludes implementing any, and `eviction-policy-lru` is the only one there is — so a P1
  scenario and a success criterion are undemonstrable on delivery. Where does the second arm
  come from? → A: **Nowhere; the criteria were wrong.** The generator is a standalone tool that
  produces workloads of various shapes and must not be written in terms of how any replacement
  policy is implemented — the current LRU is one implementation and is expected to grow more
  sophisticated. So the generator's own criterion is that **arms consumed an identical stream,
  verified by digest**, not that two named policies were compared; comparison is the consumer's
  job. Additionally the harness reports **Belady/OPT as a workload statistic** *(since retracted —
  see the 2026-08-07 session: OPT needs a capacity, so the compulsory-miss floor and the
  reuse-distance CDF carry this role instead)*, which is
  legitimate here precisely because optimal hit rate is a property of the *workload* rather than
  of any policy: it is computed from future references in the plan, with no `IEvictionPolicy`
  implementation involved. It completes a pair with the compulsory-miss floor FR-060 already
  computes — a floor without a ceiling leaves a single policy's number uninterpretable.
- Q: Was the standalone requirement already satisfied? → A: **At the design level yes, in the
  build no.** FR-034 already grades through `IEvictionPolicy`, so policy internals never reach
  the harness. But the crate table gave `apps/workload-generator` a hard Cargo dependency on
  `eviction-policy-lru`, so the "standalone" generator could not build without the current LRU
  implementation. Corrected to depend on `interfaces` only, with policy components bound at
  runtime and present as dev-dependencies for tests.
- Q: Is OPT an exact ceiling? → A: **Only for uniform entry size in a single tier.** *(Moot as of
  the 2026-08-07 session below: OPT is deferred entirely, because its hit rate is a function of a
  capacity the generator does not know. The analysis stands for whenever it returns.)* Belady's
  optimality assumes uniform-size items and one cache level; with heterogeneous sizes offline
  optimal caching is NP-hard, and furthest-next-use is then neither optimal nor guaranteed to
  bound byte hit rate. So OPT MUST be reported as exact only in the uniform-size single-tier
  case and as a labelled non-tight reference otherwise (FR-034a) — never presented as a bound it
  is not. A true bound for the heterogeneous case needs an LP/flow relaxation, parked in
  `research.md`.
- Q: SC-002, FR-057, and two US6 scenarios all gate on "the configured tolerance", but no
  tolerance field exists anywhere in the schema and no default is stated — so `fit` and
  `validate` gate on a value that cannot be set. Where does it live? → A: **On the `fit`/
  `validate` command line, per-statistic, never in the YAML.** Fitting is an operation performed
  *on* a workload model, not a property *of* one, so a tolerance in the YAML would breach the
  five-section factoring and would make two models with identical workload content compare
  unequal. And it must be per-statistic: FR-056's four statistics — reuse-distance CDF,
  prefix-sharing depth histogram, request-length distribution, unique-keys-over-time — are on
  four different scales, so a single scalar threshold across them has no consistent meaning.
  Defaults are to be derived in `research.md` rather than asserted, and the values actually used
  are recorded in the validation report so a pass is reproducible.
- Q: `run.wss_window` is a *time* span, but under `closed_loop` arrival times depend on system
  response and `t_ns` is advisory only — so neither `sessions_per_window` (FR-009f) nor the
  working set behind `fraction_of_wss` *(a field since removed — the working-set size is now
  published as a statistic rather than consumed as a capacity input, see the scope session
  below)* is knowable at plan time, leaving a MUST-reject rule
  unevaluable and capacity sizing undefined. → A: **Define the window canonically as a request
  count.** The plan is a sequence, so a count is exact in both arrival modes, and it is exactly
  convertible under `open_loop` (count = rate × duration), so `60s` survives as sugar and the
  ergonomics are unchanged. A duration combined with `closed_loop` is a schema error, since only
  `open_loop` supplies a rate. This also fixes a quieter defect in `open_loop`: a time window
  drifts whenever the schedule slips, which FR-061 exists to report — so both quantities were
  previously functions of how fast the system ran, not of the plan.
- Q: The § Fitting table says `branch_factor` is *measured* from the trace while FR-009g makes
  `auto` the recommended default — for a fitted model these disagree, and nothing said which
  wins. → A: **`fit` emits the measured value** (trunk structure is a physical property of the
  trace), records what `auto` would have chosen beside it, and **fails per FR-057** if the
  measured combination violates the occupancy floor, rather than silently substituting: a
  combination the generator cannot realise is precisely what FR-057 exists to refuse. *Note that
  the count-based window resolved above removes the rate-portability worry this question
  originally carried — `sessions_per_window` is now `wss_window / mean(turns)`, independent of
  rate, so a fitted model no longer loses sharing fidelity when replayed faster or slower.*
- Q: Is a measured `branch_factor` even well defined? → A: **Only where occupancy is high**, and
  the report must say so. A trace reveals only *visited* nodes: with many sessions per trunk path
  most children are observed and the measured width ratio approaches the truth, but at low
  occupancy each session sits alone on its own path and the ratio collapses toward **1 whatever
  the true branching**. The bias is toward the degenerate answer, so an unqualified measurement
  would read as "linear trunk" exactly where the data cannot say. Conveniently the trustworthy
  region is the same high-occupancy region in which the model is valid at all (FR-055b).

### Session 2026-08-06 (scope — the generator knows nothing about storage)

- Q: The seven-value `served_by` taxonomy is derived "from the server's `served_by` field"
  (FR-039) but SC-007 demands it of *every* report, and an offline replay has no server to ask.
  Which reports must attribute? → A: **The question was posed too narrowly.** The generator has
  no business knowing anything about Certus's internals, *including what tiers it might have or
  whether it has tiers at all*. Its job is to generate block traces following stated statistical
  rules; how that output maps onto any consumer's internals is not its concern. Certus needs the
  logging that reports per-tier hit and miss ratios for a given workload — that is real and
  wanted — but it is Certus's to produce, not the generator's to model or to ask for.
- Q: How far does that reach? → A: **Anything relating to tiers, memory caches, or disks is
  removed from the generator completely** (FR-018a). Concretely: the `system:` section is gone
  in full — capacities, `eviction_policy`, `pin_fraction`, and the `thresholds` that mapped onto
  `DispatcherConfig`; `topology.holder_tier` is gone, and with it FR-020's driving of
  `FlushToSsd` / `ClearMemoryTier` during setup; the plan record's `HOLDER_TIER_SSD` and `PIN`
  flag bits are gone; and the plan-format contract no longer enumerates the seven serving
  classes. Multi-node support **stays** — an inference deployment runs parallel across nodes and
  the stream arriving at each node is a genuine workload property (FR-018b) — but is now stated
  purely as *which node asks for which key when*, never as where a copy lives.
- Q: Is anything lost that was actually load-bearing? → A: **Capacity-relative sizing, and it
  comes back as a published statistic instead of an input.** The useful content of
  `fraction_of_wss` was never the cache size but the *ratio*; so the generator reports the
  realised working-set size over `run.wss_window` and a consumer computes its own capacity from
  it. A capacity sweep is then a loop over the consumer's own flag against one fixed plan —
  which is strictly better for the comparison, because a consumer-side sweep must hold the key
  stream identical across points (FR-036) while a workload-side sweep must change it. Keeping
  the two in separate places makes it hard to vary both at once, which is the mistake that
  silently invalidates a comparison. Three fields were also renamed away from cache vocabulary,
  since a workload cannot state an outcome: `replication.holders_per_key` → `nodes_per_key`,
  `global_miss_fraction` → `cold_fraction`, and the plan flag `EXPECT_GLOBAL_MISS` → `COLD`
  (which asserts only that warmup did not pre-request the key, not that anything will miss).
- Q: Did the spec already agree with this somewhere? → A: **Yes, in two places, which is what
  made the wide reading safe.** Design rule 1 already fenced `system:` off as a separate axis
  from `corpus`/`workload`, and US2's own command line already read
  `simulate --policy lru --capacity-sweep 0.1,...` — so policy and capacity were *already*
  command-line options and the schema fields duplicated them. The Q1 answer earlier in this
  clarify pass had also established that the generator must not be specified in terms of any
  policy's internals; this is the same principle carried to the storage structure.
- Q: Does an offline replay still have a place? → A: Yes, but as a *consumer* of a plan rather
  than as part of the generator, and one already exists: `tools/simulator/` is a SimPy
  discrete-event model of the two-tier server that already replays a block-trace JSONL
  (`run_sim.py --trace workload.jsonl`). So the tier-aware simulator this spec was going to grow
  is largely built, on the other side of the boundary, and takes as input very nearly what the
  generator should emit. FR-035 accordingly no longer asks the generator to model a hierarchy —
  only that any offline replay document what it cannot model, since a block-reference trace
  reproduces the reference pattern exactly and reproduces nothing about time.

### Session 2026-08-07 (cache simulation deferred out of scope)

- Q: With tiers removed, what remains of US2's `simulate` — an offline replacement-policy grader?
  → A: **Deferred entirely; the feature is synthetic workload generation and nothing else.** It
  can be brought back if it turns out to be needed. Three reasons, the last two the user's: a
  simulator is a *consumer* and so may not be specified inside the generator (FR-018a); a
  realistic one would have to **share the evolving cache-replacement code with Certus**, and
  while the component design probably makes that sharing mechanically easy, easy coupling is
  still coupling in the wrong direction; and **the disk tier has nowhere to live except real
  disks** — device queueing, per-drive bandwidth and write amplification are what an SSD tier
  *is*, so a discrete-event approximation yields error of unknown magnitude, which is worse than
  no figure because it reads as a measurement.
- Q: Does that cost the P1, CI-testable story? → A: **No — it splits it, and the valuable half
  was never the cache.** US2 becomes *characterise a plan without running it*: the
  reuse-distance CDF, compulsory-miss floor, sharing-depth histogram, request-length
  distribution, unique keys over time, trunk occupancy, and working-set size. All are properties
  of the reference stream, all are computable from the plan alone, and all are exactly what makes
  a later hardware number interpretable. The reuse-distance CDF carries most of the deleted
  value on its own, being a capacity-free object that *encodes* the achievable hit-rate curve —
  so a consumer reads off what any capacity would buy without this tool simulating a cache.
- Q: Did anything not survive the split cleanly? → A: **Belady/OPT, and the reason is worth
  recording because the earlier draft got it wrong.** FR-034a had justified OPT as a workload
  statistic because it needs no `IEvictionPolicy` implementation. That is true but insufficient:
  Belady evicts furthest-next-use *when full*, so OPT hit rate is a function of capacity — a
  curve over a quantity the generator does not know. It defers with the simulator. The
  compulsory-miss floor survives because it is the miss rate at *unbounded* capacity and so needs
  no capacity at all. SC-005 also had to change: an analytic-LRU check needs a cache, so the
  equivalent check is now the measured reuse-distance CDF against the analytic Zipf
  reuse-distance distribution — which tests the stream directly instead of inferring its
  correctness through a model of something consuming it.
- Q: What does this feature still give a future policy comparison? → A: The two things that make
  one valid: **identical streams, provable by digest** (FR-036), and the capacity-free statistics
  that say what the workload was. Those hold whether the comparison runs here, in
  `tools/simulator/`, or somewhere that does not exist yet.

### Session 2026-08-07 (measured against real traces)

A collection of real LLM traces became available and the model was checked against it rather than
reasoned about. The traces themselves are **not in this repository and are not a dependency** — they
were a sample of convenience, and what survives here is the *format* (`contracts/trace-io.md`) plus
structural observations that any similar trace would show. Traces are described below by character
rather than by name, deliberately: a name would be a dangling reference, and no requirement should
rest on one file.

- Q: Are the `.jsonl` and parquet files different levels of detail — tokens versus blocks? → A:
  **No, they are the same schema in two containers.** The jsonl is a 3–136 line eyeball sample of
  the invocations table, sharing 17 of 18 fields with the parquet. Neither holds tokens or text
  anywhere; both are block-level, carrying block IDs plus token *counts*. So there is one input
  format to support, not two, and the generator's file output modes can emit exactly it.
- Q: Can these traces actually drive `fit`? → A: **Yes, with a three-way split by what they carry.**
  `raw_text` traces give everything including block roles; `pre_hashed` gives structure
  and arrival but no roles; `metadata_only` traces have **no block data at all** and can only supply arrival
  and token-length distributions. Two further disqualifications matter more than the class: a null
  `session_id` makes `turns`, `growth_per_turn` and the FR-009a root binding unfittable, and absent
  timestamps make reuse statistics order-dependent. So the best fit target is a *production* trace
  with native session IDs and real timestamps, cross-validated against one at the opposite end of
  the sharing spectrum — the selection criteria matter, the particular files do not.
- Q: Do the traces confirm the model, or contradict it? → A: **Confirm the sharing model, refute
  the trunk-shape model.** `id_semantics: rolling_prefix` in every trace examined that carried blocks is
  exactly the FR-008 key design, which is strong external validation. But the **width-by-depth
  profile is piecewise, not smooth**: width stays *exactly* constant for 16–40 consecutive depths
  and then jumps at particular depths. A uniform `branch_factor` of even 1.05 would widen 7× across
  40 flat levels, so the scalar was not a coarse approximation of the real shape but a different
  shape. Hence `branch_factor` → the `branching` profile (FR-009e, FR-009e1).
- Q: Is the interesting case a global prefix, then per-branch commonality from tool use, then
  fanout? → A: **Yes, and real traces have it.** An agentic tool-use trace fanned out at depth 1
  *and* again at depth 23, with a flat region between. That shape — everything shares the preamble, each branch then
  shares a tool definition or retrieved document, and only then does the private tail begin — is
  the one that matters most for a cache, because two sessions on the same branch share far more
  than two on different branches, which no single `shared_depth` expresses. The profile makes it
  statable. **What is still not expressible** is fanout depths that differ *between* branches: the
  profile is global, so the trie is self-similar. No trace examined demanded more.
- Q: Does a depth-varying fanout reopen the non-monotone-sharing question that killed the
  Pitman–Yor `by_depth` table? → A: **No, and the earlier note conflated two things.** Divergence
  is still irreversible and sharing is still a monotone prefix property of any *pair* of sessions —
  that is what the rolling hash requires. What varies by depth here is only **how many children a
  node has**, a property of the trie's shape rather than of any pair's sharing, and a node with one
  child at depth 20 and forty at depth 21 contradicts nothing. The 2026-08-05 remark that
  depth-varying branching "was unrealisable anyway" was wrong; only depth-varying *sharing* is.
- Q: Then why did the scalar fit look fine? → A: **It averaged flat runs against rare jumps.** The
  measured scalar comes out 1.009–1.078 on agentic traces, and the same estimator gives 7.6–82 on
  chat and retrieval, which is not a trunk width but one enormous near-root jump. A near-root jump
  *can* be absorbed by redefining the root boundary — a trace showing 155 roots that each split 31
  ways is better described as ~4 900 roots, which is now FR-055c — but fanout has been observed at
  **depth 124**, and no choice of root boundary reaches that.
- Q: Anything that validates a number previously taken on faith? → A: **`target_occupancy = 4`.**
  Observed occupancy settled in the low single digits below the fanout points and held there across
  hundreds of depths. That sits just under the target, which is the right side for a floor to design
  against — so the judgement stands *consistent with* observation rather than established by it,
  which is as far as a small incidental sample licenses (FR-009g1).
- Q: The format's `block_size` counts tokens and `model_config` was null in every trace on hand, so
  KV bytes are not recoverable. How serious? → A: **Not serious, because entry size is a chosen
  parameter and not a fitted one.** Size does not affect the generated reference pattern; it affects
  only when a consumer's storage fills. So `block_bytes` stays an input, is recorded in every
  report, and is never derived from a trace (FR-011a). Two consequences worth stating: the payload
  becomes **the 8-byte key followed by zero padding**, which costs nothing and makes a returned
  value self-identifying, so a consumer returning the *wrong value* — not merely the wrong size —
  is detectable (FR-011b); and with a constant `block_bytes`, **byte hit rate is object hit rate
  times a constant and carries no independent information**, which FR-040 must say rather than
  presenting two numbers as two findings.
- Q: What about block roles and the fan-in DAG? → A: **Both deferred, with reasons.** Roles explain
  *why* blocks are reused while the generator models *that* they are, and the statistics that drive
  cache behaviour are role-agnostic; the format carries roles, so it stays recoverable. Fan-in was
  vanishingly rare and **`reuse_from` was empty in every instance of it**, so it is a scheduling
  dependency rather than prefix reuse — not something a prefix model omits.
- Q: Did reading real traces find a defect in this spec? → A: **Yes, in US6's own command line.** It
  passed `500convs-64g.handler.jsonl.gz` to `fit`, but the `*.handler.jsonl` files are GPU↔CPU
  *transfer* records (`medium`, `block_ids`, `group_sizes`); the KV key trace is `*.mgr.jsonl`. The
  example named a file that is not a key trace at all.

### Session 2026-08-07 (unbounded runs and session lifetime)

- Q: A trace is finite with fixed branching at each level, but the generator must run for an
  arbitrary number of blocks with branching that *averages* the configured value. Is that already
  the case? → A: **Yes for the stochastic branching, and it was never the trace's fixed counts that
  got copied.** FR-009e makes each fanout a *mean* realised by randomised rounding — a node at
  fanout 1.18 gets one child with probability 0.82 and two with probability 0.18 — so realised width
  at any depth varies around the configured value and converges as more of the trie is visited. The
  draw is keyed on the **node**, not on the visit (FR-009b), which is what makes an arbitrarily long
  run reproducible and independent of arrival order while still being stochastic in the way that
  matters.
- Q: Is the generator already able to run indefinitely? → A: **Mechanically yes, and it had never
  been said.** FR-010 bounds resident memory by active paths rather than by keys minted, every
  segment's fanout ≥ 1 makes the trunk unbounded in depth, and private branches mint fresh keys per
  session, so distinct keys grow without bound. What was missing was the statement that makes those
  add up to an unbounded run: **that sessions retire.**
- Q: So do we need a concept of session lifetime, retiring old sessions and creating new ones? → A:
  **Yes, and it was genuinely absent — but it must be *derived*, not configured.** Now FR-014b:
  a session is born on arrival, binds its root, issues `turns` requests separated by `think_time`,
  and is retired when its last turn completes, at which point its private keys are dead forever
  (already guaranteed by FR-009c's disjoint private namespaces). Lifetime is `Σ think_time`, so a
  `lifetime` field would be a third statement of a quantity `turns` and `think_time` already fix —
  the same over-determination design rule 3 rejects for `gets_per_key`. FR-014c states the
  consequence: arbitrary run length comes from continuously retiring and creating sessions.
- Q: Then how many sessions are live at once, and does anything depend on it? → A: **Two things
  depend on it and neither was checkable.** Under `open_loop` the live population follows from
  Little's law — `(rate / mean(turns)) × (mean(turns) − 1) × mean(think_time)`, about **10 000** for
  the worked example — and under `closed_loop` it simply *is* `arrival.concurrency`, which is
  legitimate there because `closed_loop` has no rate. So it is computed and reported, never a field
  (FR-015a). It is the constant in FR-010's `O(active paths)`, and it is what warmup must be
  measured against. Note it is **not** `sessions_per_window` (FR-009f): occupancy needs how many
  sessions have *walked* the trunk in a window, memory needs how many are walking it *now*.
- Q: Did looking for it find a defect? → A: **Yes — warmup had no relationship to the session
  model.** At t=0 no session is live and the population fills over roughly one mean lifetime, so a
  measured window opening sooner sees less concurrency, less trunk occupancy and less sharing than
  configured — and all three read as properties of the workload rather than of the clock. The worked
  example survived on luck (15 s ramp inside a 20 s warmup); `turns: geometric(50)` with
  `think_time` median 30 s implies a **~24 minute** ramp against which a 20 s warmup measures pure
  transient. FR-015b now computes the ramp and **rejects** a shorter warmup, rather than warning,
  because those numbers are wrong rather than noisy.
- Q: Is anything about an unbounded run still not representative? → A: **Yes, and it is the one
  limitation that *grows* with run length.** `drift` re-weights which shared keys are popular but
  never changes which shared keys exist, and the shared space is bounded by construction, so on a
  long run every trunk key is touched and then re-touched forever while all novelty comes from
  private branches. Real deployments churn shared content — documents re-indexed, prompts
  redeployed, threads aging out — and this model does not. FR-016a requires the report to declare it
  rather than leaving it to be inferred; fixing it would mean retiring and minting trunk keys, which
  is a corpus-churn model this feature does not have.

### Session 2026-08-07 (shared-content churn; reference counting rejected)

- Q: Can sessions hold reference counts on the shared nodes they use, so a shared node goes away when
  its last user ends — and would that fix the immortal-trunk problem? → A: **We could, but it would
  not fix it, for three reasons.** (1) **It cannot create novelty**: node identity is a pure function
  of the path, so a "retired" node is re-derived *identically* the moment another session walks the
  same child indices. Deletion hides a key briefly; only a change to the hash input produces a new
  one. (2) **Refcount-zero fires in the wrong places**: for the worked example there are ~833 live
  sessions on each of 12 roots but only ~1.1 per distinct path at depth 40, so refcounts essentially
  never reach zero near the root and reach it constantly deep down — the scheme would churn the
  nearly-private deep nodes and never the popular shallow ones, which is the reverse of content
  lifecycle, and the case that matters most (a redeployed system prompt) hits the *top* of the
  trunk. (3) **It would re-couple `corpus` to `workload`**: node existence would depend on arrival
  timing, so changing the request rate would change which keys exist — precisely the objection that
  ruled out a Chinese-restaurant minting rule on 2026-08-06. Prohibited explicitly in FR-016c so the
  idea is not re-derived later.
- Q: Was the instinct wrong, then? → A: **No — it is right, and it is already implemented where it
  applies.** Reference counting is the correct model wherever the reader *owns* the content, and
  that is the private path: FR-009c makes private namespaces disjoint per session and FR-014b kills
  them at retirement, with no count needed because the count is known to be one. What refcounting
  cannot model is content whose lifecycle is independent of its readers, which is what shared
  content is.
- Q: So what does fix it? → A: **A generation term in node identity, advancing on the corpus's own
  schedule.** FR-008 becomes `child_id = H(parent_id, child_index, generation(node))` with
  `generation` fixed at 0 by default, so the default reduces exactly to the old derivation. FR-016b
  adds `corpus.trees.churn.half_life`, advancing per node from the node's own identity and the seed.
  Because the key is a rolling hash, **rotating a node invalidates its whole subtree implicitly** —
  which is why one number covers whole-tree replacement (the root rotates), per-branch content
  replacement (a mid-trunk node rotates), and everything between. It preserves determinism,
  path-computable identity, `O(active paths)`, and — the property refcounting would have lost —
  orthogonality, since churn is a function of the seed and the clock and of nothing about who is
  reading.
- Q: Does one half-life give sensible behaviour at every depth? → A: **Yes, and the emergent
  behaviour is the realistic one.** A depth-*d* path survives only while all `d+1` of its nodes do,
  so a path's effective half-life is `half_life/(d+1)`: shallow shared prefixes are stable and deep
  ones are fragile, which is the way round real deployments behave. A `branching` segment may
  override the half-life for cases that need saying explicitly — prompts stable for weeks, retrieved
  documents turning over daily.
- Q: What does churn interact with that could break quietly? → A: **The occupancy floor, and it
  would have.** A trunk path accumulates sharers only while it exists, so occupancy must be computed
  over `min(wss_window, path_lifetime(d))` rather than the whole window (FR-016e). Without that term
  the floor would approve a configuration whose sharing churn then destroys — and because
  `path_lifetime` falls as `1/(d+1)`, a half-life generous at depth 4 can be far too short at depth
  40. Same failure shape as a warmup shorter than the session ramp: internally consistent, passes
  every other check, does not measure what it claims to.
- Q: Should `drift` and `churn` be one parameter? → A: **No.** A popularity shift leaves a cache's
  contents *valid* and changes only what is asked for next; a content replacement *invalidates*
  what is already held. One half-life covering both would mean two physically different things, and
  distinguishing them is the point of the new Test Matrix rows — policies that cope well with drift
  need not cope well with invalidation.
- Q: Can churn be fitted? → A: **No, and it must not be faked.** Its signature is a trunk key used
  and then never used again, but available traces span hours while plausible content cadences run to days
  or weeks, and a half-life beyond the observation window is indistinguishable from no churn. A
  fitted value would be an artifact of trace length, biased **short** — the direction that
  manufactures misses. So `fit` leaves it unset and MAY report a lower bound (FR-055d); setting it
  stays a deliberate act by whoever knows the deployment's cadence.

### Session 2026-08-09 (containers, encodings, and reading JSONL)

- Q: Are there two different parquet *formats* for trace input? → A: **No — one schema with two
  population patterns.** Every column exists in every invocations file; what differs is which are
  *populated*. Delta fills `new_*`/`reuse_from` and leaves `full_*` empty; full does the reverse. So
  it is one parser with a branch, not two parsers, and `contracts/trace-io.md` now separates the three
  things that were easy to conflate: **container** (parquet or JSONL), **population pattern** (delta
  or full), and **capability** (`field_status`). Only the second needs a decision from a reader, and
  the contract recommends **normalising on ingest** — reconstruct full block lists once at the
  boundary — because otherwise the branch leaks into every statistic that walks a block list, and each
  such site is a chance to get the trailing-partial-block convention wrong.
- Q: Should JSONL be supported too, or does it carry less information? → A: **Same information per
  record; drastically less coverage — and the distinction matters more than either fact.** Measured
  against the corresponding parquet: the only JSONL-only field is a redundant `block_size` (already in
  the path and manifest), the only parquet-only field is `parent_invocations`, and that is omitted
  exactly where it would be empty in every record — the one trace with real fan-in does carry it. Every
  sampled row was located in the parquet (6/6, 136/136, 3/3). But the shipped files are eyeball
  samples: **6 lines against 1 960 074 parquet rows** in one trace, 136 against 2 115 623 in another.
- Q: So what is the requirement? → A: **Read both containers, and refuse to fit from a partial
  trace.** FR-055 now requires either container and either population pattern, symmetric with
  FR-021a's output modes — and that symmetry is the argument, not a preference: the generator *emits*
  JSONL, so refusing to read it would leave its own output unconsumable by its own tools. FR-055e
  refuses a partial trace and judges partiality by comparing records consumed against
  `block_stats.<block_size>.invocations`, **not** by filename, because a `sample_` prefix is a
  convention rather than a guarantee. `validate` may proceed on a sample but must label its size.
  Two smaller rules follow from the field differences: a per-record `block_size` disagreeing with the
  manifest or path is rejected rather than resolved, and an absent `parent_invocations` means empty
  rather than unknown.
- Q: Did requiring JSONL input buy anything beyond symmetry? → A: **Yes — the round trip, which is now
  the best test `fit` has** (FR-058a). Generate a plan from a known YAML, emit it as a trace, re-fit,
  and compare recovered parameters against the originals. Ground truth is *exact* rather than
  estimated, so any divergence is a defect in `fit`, the emitter, or the reader rather than a property
  of some real workload — and it is the only check that exercises emitter and reader against each
  other. It also needs no external data, so unlike fitting a real trace it runs in CI.
- Q: Did that expose a contradiction? → A: **Yes, in US6's Independent Test**, which said "fit against
  a checked-in trace excerpt". Traces are not checked in, and FR-055e now forbids fitting from an
  excerpt, so the stated test was both impossible and prohibited. It is now the round trip, with
  fitting a real trace kept as a separate non-CI check.

### Dependencies on other components (implied by the above)

1. **`apps/certus-server-yaml` / `apps/certus-server` proto** — add `served_by` to
   `EntryResult` (`DRAM | SSD | REMOTE_DRAM | REMOTE_SSD | MISS | SIZE_MISMATCH | ERROR`) and
   plumb the dispatcher's existing internal knowledge of which tier resolved a lookup out
   through the gRPC layer. **Specified in
   `components/dispatcher/specs/002-served-by-tier-attribution/`**; this spec depends on that
   feature's `contracts/served-by.md` as the normative taxonomy and does not restate its
   semantics. **Blocks all hit-rate measurement (US3, US4).** It does not block US1 or US2.
   Note that it also requires an `IRemoteLookup::batch_lookup` signature change to carry the
   peer's advertised tier, and that the tier must derive from that advertisement rather than
   from the operation's phase — phase is per-operation and transitions on quorum and timeout,
   so it is not a tier proxy.
2. **`certus-server-yaml` `rw-telemetry` forwarding** — the `GetIoStats` read/write byte
   counters this spec wants for its independent cross-check are zeroed unless the *active*
   dispatcher is built with `rw-telemetry`, and `apps/certus-server-yaml/Cargo.toml:53`
   forwards that feature only to `dispatcher`, not to `dispatcher-p2p`. Under
   `--features p2p-native` — the configuration US3 and US4 actually run — nothing enables
   them, and `components/dispatcher-p2p/src/lib.rs:2589` returns a zeroed aggregate. Enabling
   the cross-check therefore requires a Cargo feature change in `certus-server-yaml`
   (forwarding to `dispatcher-p2p`, which already defines the feature at
   `components/dispatcher-p2p/Cargo.toml:28`). **Blocks SC-007a and the byte-provenance
   cross-check in FR-042a, not attribution itself (SC-007).**
3. **`components/eviction-policy-lru`** — **no dependency in either direction.** An earlier draft
   consumed it through `IEvictionPolicy`; cache simulation is deferred out of this feature
   (FR-034), so nothing here touches it.
4. **`components/dispatcher`** — no change required *for configuration*, and the generator no
   longer supplies any. `DispatcherConfig`'s capacities and eviction thresholds
   (`max_cache_entries`, `memory_tier_eviction_threshold` / `_low_watermark`,
   `ssd_eviction_threshold` / `_low_watermark`) are set by whoever deploys the server, exactly
   as they are for any other run; an earlier draft mapped a `system:` section onto them, which
   FR-018a removes. The dispatcher does change under dependency 1, which is that feature's
   scope rather than this one's.
5. **`components/remote-lookup`** — the `topology:` section is realised entirely by *which node
   asks for which key*, plus the existing `CERTUS_RL_*` environment overrides for group,
   deadlines, and quorum percentage, which are operator settings rather than workload fields.
   No change for the request streams; it does change under dependency 1 to surface the peer's
   advertised tier through `IRemoteLookup::batch_lookup`.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Compact YAML to Reproducible Plan (Priority: P1)

A performance engineer writes a ~40-line YAML describing a conversational KV workload with
shared system prompts, and runs `certus-workload plan -c conv.yaml -o conv.plan`. The tool
emits a deterministic, content-hashed event plan of millions of requests. Re-running with
the same seed produces a byte-identical plan.

**Why this priority**: Every other story consumes the plan. It is also the story that
proves the central claim of the design — that a compact statistical description expands
into a rich, prefix-structured key stream — and it needs no hardware at all.

**Independent Test**: Fully testable in CI. Generate a plan from a checked-in YAML, assert
the content hash, and assert structural properties of the output (prefix-sharing depth
histogram, request-length distribution, unique-key growth curve) against expected ranges.

**Acceptance Scenarios**:

1. **Given** a YAML with `seed: 0xC0FFEE`, **When** `plan` is run twice, **Then** both plans
   have identical content hashes.
2. **Given** two YAMLs differing only in `seed`, **When** each is planned, **Then** the
   content hashes differ but both satisfy the same distributional assertions.
3. **Given** `corpus.trees.roots.count: 12`, **When** 10 000 requests are planned, **Then**
   exactly 12 distinct depth-0 keys appear, every session's requests all begin with the same
   one, and the sessions-per-root histogram matches the configured `roots.popularity`.
4. **Given** a `shared_depth` distribution whose realised median is *d*, **When** the same
   plan is generated, **Then** the measured prefix-sharing depth histogram matches that
   distribution, and ≤ 5% of keys deeper than the session's own `shared_depth` are shared by
   more than one session.
5. **Given** a plan of 10^7 requests, **When** it is generated, **Then** resident memory
   stays O(active paths) and does not scale with the number of distinct keys minted.

---

### User Story 2 - Characterise a Plan Without Running It (Priority: P1)

The engineer runs `certus-workload report -p conv.plan` and gets, in seconds and with no SPDK,
GPU, or RDMA, the statistics that say what this workload *is*: the reuse-distance CDF, the
compulsory-miss floor, the prefix-sharing depth histogram, the request-length distribution,
unique keys over time, realised trunk occupancy, and the realised working-set size over
`run.wss_window`.

**Why this priority**: These are the numbers that make a later measurement interpretable, and
every one of them is computable from the plan alone — so this is the story that is fully
CI-testable, and it is what lets an engineer discover that a workload is degenerate before
spending hardware time on it. The reuse-distance CDF is the load-bearing one: it is a
capacity-free object that *encodes* the achievable hit-rate curve, so a consumer can read off
what any capacity would buy without this tool simulating a cache to tell it.

**Independent Test**: Run `report` over a checked-in pure-Zipf plan and assert the measured
reuse-distance CDF matches the analytic Zipf reuse-distance distribution within tolerance. This
validates the generator end-to-end — that it really emitted the distribution it was asked for —
by testing the stream directly rather than inferring its correctness through a cache model.

**Acceptance Scenarios**:

1. **Given** a pure-Zipf plan with known parameters, **When** `report` runs, **Then** the
   measured reuse-distance CDF matches the analytic distribution within the per-statistic
   tolerance of FR-057a.
2. **Given** one plan consumed twice by any two arms, **When** both complete, **Then** both
   consumed exactly the same key stream in the same order, asserted by identical stream
   digests, so that any difference in result is attributable to the arm alone. The generator's
   criterion is **stream identity**; what the arms are, and what is compared between them, is
   the consumer's concern.
3. **Given** any plan, **When** `report` runs, **Then** the compulsory-miss floor is stated, so
   that a consumer's hit rate can be read against the best any consumer could do on this
   workload. The floor requires no capacity and no cache model — it is the miss rate at
   unbounded capacity.
4. **Given** a plan with heterogeneous entry sizes, **When** `report` runs, **Then** the
   statistics are given both per object and per byte, and are permitted to differ.
5. **Given** a `scan`-shaped mixture entry (`turns: 1` with a large `private_depth`)
   interleaved with a hot conversational set, **When** `report` runs, **Then** the reuse-distance
   CDF shows the resulting bimodality and the scan's share of unique keys is quantified — which
   is the workload-side statement of scan pollution, without asserting what any cache will do
   about it.

---

### User Story 3 - Single-Node Hardware Measurement with Tier Attribution (Priority: P1)

The engineer runs `certus-workload-run run -p conv.plan --endpoint localhost:50051` against
a `full-p2p` or `full` profile server and gets throughput (GB/s, keys/s), latency
percentiles **broken down by serving tier**, and hit rate per tier.

**Why this priority**: This is the first story that measures the real system, and it
delivers value with one machine. Latency percentiles that mix DRAM hits with SSD reads are
meaningless, so the per-outcome breakdown is the point, not a refinement.

**Independent Test**: Run against a single node with a plan whose reported working-set size
exceeds what that server was configured to hold in memory — a comparison between a published
workload statistic and the operator's own server configuration, not something the plan states —
and assert the reported split is internally consistent: every entry attributed, hits + misses +
errors equal to entries requested, and the slower-medium fraction rising monotonically as the
server's configured capacity is reduced across a sweep driven from the server side. The
`GetIoStats` byte cross-check is a
*conditional* addition to this test, available only when the server was built with
`rw-telemetry` forwarded to the active dispatcher (Dependencies 2); the report states whether
it ran.

**Acceptance Scenarios**:

1. **Given** a plan and a running server, **When** `run` completes, **Then** every request's
   outcome is classified into exactly one of the seven `served_by` values
   (`DRAM | SSD | REMOTE_DRAM | REMOTE_SSD | MISS | SIZE_MISMATCH | ERROR`) with no "unknown"
   bucket, and hits + misses + errors equals the number of entries requested.
2. **Given** a completed run, **When** the report is produced, **Then** p50/p90/p99/p99.9
   latency is reported *per outcome class* as well as in aggregate.
3. **Given** a server that predates serving-tier attribution, **When** `run` completes,
   **Then** the responses carry `SERVED_BY_UNSPECIFIED` and the report says "attribution
   unsupported by server" rather than guessing a tier or inventing an unknown bucket.
4. **Given** a server built with `rw-telemetry` reaching the active dispatcher, **When** the
   report is produced, **Then** reported SSD-served bytes are compared against the
   `GetIoStats` read-byte delta and any disagreement beyond the derived tolerance is flagged
   rather than silently reported.
5. **Given** a server without that telemetry, **When** the report is produced, **Then** the
   cross-check is reported as `unavailable` with the reason, and MUST NOT be reported as
   passing. A zeroed counter must never read as agreement.
6. **Given** `run.warmup: 20s`, **When** the report is produced, **Then** operations inside
   the warmup window are excluded from all steady-state statistics and counted separately.
7. **Given** the generator running at the platform's measured ceiling, **When** the run
   completes, **Then** the report includes a generator-overhead figure demonstrating the
   harness was not the bottleneck.

---

### User Story 4 - Multi-Node Remote-Lookup Measurement (Priority: P2)

The engineer runs the same plan across a symmetric 4-node cluster with
`topology.self_affinity: 0.25`, and gets the remote-served fraction, whatever split the server
reports across its remote classes, and the cost of keys nothing had seen before — plus proof
from `GetIoStats` that the bytes came over the fabric rather than off local disk. Every one of
those breakdowns is the *server's* account of what it did, relayed; the plan asked only for a
set of keys in an order, at nodes.

**Why this priority**: This is the measurement that only Certus needs and that a generic
cache simulator cannot produce. It is P2 only because it depends on US1–US3 and on a
symmetric cluster being available.

**Independent Test**: Sweep `self_affinity` from 0.0 to 1.0 on a fixed plan and confirm the
measured remote-hit fraction tracks the configured value, and that at 1.0 the fabric byte
counter is ~zero.

**Acceptance Scenarios**:

1. **Given** `self_affinity: 1.0`, **When** the run completes, **Then** the remote-served
   fraction is ~0 and no RDMA bytes are attributed to the measured window.
2. **Given** `self_affinity: 0.0`, **When** the run completes, **Then** essentially every
   hit is remote-served and the fabric byte count accounts for the delivered bytes.
3. **Given** a plan whose distinct bytes per node exceed what the peers hold in their faster
   medium — a statement about the workload, with no field naming a medium — **When** the run
   completes, **Then** whatever split the server reports across its remote classes is reported
   as the server gave it, and the generator asserts nothing about which class an individual key
   should have landed in. It cannot: it does not know what tiers the server has.
4. **Given** any remote class whose share changes over the course of a run, **When** the report
   is produced, **Then** the drift is reported as observed and MUST NOT be presented as a
   regression. Serving from a peer's slower medium may promote the entry within that peer, so a
   class share is expected to move with reuse; the generator does not model that promotion and
   MUST NOT claim a stable share for any class.
5. **Given** `cold_fraction: 0.30`, **When** the run completes, **Then** the report states what
   fraction of total wall time went to keys nothing had seen before, using the server's own
   attribution for what happened to them.
6. **Given** a plan with many nodes requesting one absent-then-arriving key simultaneously,
   **When** the run completes, **Then** the count of distinct remote fetches issued for that
   key is reported, exercising remote-lookup's single-flight dedup.
7. **Given** a plan run across N nodes, **When** each node loads its slice, **Then** each
   verifies the distributed plan's content hash and aborts on mismatch.
8. **Given** any remote hit, **When** it is attributed, **Then** the reported tier is the
   peer's *advertisement*, and the report MUST label it as such rather than as serve-time
   ground truth. No wire-protocol change is available to obtain the latter.

---

### User Story 5 - Cluster Symmetry Preflight (Priority: P2)

Before a comparative run, `certus-workload-run preflight --nodes node2,node7,...` inspects
every node and either certifies the cluster symmetric or fails with a specific report of
what differs.

**Why this priority**: It is the mechanism that lets US4 stipulate symmetry rather than
work around asymmetry. Without it, an asymmetric cluster produces plausible numbers that
are quietly incomparable — the most expensive kind of measurement error.

**Independent Test**: Run preflight against a cluster with a known asymmetry (a node whose
NIC and GPU sit on different NUMA sockets) and confirm it fails and names that node and
that specific attribute.

**Acceptance Scenarios**:

1. **Given** a cluster where all nodes match on every checked attribute, **When** preflight
   runs, **Then** it exits 0 and emits a symmetry certificate embedded into subsequent run
   reports.
2. **Given** a node whose NIC `numa_node` differs from its GPU `numa_node`, **When**
   preflight runs, **Then** it fails, names the node and the attribute, and states the
   expected remedy.
3. **Given** a node whose NIC port speed, GPU model, NVMe count, hugepage capacity, or
   `memlock` limit differs from its peers, **When** preflight runs, **Then** it fails and
   names the differing attribute and both values.
4. **Given** inter-node clock offset above the configured bound, **When** preflight runs,
   **Then** it fails, because cross-node relative timing is part of the model.
5. **Given** `--allow-asymmetric`, **When** preflight fails, **Then** the run proceeds but
   every report is indelibly marked `NON-COMPARABLE` with the reason.

---

### User Story 6 - Fit a Model from a Real Trace (Priority: P2)

The engineer runs `certus-workload fit --trace <path-to-a-trace> -o fitted.yaml` and gets a
starting YAML whose synthetic output statistically resembles that real workload, plus a validation
report comparing the two.

**Why this priority**: It answers the standing objection to any synthetic generator —
"where did these numbers come from?" — and it is what keeps the YAML compact: the file holds
*fitted parameters*, not a trace. P2 because US1–US3 deliver value with hand-written YAML.

**Independent Test**: the **round trip** of FR-058a, which needs no external data and is therefore
CI-runnable. Generate a plan from a known YAML, emit it as a trace file, re-run `fit` against that
file, and assert the recovered parameters match the original within the FR-057a tolerances. Ground
truth is exact here rather than estimated, so any divergence is a defect in `fit`, the emitter, or the
reader — not a property of some real workload. Fitting against a real trace is a *separate*,
non-CI check, since traces are external (§ Scope) and FR-055e forbids fitting from an excerpt.

**Acceptance Scenarios**:

1. **Given** a real trace, **When** `fit` runs, **Then** it emits a valid YAML that `plan`
   accepts without modification.
2. **Given** a fitted YAML, **When** its **reuse-distance CDF** is compared against the real
   trace's, **Then** the two agree within that statistic's own tolerance across the distance
   range that matters for the capacities under test. *This is the primary validation statistic:
   if reuse distance matches, LRU hit rate matches at every capacity.*
3. **Given** a fitted YAML, **When** the prefix-sharing depth histogram, request-length
   distribution, and unique-keys-over-time curve are compared, **Then** each agrees within
   **its own** tolerance, and the validation report records the four tolerance values used.
4. **Given** a fit whose validation exceeds tolerance on any statistic, **When** `fit`
   completes, **Then** it reports which statistic failed and by how much rather than
   emitting a silently unfaithful model.
5. **Given** a YAML, **When** a plan is generated from it, emitted as a trace file, and re-fitted,
   **Then** the recovered parameters match the originals within the FR-057a tolerances — the round
   trip of FR-058a, which is the only check whose ground truth is exact.
6. **Given** the same trace content in parquet and in JSONL, **When** each is fitted, **Then** the
   two fits are identical. The container is not information (FR-055).
7. **Given** a trace file holding fewer records than its manifest's `block_stats` declares — the
   shape of the sample files that ship beside real traces — **When** `fit` runs, **Then** it
   **refuses**, naming the records-consumed and records-declared counts, rather than fitting a
   confident model from a handful of requests (FR-055e).

---

### User Story 7 - Parameter Sweeps with Statistical Reporting (Priority: P2)

The engineer declares a sweep in the YAML (`sweep: {topology.self_affinity: [0.0, 0.25,
0.5, 1.0], repeat: 8}`), runs it, and gets a table with means, confidence intervals, and an
explicit significance verdict per comparison.

**Why this priority**: Prior work on this bench established that n ≥ 8 is needed for
significance and that n = 3 sampling produced misleading conclusions. Encoding that in the
harness prevents rediscovering it.

**Independent Test**: Run a two-point sweep with `repeat: 8` and
confirm the report includes per-point n, mean, cv, CI, and a pairwise significance verdict.

**Acceptance Scenarios**:

1. **Given** `repeat: N`, **When** the sweep runs, **Then** each point is measured N times
   with a distinct per-repeat seed derived from the root seed, and n/mean/cv/CI are reported.
2. **Given** `repeat` unset, **When** the sweep runs, **Then** it defaults to 8 and the
   report states the default was applied.
3. **Given** two sweep points, **When** the report is produced, **Then** it states whether
   the difference is significant at p < 0.05 rather than only printing two means.
4. **Given** a point whose cv exceeds a configured threshold, **When** the report is
   produced, **Then** that point is flagged as unstable.

---

### User Story 8 - Fault and Churn Injection (Priority: P3)

The engineer adds `topology.membership_events` to stop a node partway through a run and
observes hit rate degrading gracefully rather than cliffing.

**Why this priority**: Valuable for robustness characterisation but not needed for the
performance and hit-rate questions that motivate the tool.

**Independent Test**: Run a multi-node plan with one scheduled node stop and confirm the
report segments statistics into before/after windows around the event.

**Acceptance Scenarios**:

1. **Given** a scheduled `stop` event, **When** the run completes, **Then** statistics are
   segmented at the event boundary and both windows are reported.
2. **Given** a node stop, **When** the run continues, **Then** requests for keys that node
   held are reported as global misses rather than as errors.

---

### Edge Cases

- **A working set the consumer never has to evict from** — hit rate saturates near 1.0 and the
  run measures nothing about the consumer's policy. The generator cannot detect this from the
  plan alone, since it does not know the consumer's capacity, so the *report* MUST warn whenever
  the consumer reports steady-state evictions at ~zero.
- **A working set so large that every policy looks alike** — hit rate approaches the
  compulsory-miss floor. This half *is* knowable from the plan: the floor is a workload
  statistic (FR-060), so the generator MUST publish it, and the report MUST warn when the
  measured hit rate is within noise of it.
- **Plan whose arrival rate exceeds system capability under open-loop** — the schedule slips
  and the run stops measuring the configured offered load. The runner MUST detect and report
  cumulative schedule lag, and MUST NOT report the configured rate as if achieved.
- **A key drawn at two different sizes** — a consumer is entitled to treat a size disagreement
  however it likes, and at least one treats it as a miss, so a plan that contains one has
  manufactured an outcome rather than described a workload. Size MUST be a pure function of key
  identity, and the plan generator MUST assert this invariant.
- **Zero-length request** (`shared_depth` and `private_depth` both sampling 0) — clamp the
  total path to 1 block and count the clamping.
- **A finite key space** — a corpus that mints no keys below the trunk
  (`sessions.private_depth` fixed at 0 with `roots.count` and `shared_depth` also fixed) never
  exercises eviction, so the run is degenerate. Detect and report rather than run.
- **A session outrunning the trunk** — *impossible by construction*, and deliberately so:
  every `branching` segment's fanout is ≥ 1, so every trunk node has a child and the trunk is
  unbounded in
  depth. Path depth is `shared_depth` *plus* a non-negative private part (FR-014a), so the
  trunk can never exceed the path either. There is no clamp here and no counter for one.
- **`fit` against a trace with one root** — a legitimate outcome (`roots.count: 1` is a global
  system prompt), not an error, but it must be reported so it is not mistaken for a fitting
  failure.
- **A workload whose live key space is almost entirely never re-read** — the consumer's cache
  does no useful work whatever its policy. The generator MUST report this from the plan (the
  compulsory-miss floor of FR-060 approaches 1.0) rather than leaving it to be discovered as a
  flat result, because it is knowable before anything runs.
- **`fit` against a trace lacking multi-turn structure** — the fitted `turns` distribution
  collapses to a point mass at 1, so the mixture degenerates to one-shot; report that rather
  than emitting the degenerate model silently.
- **A warmup shorter than the session-population ramp-up** — the measured window opens while
  sessions are still accumulating, so concurrency, trunk occupancy and realised sharing are all
  below what the model specifies, and every one of them looks like a property of the workload
  rather than of the clock. Rejected per FR-015b rather than warned about, because the resulting
  numbers are wrong rather than merely noisy.
- **`churn.half_life` short relative to depth** — because a path survives only while all `d+1` of
  its nodes do, the effective lifetime of a deep path is `half_life/(d+1)`. A half-life that looks
  generous against `shared_depth` p50 can be shorter than the accumulation time at p99, so deep
  sharing silently fails while shallow sharing is fine. Caught by the churn-adjusted occupancy
  floor (FR-016e) rather than left to be discovered in the hit rate.
- **`churn.half_life` set on a plan with no `duration`** — churn advances with elapsed plan time, so
  a plan specified purely as a request count has no clock for a half-life to be relative to.
  Rejected rather than defaulted.
- **A run long enough for real content to have churned, with churn left at 0** — the default is an
  immortal trunk, whose error grows with run length rather than staying constant (FR-016b). Not
  detectable from within the run, so the report must state which case it is.
- **Cold RDMA connections on the first measured operation** — a stale warm-connect
  first-write has previously cost ~15 s. The runner MUST have an explicit connection-warm
  phase outside the measured window.

## Requirements *(mandatory)*

### Input format and schema

- **FR-001**: The generator MUST accept a single YAML document as its complete workload
  description, validated against the schema in `contracts/workload-schema.md`.
- **FR-002**: The schema MUST be factored into four orthogonal sections — `corpus`
  (what keys exist and how they overlap), `workload` (who asks for what, when), `topology`
  (which node asks for what), and `run` (execution and measurement) — such that changing one
  axis does not require edits to another. There MUST NOT be a section describing the system
  under test (FR-018a).
- **FR-003**: Every scalar drawn from a distribution MUST use one uniform tagged-union
  syntax, `{dist: <shape>, ...params}`, supporting at minimum `const`, `uniform`, `normal`,
  `lognormal`, `exponential`, `geometric`, `zipf`, `pareto`, and `empirical` (explicit CDF
  points).
- **FR-004**: The schema MUST support `extends: <path>` with deep merge, so that a common
  experiment is expressible in under ten lines against a checked-in preset.
- **FR-005**: The generator MUST reject unknown fields rather than ignoring them, so a typo
  in a distribution parameter cannot silently fall back to a default.
- **FR-006**: The schema MUST carry an explicit `version` field, and the generator MUST
  refuse inputs whose version it does not implement.
- **FR-007**: The schema MUST separate inter-session sharing (`corpus.trees`) from
  intra-session sharing and private path length (`workload.sessions`) and MUST NOT permit
  either to be specified in more than one place, so that `corpus` describes the shared key
  structure and `workload` describes how a session traverses it. The generator MUST reject a
  top-level `gets_per_key` or `lifetime`, a `depth` field anywhere, and any `corpus` field
  restated inside `workload.mix` — `popularity` and `shared_depth` in particular — because a
  mixture entry varies how a session behaves, never what is shared.

### Corpus and prefix structure

- **FR-008**: The generator MUST model the shared-prefix structure of KV keys as a trie
  whose node identity is derived by `child_id = H(parent_id, child_index, generation(node))`, so
  that requests sharing a prefix emit identical leading `CacheKey` values. `generation` is fixed at
  0 unless `corpus.trees.churn` is configured (FR-016b), so the default reduces exactly to
  `H(parent_id, child_index)` and the key space remains a pure function of the path.
- **FR-009**: The generator MUST model the key space as a **forest** of configurably many
  roots, and MUST express sharing as a monotone prefix property: a per-session `shared_depth`
  giving the depth at which a session *attempts* to leave the shared trunk, a `branching`
  profile giving the **mean** number of trunk children per trunk node **as a function of depth**,
  a `branch_skew` concentrating descents among those children as a Zipf exponent over child rank,
  and a `private_depth` walked below the trunk on a branch private to the session. The generator
  MUST NOT offer a per-depth **sharing** table, because a rolling-hash key makes divergence
  irreversible and such a table would expose unrealisable configurations. A per-depth **fanout**
  profile is a different thing and is required (FR-009e): what it varies is how many children a
  node has, not whether a pair of sessions can resume sharing after diverging.
- **FR-009e**: `branching` MUST be a piecewise profile over depth — a list of
  `{from_depth, fanout}` segments, each fanout in force until the next segment — with a bare
  scalar accepted as sugar for one segment at depth 0. Each fanout MUST be a scalar mean rather
  than a distribution, and a non-integer mean MUST be realised exactly by giving each node
  `floor(m)` or `ceil(m)` children with the probabilities satisfying `E[children] = m`: child
  counts are integers, so the mean is the only load-bearing moment, and a median-and-sigma
  parameterisation would let the realised mean drift from the stated value.
- **FR-009e1**: A profile is required rather than a single exponent because **measured tries are
  flat for long stretches and then fan out at particular depths**. Agentic tool-use traces examined
  during design held width *exactly* constant across runs of 20-40 consecutive depths, fanned out at
  more than one depth, and did so as deep as depth 124 after that many levels of shared path. A
  uniform fanout of even 1.05 would widen by 7× across 40 levels, so a scalar does not approximate
  this shape — it describes a different one. The profile MUST therefore be
  able to express a global preamble, a fanout, a **second shared segment** carrying per-branch
  commonality such as a tool definition or retrieved document, a further fanout, and only then the
  private tail. The generator MUST document that the profile is global, so the trie is
  self-similar and fanout depths cannot differ between branches.
- **FR-009b**: A trunk node's children MUST be determined once, as a pure function of that
  node's identity and the seed, and MUST NOT depend on arrival order. A Chinese-restaurant
  rule that mints children as sessions arrive is prohibited: it would make `corpus` depend on
  `workload` in violation of FR-002, and would make a key's identity uncomputable without
  replaying all prior sessions, defeating FR-008 and FR-010. Depth-indexing the fanout preserves
  this: depth is a property of the node, so a child count still depends only on node identity and
  seed. Every segment's `fanout` MUST be at least 1, so the trunk is unbounded in depth and no
  session can run off the end of it.
- **FR-009c**: Descents below `shared_depth` MUST draw from a child namespace disjoint from
  the trunk's — `child_id = H(parent_id, PRIVATE_TAG, session_id, i)` — so that two sessions
  can never collide on a private node. Without this, private branches would be only
  probabilistically private and the FR-007 separation would leak undetectably.
- **FR-009a**: Each session MUST bind to exactly one root, drawn once at session creation
  from `roots.popularity` and fixed for the session's lifetime, so that many sessions map to
  one root and every turn of a session starts from the same key.
- **FR-009f**: The generator MUST compute **trunk occupancy**,
  `occupancy(d) = sessions_per_window / paths(d)` where
  `paths(d) = roots.count × Π fanout(k) for k in 1..d` taken from the `branching` profile — which
  reduces to `roots.count × branch_factor^d` for a uniform profile — and where
  `sessions_per_window` counts sessions begun within one `run.wss_window` of requests. It MUST
  reject a
  configuration whose `occupancy(p99(shared_depth))` is below 1.0 and MUST warn below 4.0. A
  drawn `shared_depth` is only an **upper bound** on realised sharing — a session sharing *s*
  levels requires that some earlier session walked the same *s* steps — and occupancy is what
  decides whether the bound is tight. The window is part of the definition, not a refinement:
  counted over a whole run, a configuration could "achieve" sharing merely by running longer,
  which is not a physical effect.
- **FR-009g**: `branching: auto` MUST be supported and MUST be the recommended default, resolving
  to a **uniform** profile by the closed form
  `(sessions_per_window / roots.count / target_occupancy) ^ (1 / p99(shared_depth))` with
  `target_occupancy = 4`. It MUST be a direct solve, never an iterative calibration against
  generated output, so that no part of this model requires a nonlinear fit. The resolved profile
  MUST appear in the normalised YAML. When `shared_depth` is a swept axis the generator MUST warn
  that `auto` re-solves per sweep point, varying the trunk shape along with the swept axis. `auto`
  resolves to a uniform profile deliberately: a non-uniform profile encodes a claim about where
  branches diverge, which the generator has no basis to invent and which MUST come either from the
  user or from `fit`.
- **FR-009g1**: `target_occupancy = 4` is a **judgement, consistent with what has been observed**.
  In the traces examined during design, occupancy below the fanout points settled in the low single
  digits and held there across hundreds of depths. That is just below the chosen target, which is the
  correct side for a floor to design against, but the sample was small and incidental and the value
  MUST NOT be presented as measured. It is a design floor, not an estimate of any population.
- **FR-009h**: `run.wss_window` — the window for both trunk occupancy and the reported
  working-set size — MUST be defined canonically as a **request count**, not a wall-clock
  span. A duration MAY be accepted as sugar and converted via the configured rate, and a duration
  combined with `closed_loop` MUST be rejected because only `open_loop` supplies a rate. A time
  window would be unknowable at plan time under `closed_loop`, where arrivals depend on system
  response and `t_ns` is advisory ordering only, and would drift under `open_loop` whenever the
  schedule slips (FR-061). A count makes both quantities a property of the plan rather than of
  how fast the system happened to run it.
- **FR-010**: The generator MUST NOT materialise the trie; resident memory MUST be O(active
  paths), independent of the number of distinct keys minted over a run. "Active paths" is the
  **live-session population** of FR-015a — sessions born but not yet retired — so the bound holds
  for a run of any length only because sessions retire (FR-014b). The generator MUST report the
  realised live-session count against this bound, since an unbounded run makes the difference
  between "O(live sessions)" and "O(keys minted)" the difference between running and not.
- **FR-011**: Entry size MUST be a pure, deterministic function of key identity.
- **FR-011a**: Entry size is a **chosen parameter, not a fitted one**. It does not affect the
  generated reference pattern — it affects only when a consumer's storage fills — so no trace
  needs to supply it and the trace formats this tool reads cannot: their `block_size` counts
  *tokens*, and converting to KV bytes would need a model's layer count, KV head count, head
  dimension and dtype width, for which every manifest's `model_config` is null. The generator MUST
  therefore take `corpus.block_bytes` as an input, MUST record the value used in every report, and
  MUST NOT attempt to derive it from a trace or silently default it when fitting.
- **FR-011b**: Entry **payload** MUST be the 8-byte key followed by zero padding to the entry's
  size. The value's content is irrelevant to every measurement this feature makes, and this
  construction costs nothing while making a returned value **self-identifying**: a consumer that
  returns the wrong value for a key — as against the wrong size, which FR-039b already covers — is
  detectable by comparing the first eight bytes. The runner SHOULD therefore offer an opt-in
  payload check, and MUST fill its device buffer once at startup rather than per entry, since
  FR-030 already mandates a single process-wide allocation. The report MUST note that padding is
  zeros, so that no future bandwidth or capacity figure measured through a compressing path is
  mistaken for a realistic one.
- **FR-012**: The generator MUST report, for any plan, a summary of realised corpus
  properties: distinct keys, total bytes, working-set size over a configurable window, a
  prefix-sharing depth histogram, and — because trunk width is emergent (`w(0) = roots.count`,
  `w(d+1) = w(d) × fanout(d+1)`) rather than configured — the realised **trunk width per
  depth** and **trunk occupancy per depth**, so that the shape actually generated is recoverable
  from the report.
- **FR-012a**: The report MUST state the **intended** `shared_depth` distribution and the
  **realised** prefix-sharing depth histogram as two separate statistics, and MUST NOT present
  the configured value as if it were the measured one. Where they diverge, the divergence is the
  finding; FR-057 already fails a fitted model on per-statistic divergence, and this is the same
  comparison applied to a hand-written one.

### Workload model

- **FR-013**: The generator MUST support a weighted mixture of session parameter sets in one
  workload, with weights normalised rather than required to sum to 1. Each mixture entry
  overrides fields of the one session model; entries MUST NOT be distinct behavioural modes,
  so that there is exactly one sampling path and one fitting routine.
- **FR-014**: The generator MUST implement a single session model — a sticky root binding, a
  `turns` count, a `think_time` between turns, a `private_depth` turn-1 private path, and a
  `growth_per_turn` path extension where turn N+1 re-reads turn N's blocks — and MUST ship
  `conversation`, `one_shot`, and `scan` as **presets** over it (`turns: geometric(6)`;
  `turns: 1`; `turns: 1` with a large `private_depth`). There MUST be no `archetype` field in
  the schema.
- **FR-014a**: Path depth MUST be stated exactly once, by
  `depth(turn N) = shared_depth + private_depth + Σ(i = 2..N) growth_per_turn(i)`, where
  `private_depth` is the turn-1 private path and `growth_per_turn` is drawn per turn. Turn N's
  path MUST be a strict prefix of turn N+1's, which the rolling-hash key requires: a changed
  prefix would rehash every block below it.
- **FR-014b**: A session MUST have an explicit lifecycle: it is **born** when it arrives, binds
  its root once (FR-009a), issues its `turns` requests separated by `think_time`, and is
  **retired** when its last turn completes. Retirement MUST release the session's state, and a
  retired session MUST never be revisited — its private keys are dead from that moment, which
  FR-009c already guarantees by making private namespaces disjoint per session. Session lifetime is
  therefore **derived, never configured**: `lifetime = Σ(i = 2..turns) think_time(i)`. There MUST NOT
  be a `lifetime` field, for the same reason there is no `gets_per_key`: turns and think_time already
  determine it, and a third statement could disagree with them.
- **FR-014c**: The generator MUST run for an arbitrary duration by **continuously retiring and
  creating sessions**, so that neither the plan length nor resident memory is bounded by the
  session model. Unbounded novelty comes from private branches: each new session mints keys under
  `H(parent_id, PRIVATE_TAG, session_id, i)`, so the distinct-key count grows without bound as
  sessions are created, while the *shared* key space stays bounded by construction unless
  `corpus.trees.churn` is configured (FR-016b), which is what lets shared content turn over on a
  long run rather than living forever.
- **FR-015**: The generator MUST support both `open_loop` arrival (absolute timestamps from
  a configurable rate and burstiness) and `closed_loop` arrival (bounded concurrent
  sessions), with `open_loop` the default.
- **FR-015a**: The **live-session population** — sessions born but not yet retired — MUST be
  computed and reported, and MUST NOT be a schema field under `open_loop`. Under `open_loop` it is
  already determined by Little's law:

  ```
  session_rate    = arrival.rate / mean(turns)
  mean_lifetime   = (mean(turns) - 1) * mean(think_time)
  live_sessions   = session_rate * mean_lifetime
  ```

  so stating it as well would over-determine the model exactly as design rule 3 forbids; for the
  worked example it is ~667 sessions/s × 15 s ≈ **10 000 live sessions**. Under `closed_loop` it is
  `arrival.concurrency` directly, which is legitimate there because `closed_loop` supplies no rate.
  It MUST be reported because two separate requirements depend on it and neither is otherwise
  checkable: it is the constant in FR-010's `O(active paths)` bound, and it is what FR-015b
  compares warmup against. Note it is **not** the same quantity as FR-009f's
  `sessions_per_window`, which counts sessions *begun* in a window; occupancy needs how many have
  walked the trunk, memory needs how many are walking it now.
- **FR-015b**: `run.warmup` MUST be at least the time for the live-session population to reach
  steady state — on the order of `mean_lifetime`, and the generator MUST compute it and **reject a
  configuration whose warmup is shorter**, rather than reporting ramp-up as though it were steady
  state. At t=0 no session is live and the population fills over roughly one session lifetime, so a
  measurement window opening before that sees fewer concurrent sessions, less trunk occupancy, and
  therefore less sharing than the model specifies. The failure is quiet and scales with the session
  model: the worked example's 15 s mean lifetime is covered by its 20 s warmup, but
  `turns: geometric(50)` with `think_time` median 30 s implies a ~24 minute ramp, against which a
  20 s warmup measures nothing but the transient.
- **FR-016**: The generator MUST support non-stationary **root** popularity via a configurable
  drift half-life, where 0 means stationary.
- **FR-016a**: `drift` MUST re-weight only **which** shared keys are popular, never **which shared
  keys exist**. Changing what exists is `churn`'s job (FR-016b), and the two MUST remain separate
  parameters: a popularity shift leaves a cache's contents valid and merely changes what is asked
  for next, whereas a content replacement invalidates entries outright. Collapsing them would make
  a single half-life mean two physically different things.
- **FR-016b**: The generator MUST support **corpus churn** — replacement of shared content over
  time — via `corpus.trees.churn.half_life`, defaulting to 0 meaning no churn. It MUST be realised
  by the `generation` term of FR-008, advancing per node on that half-life and drawn from the node's
  own identity and the seed. Because the key is a rolling hash, rotating a node MUST invalidate its
  entire subtree implicitly, which is what lets one parameter express whole-tree replacement (the
  root rotates), per-branch content replacement (a mid-trunk node rotates), and everything between.
  A `branching` segment MAY override the half-life, so that "prompts stable for weeks, retrieved
  documents turning over daily" is statable.
- **FR-016c**: Churn MUST remain a function of the seed and elapsed plan time only, and MUST NOT
  depend on which sessions are live. Reference counting shared nodes and retiring a node when its
  last user leaves is specifically **prohibited**, for three reasons: node identity is a pure
  function of the path, so a retired node is re-derived identically by the next session to walk the
  same indices and no novelty is produced; refcount-zero is unreachable near the root and constant
  deep down (~833 live sessions per root against ~1.1 per path at depth 40 for the worked example),
  so it would churn the nearly-private deep nodes and never the popular shallow ones, which is the
  opposite of how content lifecycle behaves; and node existence would become a function of arrival
  timing, so changing the request rate would change the key space, breaking FR-002 orthogonality
  exactly as a Chinese-restaurant minting rule would (FR-009b). Reference counting remains the
  correct model where the reader owns the content, and that case is already covered by FR-009c and
  FR-014b, which make a session's private keys dead at retirement with no count needed.
- **FR-016d**: The generator MUST report **rotation events** and the **compulsory-miss shock** each
  causes: at the instant a shared node rotates, every session that would have hit its old key misses
  simultaneously and must re-populate. This is the measurable consequence of churn and the reason to
  model it — a redeployed system prompt invalidating the most-shared prefix in the system is a sharp
  transient of exactly the kind that distinguishes replacement policies. The compulsory-miss floor
  of FR-060 MUST account for churn-induced misses, which remains computable from the plan alone
  because rotation is deterministic.
- **FR-016e**: When `churn.half_life` is set, the occupancy of FR-009f MUST be computed over the
  **churn-adjusted window** — sessions arriving within `path_lifetime(d) = half_life / (d + 1)`
  rather than within the whole `wss_window`, whichever is shorter. A path accumulates sharers only
  while it exists, so without this term the occupancy floor would approve a configuration whose
  sharing churn then destroys. The interaction bites hardest where sharing is deepest, since
  `path_lifetime` falls as `1/(d+1)`: a half-life generous at depth 4 can be far too short at
  depth 40.
- **FR-017**: Burstiness MUST be expressible as an index of dispersion where 1.0 reproduces
  a Poisson process, so that the parameter has a defined neutral value.

### Topology and placement

- **FR-018**: The generator MUST express cross-node behaviour as properties of the *request
  streams* only: `self_affinity` (probability that the node asking for a key is one that asked
  for it before, i.e. how far the per-node streams overlap), `replication.nodes_per_key` (how
  many distinct nodes ask for the same key), and `cold_fraction` (keys the warmup phase
  deliberately does not pre-request). It MUST NOT express where any copy of a key resides.
- **FR-018a**: The generator MUST NOT contain any concept of a tier, cache, memory, or disk.
  Capacities, eviction policies, watermarks, pinning, and placement of copies are properties of
  the consumer of a workload, not of the workload, and MUST be absent from the schema, the plan
  artifact, and every report the generator produces. A workload MUST mean exactly the same thing
  whether its consumer has two tiers, five, one, or none.
- **FR-018b**: Multi-node support MUST remain, because an inference deployment runs in parallel
  across nodes and the request streams that arrive at each node are a genuine property of the
  workload. It MUST be expressed without reference to any specific system: the generator states
  which node asks for which key at which time, and nothing about what any node stores.
- **FR-019**: Request placement MUST be uniform across nodes by default, and the schema MUST
  NOT provide dedicated requester or holder node roles. Every node both requests and, so far as
  the workload is concerned, is indistinguishable from every other.
- **FR-020**: The generator MUST NOT drive any tier-placement or cache-control operation — in
  particular not `FlushToSsd` or `ClearMemoryTier` — during plan setup or at any other time. A
  run that wants to exercise a slower medium arrives there by asking for more distinct bytes
  than the faster one holds, which is a statement about the workload, and reads the resulting
  split out of the consumer's own reporting.
- **FR-021**: The generator MUST support scheduled membership events (`stop`, `start`) at
  absolute plan times.

### Plan artifact and determinism

- **FR-021a**: The generator MUST support three **output modes**, exactly one of which involves
  Certus: (1) drive a Certus server directly, presenting the generated keys as lookups and storing
  what missed; (2) write a `.jsonl` trace file; (3) write a parquet trace file. Modes 2 and 3 MUST
  emit the schema of `contracts/trace-io.md` — the same schema `fit` reads from real traces — so
  that a synthetic workload is substitutable for a real one as input to any third-party tool, and
  so that the generator's output can be checked against its input in one comparison. Neither file
  mode may require Certus to be built, running, or present.
- **FR-021b**: Modes 2 and 3 MUST use the contract's **full** block encoding, MUST populate
  `partial_final_valid`, and MUST write a `manifest.json` declaring `source_class: pre_hashed`,
  `id_semantics: rolling_prefix`, `provenance: synthetic`, and `timestamp_is_synthetic: true`. The
  full encoding is the honest one for generated data, since the generator knows every block ID it
  minted and has no reason to express reuse as a delta against ancestors.
- **FR-021c**: `events.bin` (`contracts/plan-format.md`) MUST remain the native artifact. It is
  fixed-width, indexable by ordinal, and streamable, none of which the interchange formats are; FR-037's
  allocation-free requirement depends on it. Modes 2 and 3 are interchange, not a replacement, and
  the generator MUST be able to produce them from an existing `events.bin` without regenerating.
- **FR-022**: The generator MUST emit the event plan as a first-class, persistable artifact
  distinct from execution, so that the identical stream can be replayed against any consumer —
  the hardware runner, an emitted trace file, or a tool this feature never anticipated.
- **FR-023**: A plan MUST contain **requests** — an ordered list of `(key, size)` per request
  with an absolute timestamp and an owning node — and MUST NOT contain populate operations.
  The executor derives populates from observed misses.
- **FR-023a**: Every plan event MUST carry its owning `session_id`, its 1-based `turn` index,
  and the `mix_index` of the `workload.mix` entry the session was drawn from. Session identity
  MUST be stored rather than derived: turns are separated by `think_time`, so a session's
  requests are not contiguous in the plan and cannot be recovered by grouping on `request_id`.
  `mix_index` replaces the former per-archetype tag, preserving per-class reporting while the
  schema itself has no `archetype` field (FR-014).
- **FR-023b**: The plan record MUST have no length prefix and therefore MUST be versioned by
  `plan_format` in the manifest; any change to a field's presence, order, or width MUST bump it,
  and a reader MUST refuse a `plan_format` it does not implement. Reserved bytes MUST be zero on
  write and MUST be rejected if non-zero on read, so that a later additive change moves no
  existing field.
- **FR-024**: Plan generation MUST be fully determined by the YAML plus its `seed`; the same
  input MUST produce a byte-identical plan on the same build.
- **FR-025**: Each per-repeat and per-sweep-point seed MUST be derived deterministically
  from the root seed, so that a whole sweep is reproducible from one number.
- **FR-026**: A plan MUST carry a content hash and the full normalised YAML that produced
  it, so that any report can be traced to its exact input.
- **FR-027**: The plan MUST be partitionable by node such that each node loads only its own
  slice, and each node MUST verify the plan's content hash before executing.
- **FR-028**: The plan format MUST be compact enough that 10^7 events is a routine artifact,
  and MUST be streamable rather than requiring the whole plan resident.
- **FR-029**: The generator MUST optionally emit a human-readable trace of the plan for
  debugging, and this MUST never be required as an input.

### Executors

- **FR-030**: The hardware runner MUST issue lookups through the existing batched gRPC
  surface, using one process-wide CUDA allocation addressed per entry via `IpcHandle.offset`,
  with configurable `batch_size`, `workers`, and `inflight`.
- **FR-031**: The hardware runner MUST NOT perform a host/device copy on the measured path.
- **FR-032**: The hardware runner MUST populate on miss, using the existing
  `Reserve`/`CopyToStore`/`CommitStore` or `Populate` path, and MUST account populate cost
  separately from lookup cost.
- **FR-033**: The hardware runner MUST have an explicit connection-warm phase, enabled by
  default, completing before the measured window opens.
- **FR-034**: The generator MUST NOT evaluate replacement decisions, grade a replacement policy,
  or report a hit rate at a capacity. **Cache simulation is deferred out of this feature**
  (see Out of Scope), and the generator takes no dependency, build-time or runtime, on
  `IEvictionPolicy` or on any policy component. What it publishes instead are the capacity-free
  statistics of FR-034a, from which a consumer derives whatever its own capacity would buy.
- **FR-034a**: The generator MUST report, from the plan alone: the **reuse-distance CDF** (per
  object and per byte), the **compulsory-miss floor** (FR-060), the prefix-sharing depth
  histogram, the request-length distribution, unique keys over time, realised trunk occupancy,
  and the realised working-set size over `run.wss_window`. Every one of these is a property of
  the reference stream and requires no capacity parameter and no cache model. The reuse-distance
  CDF is primary: it encodes the achievable hit-rate curve, so a consumer can read off any
  capacity point without this tool modelling a cache to tell it.
- **FR-034b**: The generator MUST NOT report a **Belady/OPT** figure. An earlier draft treated
  OPT as a capacity-free workload statistic on the grounds that it needs no `IEvictionPolicy`
  implementation; that is true but insufficient, because OPT evicts furthest-next-use *when
  full* and its hit rate is therefore a function of capacity. It is a curve over a quantity the
  generator does not know, so it defers with the rest of cache simulation. The compulsory-miss
  floor is the part that survives, being the miss rate at unbounded capacity. Should OPT return
  with the simulator, the earlier caveat still applies and MUST be restated then: Belady is
  exact only for uniform entry size in a single tier, since with heterogeneous sizes offline
  optimal caching is NP-hard and furthest-next-use bounds neither optimality nor byte hit rate.
- **FR-035**: Any offline replay of a plan MUST document explicitly which effects it does not
  model — at minimum fabric transfer and connection setup, control-plane quorum timing, device
  queue depth and per-device bandwidth, DMA bandwidth and interconnect topology, lock
  contention, and the cost of absent keys — because a trace of block references reproduces the
  reference pattern exactly and reproduces nothing about time. The generator itself MUST NOT
  model a cache in order to make this statement: the structure of the consumer's storage is the
  consumer's to describe (FR-018a).
- **FR-036**: Every consumer of a plan that this feature ships MUST emit a stream digest over
  the key sequence it consumed, and the plan MUST carry the digest of the sequence it encodes,
  so that any two arms — whatever they are, and whoever runs them — can be proven to have seen
  the identical stream. This is the generator's whole contribution to a comparison's validity,
  and it is what makes an externally-run comparison as trustworthy as one run here.
- **FR-037**: The generator MUST NOT be the bottleneck: plan events MUST be pre-generated
  into a flat, allocation-free representation, and event generation MUST NOT run on the
  cores issuing requests.
- **FR-038**: The runner MUST measure and report its own overhead, and MUST flag any run
  where harness overhead could account for more than 5% of the measured figure.

### Metrics and reporting

- **FR-039**: **In the runner only**, and only when the consumer is a Certus server, every
  lookup outcome MUST be classified into exactly one of the seven values defined by
  `components/dispatcher/specs/002-served-by-tier-attribution/contracts/served-by.md` —
  `DRAM`, `SSD`, `REMOTE_DRAM`, `REMOTE_SSD`, `MISS`, `SIZE_MISMATCH`, `ERROR` — **taken
  verbatim from the server's `served_by` field**. There MUST NOT be an "unknown" class. The
  first four MUST be counted as hits and the last three MUST NOT, and `hits + misses + errors`
  MUST equal entries requested for every batch.
- **FR-039d**: This taxonomy is the *server's*, relayed. The runner MUST NOT derive, infer, or
  predict a class from anything in the plan, from timing, or from any model of its own, and the
  taxonomy MUST NOT appear in the schema, in the plan artifact, or in any output produced
  without a server (FR-018a). Per-tier hit and miss ratios for a workload are Certus's to report
  — that reporting is the point of the `served_by` dependency — and the runner's role is to
  drive the requests, aggregate what comes back, and add the one thing only a client knows,
  which is client-observed latency. A report produced from any other consumer MUST simply not
  have these columns, rather than filling them in with guesses.
- **FR-039a**: The runner MUST treat `SERVED_BY_UNSPECIFIED` as "server does not support
  attribution" and MUST refuse to emit tiered hit rates for that run, rather than mapping the
  zero value onto any tier or into an unknown bucket.
- **FR-039b**: `SIZE_MISMATCH` MUST be reported as its own class and MUST NOT be folded into
  `MISS`. Because size is a pure function of key identity (FR-011), a non-zero
  `SIZE_MISMATCH` count indicates a generator or plan defect, and the report MUST flag it as
  such rather than absorbing it into the miss rate.
- **FR-039c**: Remote hits MUST be reported split by first-touch versus repeat touch of the
  same key, because `REMOTE_SSD` is a first-touch property that decays to `REMOTE_DRAM` on
  reuse.
- **FR-040**: The report MUST present both object hit rate and byte hit rate, per tier and
  in aggregate. Where `corpus.block_bytes` is a constant — the default, and the physically correct
  choice for a fixed KV block size — byte hit rate is object hit rate multiplied by that constant
  and carries **no independent information**. The report MUST say so rather than presenting two
  numbers as two findings. Byte hit rate becomes independently informative only under size
  heterogeneity, which is exactly why the size-heterogeneity row of the Test Matrix exists.
- **FR-041**: Latency percentiles MUST be reported per outcome class as well as in
  aggregate, at minimum p50, p90, p99, and p99.9.
- **FR-042**: The report MUST include throughput in both GB/s and keys/s, and MUST report
  bytes delivered over the fabric separately from bytes read off local disk.
- **FR-042a**: The `GetIoStats` cross-check of local-disk bytes MUST be performed when the
  server exposes non-zero counters, and MUST be reported as `unavailable` with its reason
  otherwise. The runner MUST distinguish "counters absent because `rw-telemetry` did not reach
  the active dispatcher" from "counters present and reading zero", and MUST NOT report the
  former as agreement.
- **FR-042b**: The cross-check's tolerance MUST be derived rather than assumed, and the
  derivation MUST be documented in `research.md`. `GetIoStats` is aggregated across all data
  drives and includes background traffic — staging writes and promotion reads — that no lookup
  in the plan requested, so the runner MUST subtract or bound that background component and
  state which it did. A tolerance asserted without this derivation would fail for reasons
  unrelated to attribution correctness.
- **FR-043**: The report MUST include eviction churn (evictions per unit time, and
  demote-versus-remove split from `TakeEvents`) and MUST report `dropped_count` when the
  event channel overflowed rather than silently under-reporting.
- **FR-044**: The report MUST include the wasted-populate ratio — entries populated and
  never subsequently read.
- **FR-044a**: Hit rate MUST be reported broken down by `mix_index` and by `turn`, in addition
  to in aggregate. Turn 1 and turn *N* are qualitatively different cache events — turn 1 walks a
  trunk another session may have warmed, whereas turn *N* re-reads blocks this session just
  caused to be written — so an aggregate over both conceals the intra- versus inter-session
  distinction the corpus model is built around, and a mixture sweep's crossover point is not
  interpretable without the per-entry split.
- **FR-045**: Warmup operations MUST be excluded from steady-state statistics and reported
  separately.
- **FR-046**: Sweep reports MUST include per-point n, mean, cv, and confidence interval, and
  a pairwise significance verdict at p < 0.05; `repeat` MUST default to 8.
- **FR-047**: The report MUST embed the plan content hash, the normalised input YAML, the
  symmetry certificate, and the software versions of every node, so a result is fully
  attributable.
- **FR-048**: The report MUST emit a machine-readable form (JSON) alongside the human
  summary.

### Preflight and symmetry

- **FR-049**: `preflight` MUST verify, across all participating nodes, at minimum: CPU model
  and core count; NUMA topology; that each node's NIC, GPU, and NVMe devices sit on the same
  NUMA node as each other; that this socket assignment is identical across nodes; NIC model,
  link layer, and active port speed; GPU model and count; NVMe model and count; hugepage
  capacity; `memlock` limit; and Certus build identity.
- **FR-050**: `preflight` MUST verify inter-node clock offset is within a configurable bound
  and MUST fail when it is not.
- **FR-051**: `preflight` MUST fail on any asymmetry, naming the node, the attribute, and
  both differing values.
- **FR-052**: A successful `preflight` MUST emit a symmetry certificate that the runner
  embeds in every report produced against that cluster.
- **FR-053**: `--allow-asymmetric` MUST permit the run but MUST mark every resulting report
  `NON-COMPARABLE` with the specific reason, and this marking MUST NOT be suppressible.
- **FR-054**: The runner MUST establish a start barrier across nodes so that all nodes share
  one plan time origin.

### Fitting and validation

- **FR-055**: `fit` MUST accept the trace format of `contracts/trace-io.md` in **either container**,
  parquet or JSONL, and with **either block-encoding population pattern**, detected per trace rather
  than assumed, and MUST emit a schema-valid YAML. The two population patterns are not two schemas —
  every column exists in every file and only the populated subset differs — so this is one reader
  with a branch, and it SHOULD normalise to full ordered block lists at ingest so the branch does not
  leak into each statistic. Container support MUST be symmetric with FR-021a's output modes:
  the generator emits JSONL, so refusing to read JSONL would leave its own output unconsumable by its
  own tools and would make the FR-058a round trip impossible.
- **FR-055e**: `fit` MUST **refuse to fit from a partial trace**, and MUST determine partiality by
  comparing the records it consumed against the manifest's declared
  `block_stats.<block_size>.invocations` rather than by a filename convention. The
  `sample_block_size_<N>.jsonl` files shipped beside real traces are eyeball samples — measured at 6
  records against 1 960 074, and 136 against 2 115 623 — and every parameter in the model would fit
  "successfully" against six requests while meaning nothing. `validate` MAY proceed on a partial
  trace but MUST label every statistic as computed from a sample of stated size. It MUST consult
  the manifest's `field_status` and refuse to fit a parameter whose source field is `unavailable`
  rather than producing a default: a trace with a null `session_id` cannot supply `turns`,
  `growth_per_turn`, or the FR-009a root binding, and a `metadata_only` trace cannot supply anything
  structural at all. It MUST also report which fitted values came from `reconstructed` rather than
  `native` fields, and MUST mark statistics from a trace without timestamps as order-dependent
  rather than measured.
- **FR-055d**: `fit` MUST leave `churn.half_life` **unset** rather than estimating it, and MUST say
  so in the fit report. Churn's observable signature is a trunk key used and then never used again,
  but available traces span hours at most while plausible content cadences run to days or weeks, and
  a half-life longer than the observation window is indistinguishable from no churn.
  Any fitted value would therefore be an artifact of trace length, biased **short** — the direction
  that manufactures misses. `fit` MAY report a lower bound ("no trunk rotation observed within the
  trace's N-hour span, so `half_life` ≫ N"). Setting churn MUST remain a deliberate act by whoever
  knows the deployment's content cadence.
- **FR-055c**: `fit` MUST choose the **root boundary** at the depth below the last *near-root*
  fanout event, report `roots.count` as the width at that depth, and treat the levels above it as a
  global preamble prepended to every session. The boundary depth MUST appear in the fit report,
  because it changes what `roots.count` means. Taking depth 0 literally would report
  `roots.count: 1` for a trace whose every request shares one preamble, and then have to express the
  fanout immediately below — observed at up to four orders of magnitude in a single level — as trunk
  branching, which fails the FR-009f occupancy floor at any useful depth. This rule applies only to
  near-root fanout: a fanout deep in the trunk, which has been observed beyond depth 100, is a
  genuine `branching` segment and MUST NOT be absorbed into `roots.count`, because no choice of root
  boundary can reach it.
- **FR-055a**: `fit` MUST emit the **measured** `branching` profile rather than `auto`, because trunk
  structure is a physical property of the trace. It MUST also record the value `auto` would have
  chosen, and MUST **fail** per FR-057 — never silently substitute — when the measured combination
  of `roots.count`, `shared_depth`, and the `branching` profile violates the FR-009f occupancy floor,
  since that combination is one the generator cannot realise.
- **FR-055b**: `fit` MUST report that a measured fanout is only trustworthy in the
  high-occupancy region. A trace reveals only *visited* nodes: where many sessions traverse each
  trunk path most children are observed and the measured width ratio approaches the true value,
  but where occupancy is low each session sits alone on its own path and the
  measured ratio collapses toward **1 regardless of the true value**. The fit report MUST state
  the realised occupancy at which each width ratio was measured so a value near 1 is not mistaken
  for a genuinely linear trunk.
- **FR-056**: `fit` MUST validate the fitted model by comparing four statistics between the
  real trace and synthetic output: reuse-distance CDF (primary), prefix-sharing depth
  histogram, request-length distribution, and unique-keys-over-time curve.
- **FR-057**: `fit` MUST report per-statistic divergence and MUST fail rather than emit a
  model whose divergence exceeds its tolerance.
- **FR-057a**: Validation tolerances MUST be **per-statistic**, one per FR-056 statistic, and
  MUST NOT be expressed as a single scalar: the four statistics are on four different scales, so
  one threshold across all of them has no consistent meaning. The reuse-distance CDF remains
  primary.
- **FR-057b**: Tolerances MUST be supplied as `fit`/`validate` command-line options with
  documented defaults, and MUST NOT appear in the workload YAML. Fitting is an operation
  performed *on* a workload model, not a property *of* one; placing a tool parameter in the YAML
  would breach FR-002's section factoring and would make two models with identical workload
  content compare unequal. Defaults MUST be derived in `research.md` rather than asserted, and
  the tolerances actually used MUST be recorded in the validation report so a pass is
  reproducible.
- **FR-058**: The generator MUST provide a `validate` mode that runs FR-056's comparison
  between any two plans or between a plan and a trace.
- **FR-058a**: The tool MUST support a **round trip** as a self-test: generate a plan from a YAML,
  emit it as a trace file (FR-021a mode 2 or 3), re-run `fit` against that file, and compare the
  recovered parameters against the original YAML. This is the strongest available check on `fit`,
  because the ground truth is known exactly rather than estimated — any divergence is a defect in
  `fit`, in the emitter, or in the reader, and not a property of some real workload. It also
  exercises the emitter and the reader against each other, which no other test does. Divergence MUST
  be reported per parameter against the FR-057a tolerances.

### Warnings that protect the measurement

- **FR-059**: The tool MUST warn when the consumer reports steady-state evictions at ~zero (the
  working set fits, so the consumer's policy is untested). This warning depends on the consumer's
  own reporting; the generator cannot raise it from the plan, having no knowledge of capacity.
- **FR-060**: The tool MUST warn when hit rate is within noise of the compulsory-miss floor
  (working set too large, so all policies look alike).
- **FR-061**: The tool MUST report cumulative open-loop schedule lag and MUST NOT report a
  configured offered rate as achieved when the schedule slipped.
- **FR-062**: The tool MUST warn when a hit-rate comparison is attempted between arms whose
  stream digests differ.

### Key Entities

- **WorkloadModel** — the parsed, normalised, validated YAML. Five sections (`corpus`,
  `workload`, `topology`, `system`, `run`) plus `version`, `seed`, and an optional `sweep`.
  Reuse is split across exactly two of them: `corpus.trees` (the shared structure) and
  `workload.sessions` (intra-session reuse and the private path).
- **PrefixForest** — the lazily-evaluated key-identity space: `roots.count` root keys, each
  the origin of a trie defined by `child_id = H(parent_id, child_index)`. A node's child count
  is a pure function of its own identity and the seed, never of arrival order (FR-009b), and
  private descents use a disjoint namespace (FR-009c). Not stored; resident state is the active
  paths only.
- **Session** — the only behavioural unit. A root binding drawn once at creation and fixed, a
  `turns` count, a `think_time`, a `private_depth` turn-1 private path, and a
  `growth_per_turn` per-turn extension; its `shared_depth` (in `corpus`) is where it leaves the
  shared trunk. Mixture entries are parameter sets over this one entity, not subtypes of it.
- **Request** — one plan event: absolute timestamp, owning node, request id, owning session id,
  1-based turn index, the `mix_index` it was drawn from, and an ordered list of `(key, size)`.
- **Plan** — the ordered, content-hashed sequence of Requests, plus the normalised YAML and
  a per-node partitioning. The unit of reproducibility.
- **Outcome** — the classification of a single lookup: exactly one of `DRAM`, `SSD`,
  `REMOTE_DRAM`, `REMOTE_SSD`, `MISS`, `SIZE_MISMATCH`, `ERROR`, plus latency, byte count, and
  a first-touch flag (needed because `REMOTE_SSD` is a first-touch property). The four
  remote/local hit values are hits; the other three are not. Defined normatively by
  `components/dispatcher/specs/002-served-by-tier-attribution/contracts/served-by.md`;
  this spec does not maintain a second definition.
- **SymmetryCertificate** — the preflight result: per-node attribute inventory plus a verdict,
  embedded in reports.
- **Report** — outcomes aggregated per FR-039..FR-048, in human and JSON form, carrying the
  plan hash, input YAML, and symmetry certificate.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A workload exhibiting realistic multi-level prefix sharing, drawn from a real
  ShareGPT trace, is expressible in **under 60 lines of YAML**, and a common variation on it
  in **under 10 lines** using `extends`.
- **SC-002**: A plan generated from a fitted YAML matches the source trace's reuse-distance
  CDF within that statistic's tolerance — supplied on the `fit`/`validate` command line, per
  FR-057b, and recorded in the validation report — so that LRU hit rate agrees at every capacity
  under test.
- **SC-003**: The same YAML and seed produce a byte-identical plan across repeated runs, and
  two arms of a policy comparison produce identical stream digests.
- **SC-004**: `report` computes every FR-034a statistic over a 10^7-request plan in under one
  minute on a single core, so that characterising a workload is never the reason not to.
- **SC-005**: The reuse-distance CDF reported for a pure-Zipf plan matches the analytic Zipf
  reuse-distance distribution within the FR-057a tolerance, validating the generator end-to-end
  against a closed form — a check on the stream itself rather than on a model of a cache
  consuming it.
- **SC-006**: The measured remote-served fraction tracks configured `self_affinity` across a
  0.0→1.0 sweep, and at 1.0 fabric bytes in the measured window are ~0.
- **SC-007**: Every lookup in every **runner** report — that is, every report produced against a
  Certus server — is attributed to exactly one of the seven `served_by` values, with no unknown
  bucket, and for every batch `hits + misses + errors == entries requested`. This is the
  attribution criterion and it depends only on the `served_by` field the server returned.
  Reports produced without a server carry no attribution columns at all and are not held to this
  criterion: the generator has no tiers to attribute to (FR-018a, FR-039d).
- **SC-007a**: When the server exposes non-zero `GetIoStats` counters, reported SSD-served
  bytes agree with the `GetIoStats` read-byte delta within the tolerance derived per FR-042b,
  after accounting for drive aggregation and background promotion traffic. When the counters
  are unavailable, every report states the cross-check was not performed. **No report ever
  presents an unavailable cross-check as a passing one.** SC-007a is deliberately separate
  from SC-007: the cross-check is corroborating evidence for byte provenance, not the
  definition of correct attribution, and gating attribution on a Cargo feature that
  `p2p-native` builds cannot currently set would make SC-007 unmeetable for the very
  configuration US3 and US4 run.
- **SC-008**: Harness overhead is demonstrated to be under 5% of the measured figure at the
  platform's throughput ceiling, so reported numbers describe Certus rather than the
  generator.
- **SC-009**: Preflight detects a known NUMA asymmetry (NIC and GPU on different sockets) and
  fails with the node and attribute named, and no report from an asymmetric cluster is
  emitted without the `NON-COMPARABLE` marking.
- **SC-010**: A sweep of **any two arms** at `repeat: 8` yields a significance verdict, and the
  harness refuses to present a comparison between arms whose stream digests differ. The criterion
  is that the comparison machinery is sound and the streams are provably identical; what the arms
  are is deliberately not this tool's business, because it describes workloads and is not
  specified in terms of any consumer's implementation.
- **SC-010a**: Every plan report states the compulsory-miss floor and the reuse-distance CDF, so
  that a consumer's single measured number is interpretable on its own — the floor says what the
  best imaginable consumer could do on this workload, and the CDF says what capacity would buy.
  Neither requires this tool to model a cache.
- **SC-011**: A single YAML expresses the full test matrix in the Test Matrix section below,
  and each case is runnable without editing code.

## Test Matrix

The cases the generator exists to produce. The "what it discriminates" column records *why* each
shape is worth producing — that is, what a consumer could learn by running it. It is motivation,
not a claim that the generator measures any of it.

### Workload shapes that discriminate between replacement policies

| Case | Configuration shape | What it discriminates |
| --- | --- | --- |
| **Zipf baseline** | flat key space: `shared_depth: 0`, `private_depth: 1`, Zipf over many roots | Harness validation — hit rate is analytic, so it catches generator bugs |
| **Pure conversational** | `turns` geometric at weight 1.0 | Recency-friendly; LRU near-optimal |
| **Pure shared-preamble** | `turns: 1`, `roots.count: 1`, high `shared_depth` | Frequency-friendly; LRU underperforms. Hot root blocks must never be evicted |
| **Forest width sweep** | `roots.count` 1→1000 at fixed `shared_depth` | How sharing breadth (many tenants/templates) trades against sharing depth — a case the superseded single-root model could not state |
| **Sharing depth sweep** | sweep the `shared_depth` median, `branching` **pinned** | Where the trunk ends — the only sharing quantity a *session* freely chooses. Pinning is required: `auto` would re-solve per point and vary the trunk shape along with the swept axis |
| **Per-branch commonality** | `branching` with a fanout, a flat segment, then a second fanout — the shape agentic tool-use traces show | A global preamble shared by everything, then a tool definition or retrieved document shared only within a branch. Two sessions on one branch share far more than two on different branches, which no single `shared_depth` can express on its own |
| **Trunk width sweep** | sweep a uniform `branching` fanout 1.0→1.25 at fixed `roots.count` | Width *at depth*, as against width at the root: a linear preamble shared by everyone (1.0) versus a trunk fanning out fast enough that occupancy falls and sharing decays. The range is narrow because the occupancy floor (FR-009f) is what bounds it |
| **Mixture sweep** | sweep the geometric-`turns` weight against the `turns: 1` weight | **The headline experiment.** Real workloads are a mixture; the crossover point says whether an adaptive policy (ARC/LIRS/S3-FIFO) is worth building |
| **Scan resistance** | 5% at `turns: 1, private_depth: 4000` over a hot conversational set | Classic LRU-killer, with `benchmarks/long-doc-qa` as the real analogue |
| **Working-set sweep** | scale the distinct-key count via `roots.count` and `private_depth` at a fixed consumer configuration | The same curve a capacity sweep produces, obtained by moving the workload instead of the cache. The generator publishes the realised working-set size, so the x-axis is still a ratio; a consumer that prefers to hold the workload fixed and sweep its own capacity does so from its own command line, which is where capacity lives |
| **Churn / abandonment** | a mixture entry whose keys are never re-read, swept in weight 0→0.5 | Wasted-write cost; can a policy spot dead-on-arrival entries. Sweeping the weight also subsumes the pin-pressure case the earlier draft listed, since both vary how much of the live key space is dead weight — with the difference that this is a workload property, whereas what a consumer pins is its own decision |
| **Size heterogeneity** | `block_bytes` lognormal | Object hit rate vs byte hit rate diverge; evicting one large vs many small differs |
| **Root popularity drift** | `drift.half_life` finite | Static LFU collapses; adaptive policies do not |
| **Shared-content churn** | `corpus.trees.churn.half_life` swept from ∞ down toward the session lifetime | How a policy handles shared entries becoming *invalid* rather than merely unpopular. The two are physically different and this is the row that separates them: drift changes what is asked for next, churn invalidates what is already held |
| **Prefix-rotation shock** | a `churn_half_life` on the depth-0 segment only, long relative to the run | The redeployed-system-prompt event: one rotation invalidates the most-shared prefix in the system and every live session misses at once. A transient, not a steady state, so it is read from the miss timeline rather than from a run aggregate |

### Remote-lookup behaviour (hardware, symmetric cluster required)

| Case | Configuration shape | What it measures |
| --- | --- | --- |
| **Affinity sweep** | `self_affinity` 0.0→1.0 | The remote-hit fraction directly; the single most useful remote knob |
| **Replication sweep** | `nodes_per_key` 1→N | How the number of nodes that ask for the same key affects a consumer's lookup path — for Certus, whether a wider quorum is reached sooner at the cost of loading more responders |
| **Cold storm** | `cold_fraction` 0.05→0.30 | The cost of keys nothing has seen before. A consumer may spend its full lookup deadline on each, so a high-`cold_fraction` workload can be *entirely* deadline-dominated — the case most likely to surprise |
| **Thundering herd** | many nodes, one absent-then-arriving key | remote-lookup's single-flight dedup: distinct fetches issued per key |
| **Node hotspot** | `roots.popularity` skewed so one node's stream carries the popular roots | Saturation of whichever node the consumer ends up serving those keys from |
| **Membership churn** | `membership_events` mid-run | Graceful degradation vs a hit-rate cliff |

## Assumptions

- **The test cluster is symmetric.** All participating nodes have the same CPU model and
  core count, the same NIC model and active port speed, the same GPU model and count, the
  same NVMe model and count, the same hugepage capacity and `memlock` limit, and the same
  Certus build. Critically, on every node the NIC, GPU, and NVMe devices are attached to the
  **same NUMA socket as each other**, and that socket assignment is the **same on every
  node**. This is stipulated because real cross-node KV copies are essentially symmetrical,
  so the generator models a symmetric deployment rather than compensating for a lab
  artifact. Preflight enforces it (FR-049..FR-053).
  - *Known non-conforming hardware*: a previously measured node in this lab had its NIC on
    socket 0 and its GPU on socket 1, producing cv 16% versus 2% on a conforming node. Such
    a node must be re-seated, re-bound, or excluded from comparative runs — it is a preflight
    failure, not something the generator works around.
- Inter-node clocks are synchronised within the configured bound, and a start barrier
  establishes a shared plan time origin.
- `CacheKey` remains an opaque `u64` and the vLLM-side key remains a rolling hash over the
  block chain, so prefix sharing continues to manifest as shared leading key sequences — and,
  because divergence under a rolling hash is irreversible, continues to be a monotone prefix
  property. **If the key ever stopped being a rolling hash over the chain, FR-009's
  single-`shared_depth` model would become an under-specification rather than an exact fit.**
- Size mismatch continues to be treated as a cache miss by the dispatcher, which is why size
  must be a pure function of key identity (FR-011).
- The dispatcher already knows internally which tier resolved a lookup, so exposing
  `served_by` is largely a plumbing change rather than new bookkeeping — **but not purely
  so**: the remote values require an `IRemoteLookup::batch_lookup` signature change to carry
  the peer's advertised tier, and two paths need deliberate handling rather than plumbing
  (a single-flight follower owns no landing slot and must take its tier from the leading
  operation; an `AlreadyExists` publish path has a recorded peer that did not fill DRAM and
  must not be read as an advertisement). This spec assumes feature 002 resolves those; it does
  not assume they are free.
- **Remote tier is the peer's advertisement, not serve-time ground truth.** No wire-protocol
  change is available to obtain the latter (`WIRE_VERSION` stays at 1; the codec frames by
  record count with no length prefix, so appending a field would mis-align an old decoder
  silently). Reports must not claim to measure where a peer actually read from.
- **`GetIoStats` is corroboration, not ground truth.** Its counters are zeroed unless the
  active dispatcher was built with `rw-telemetry`, they are aggregated across all data drives,
  and they include background staging and promotion traffic. Every use of them in this spec is
  conditional and tolerance-derived (FR-042a, FR-042b).
- The mandatory `CERTUS_PROFILE=full-remote` build requirement for multi-node remote runs
  continues to apply.
- Trace fitting targets vLLM-shaped KV workloads. Other workload families would need
  additional mixture entries — new parameter sets over the one session model — which the
  mixture design accommodates without schema change.

## Out of Scope

- **Cache simulation of any kind — deferred, not rejected.** No modelled memory tier, no
  modelled disk tier, no replacement-policy grading, no hit rate at a capacity. This feature is
  synthetic workload generation and nothing else. Three reasons, in increasing order of how hard
  they are to design around:
  1. A workload must not be specified in terms of any consumer's internals (FR-018a), and a
     simulator is a consumer.
  2. To be *realistic* a simulator would have to share the cache-replacement code with Certus,
     and that code is still evolving. The component design may well make the sharing mechanically
     easy — one interface, bound at runtime — but easy coupling is still coupling: this tool would
     then track a moving target, and a workload generator that breaks when a policy changes has
     the dependency pointing the wrong way.
  3. **The disk tier has nowhere to live except real disks.** Device queueing, per-drive
     bandwidth, and write amplification are the substance of what an SSD tier *is*, and a
     discrete-event approximation of them produces numbers whose error is unknown and unbounded —
     which is worse than no number, because it looks like a measurement.

  If it is wanted later it can be brought back, and there is a running start: `tools/simulator/`
  already models the two-tier server in SimPy and already replays a block trace. Nothing in this
  feature forecloses it — a plan is a plain reference trace, so a simulator is just another
  consumer of one, and FR-036's digests let its results be compared against hardware honestly.
- **Implementing new replacement policies.** Each policy is a separate component behind
  `IEvictionPolicy`, and grading them is now a consumer's concern rather than this feature's.
- **Generating model activations or real KV tensor content.** Payloads are arbitrary bytes of
  the correct size; nothing in Certus interprets them.
- **Driving a real inference engine.** That is `benchmarks/kv-offload-replay`'s job. This
  tool talks to the dispatcher directly so the measurement is not mediated by vLLM.
- **Replacing `apps/remote-lookup-bench`.** Expressing its `lookup` subcommand as a workload
  YAML and retiring the overlap is a follow-up.
- **Multi-tenancy, authentication, or fairness modelling.**
- **Cross-subnet / gossip-discovery cluster topologies.** Supported by remote-lookup via
  `CERTUS_RL_GOSSIP_*` but not modelled here.
- **Automatic parameter search** (e.g. finding the workload that maximises a policy's
  advantage). The sweep mechanism is declarative only.

## Next Artifacts

Following the speckit flow, still to be written for this feature:

- `plan.md` — implementation approach, crate layout, and the phased build order implied by
  the P1/P2/P3 story priorities.
- `data-model.md` — concrete Rust representations of the Key Entities, especially the plan
  event encoding.
- `contracts/workload-schema.md` — **written** (normative YAML schema reference).
- `contracts/plan-format.md` — **written** (plan artifact encoding and hashing).
- `contracts/trace-io.md` — **written** (trace interchange schema, both block encodings with their
  verified invariants, and the mode-2/3 output formats).
- `research.md` — **partially written.** The trace measurements are complete: the two block
  encodings with their exhaustively verified invariants, the shape taxonomy in tokens, the
  width-and-occupancy-by-depth profiles behind FR-009e1 and FR-055c, cross-session sharing, the role
  distribution, fan-in, and a threats-to-validity section recording that the traces were a
  convenience sample not checked into this repository. Still **open** there, and enumerated in its
  own § Open derivations: the full derivation of the trunk-occupancy bound and the `auto` closed
  form (FR-009f/FR-009g) including the `target_occupancy = 4` choice, which the measurements support
  but do not establish (FR-009g1); **the segmentation rule for fitting a `branching` profile** — what
  jump ratio counts as a fanout event, how to choose boundaries when width is noisy, and how the
  near-root boundary of FR-055c interacts with it; **the four default per-statistic `fit`/`validate`
  tolerances** (FR-057b) and which divergence measure each statistic uses, the four being on
  different scales; the `branch_skew` parameterisation and the fitting procedures for `shared_depth`
  and `roots.popularity`; reuse-distance estimation method; the significance-testing approach behind
  `repeat: 8`; and **the `GetIoStats` cross-check tolerance** (FR-042b) — how background staging and
  promotion traffic is bounded or subtracted out of a drive-aggregated counter. An **LP/flow
  relaxation** for a true offline upper bound under heterogeneous entry sizes is parked rather than
  open: it existed only to make Belady/OPT a sound ceiling, and OPT defers with cache simulation
  (FR-034b).
- `quickstart.md` — the shortest path from a checked-in preset to a report.
- `tasks.md` — task breakdown.
