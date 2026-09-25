# Draft input for `/speckit-specify`

Everything below the rule is the feature description passed to
`apps/workload-generator:speckit-specify`. It is **kept in step with
`spec.md`** rather than frozen at the moment it was first written: when a
decision changes, this file is revised with it, so re-running the skill cannot
regenerate a spec that describes a different feature. `spec.md` is the
normative form — this is its input, not a second specification.

**Mechanical note (important):** the skill otherwise derives its own short
name, creates `specs/<NNN>-<short-name>/`, and overwrites
`.specify/feature.json`. The first line pins the existing directory so it
reuses ours and leaves `contracts/workload-input.example.yml` in place.

---

SPECIFY_FEATURE_DIRECTORY=specs/001-synthetic-workload-generator

Build a synthetic workload generator for Certus: a measurement instrument that
drives realistic LLM KV-cache traffic into one or more Certus nodes, and can
also emit the same workload to a file instead of a server. Its purpose is
performance testing and, specifically, evaluating Certus's cache eviction
policy — so the credibility of its numbers is the product, not a side effect.

The reviewed input schema is already written and is normative for this feature:
read `contracts/workload-input.example.yml` in this feature directory. Its
comments carry the semantics, and requirements below should reference it rather
than restate it.

## What it is

Two programs plus shared framing:

1. **The generator.** Reads a workload YAML file, runs a virtual-time
   simulation to produce a totally ordered plan of cache operations, and either
   issues that plan into Certus or writes it to a file.
2. **A per-node daemon.** Certus's only ingress is a host-local `/dev/shm`
   shared memory queue, so remote nodes cannot be driven directly. A small
   resident daemon on each node accepts work from the generator and pushes it
   into that node's local queue. Only keys cross the network — the daemon
   reconstructs block payloads from the key — so the transport carries roughly
   8 bytes per key.

## Primary user scenarios

- **P1 — Drive the local node.** A user with a running Certus server on this
  host points the generator at its shm mailbox and a workload file, and gets
  sustained, realistic traffic plus a run report. This must work with no daemon
  and no remote nodes configured.
- **P2 — Drive a multi-node cluster.** The user supplies a list of `(node,
  shm_name)` targets on the command line; sessions are placed across nodes and
  migrate between them, so session prefixes go cold on their new node and the
  remote-lookup path is exercised.
- **P3 — Emit a trace file.** The same workload is written to a file instead of
  being issued, in the trace format of whichever tool will read it, so a
  generated workload is an input to third-party analysis without that tool
  needing to learn anything new.

## Requirements that are already settled

**Cache hits come from exactly two sources**, driven by different parameters so
they can be swept independently: intra-session reuse from multi-turn growth,
and inter-session reuse from common *prefixes*. A session's own growth blocks
chain onto its unique prefix, so they are private by construction and can only
ever be reused intra-session.

**Keys are prefix-chained.** A block's key depends on the block and everything
ahead of it in the chain, so sharing requires a matching leading run, and an
object held in common at a different position produces no hit. Ordering is
therefore semantic: shared-object classes are concatenated in the order the
session class lists them, and within a class the chosen instances are sorted by
index. Sorting makes a session's prefix a pure function of the set it chose, so
sessions drawing overlapping sets produce nested rather than divergent chains.

**Turns are append-only and cost grows linearly with turn index.** Turn *n*
reads its whole prefix — shared objects plus every input and output block
minted by turns 1..n-1 — and mints new input and output blocks. Input and
output growth are separate distributions; both are stored and both extend the
chain.

**Population pools.** Each shared-object class and each session class has a
target population, expressed as `{poisson: N}` (default; free-running births at
rate N/E[lifetime], live count Poisson(N), so cv = 1/sqrt(N)) or `{exact: N}`
(exactly N alive, each death replaced immediately, variance 0). Pools are
seeded from the equilibrium residual-life distribution, never from the lifetime
distribution itself, which would synchronise the pool into cohorts — measured
to leave a 40% peak-to-trough churn modulation still present 80,000 virtual
seconds in. No feedback controller and no gain parameter: an integral
controller was designed, simulated, and rejected on measurement (8-23% positive
population bias from the non-negative creation rate, plus a spectral
resonance).

> **Correction, recorded after this prompt was written.** The parenthesis above
> is wrong, and is left in place because this file is the provenance record of
> what `spec.md` was generated from rather than a live document. The controller
> regulates the mean to under 1%, and the non-negative constraint never binds
at > the parameters where it was checked; the 8-23% came from a badly-scaled >
parameter sweep. FR-016 survives on the variance argument instead — a >
regulator holds `var/mean` at 0.48 where a real population sits at 1.00. See >
`research/population/README.md`.

**Instance selection is non-uniform.** Each class may carry a `selection`
distribution over the instance index, so its spread sets the *working set* size
independently of the pool's *key-space* size. `rank_by: slot` attaches
popularity to a position that new instances inherit; `rank_by: recency`
attaches it to newness so heat decays with age. Both must exist, because that
is the axis on which recency- and frequency-based eviction policies disagree.
Uniform selection is the default but makes working set equal key space and
leaves every policy scoring the same, so the effective working-set size is
reported at load time.

**Draws are without replacement within a class**, and the implied maximum for a
count draw is `min(nominal pool size, live count)`, so a session can never ask
for more instances than exist and no runtime wait or failure path is needed.

**Virtual time never touches wallclock.** Virtual time exists only to decide
the order of operations. The virtual clock is the minimum timestamp among
operations still in flight; when Certus applies backpressure a lane blocks and
the clock holds. Think time is consumed entirely at plan time, where it decides
interleaving, and must not reappear at execution time as idle Certus. For a
fixed input file and seed, the emitted plan is byte-identical regardless of how
fast Certus runs.

**Per-session ordering is the only hard constraint.** Cross-session operations
may overlap freely, including two sessions racing to mint the same shared
prefix and both storing it — which is what production vLLM does. Consequently
hit/miss outcomes are not bit-reproducible even though the plan is, and A/B
comparisons need repetitions with a stated significance test.

**The op stream must match vLLM's connector exactly** — no more and no less.
`Reserve` -> `CopyToStore` -> `CommitStore`/`AbortStore` for stores; never
`Populate`. `Touch` with `promote: 0`. Poll `TakeEvents`. Never `Remove`,
`Pin`, `Unpin`, or `promote: 1`; at most one `ClearMemoryTier` at startup. The
generator must not help the eviction policy, and must not starve it of
information the real client would supply.

**Distribution contract.** `constant`, `uniform`, `empirical`, `normal`,
`exponential`, all continuous, with optional `min`/`max` truncating by inverse
transform between F(min) and F(max) — never clamping. Integer-valued draws
truncate on [min-0.5, max+0.5] and round to nearest so every integer in range
carries equal weight. `empirical` accepts inline samples or a file, and may be
discrete (`interpolate: false`) for block counts. `inf` is legal wherever a
number is. Where truncation changes a distribution's effective mean, that is
reported at load time, and a configuration discarding more than 5% of a
distribution's mass is refused.

**The generator must never be the bottleneck, and this is asserted per run.**
The plan is produced ahead of the lanes consuming it; if the plan queue drains,
the run is invalid and reported as invalid rather than published. Every run
report carries plan-queue occupancy and lane utilisation next to its throughput
number. Block payloads are pre-filled, reused device buffers with at most a
small key stamp — never bytes constructed per operation.

**CLI, not the workload file:** node list and per-node shm name, run length,
output format and destination, lane count, seed. A workload file describes a
workload and nothing else, so one file is portable across clusters. Generation
is unbounded by default when driving Certus; writing a file requires an
explicit length.

**Node placement.** New sessions are placed uniformly across the configured
nodes; a migrating session moves to a node chosen uniformly among the others.
Its blocks stay where they were, so the new node must fetch them remotely. Keys
are globally consistent across nodes, which is what makes a remote hit
possible. With no node list, migration is inert.

**Two distinct file artifacts, at different levels.** These must not be
conflated:

- The **workload trace** is the user-facing output, at the same level of
  abstraction as a real LLM serving trace: sessions, turns, block lists, token
  counts. It is delivered as a **projection** onto a published format that a
  real consumer already reads, and System defines no interchange format of its
  own: a workload is repeated from its description and seed, so storing one is
  never the way to repeat it, and a format nothing outside this repository
  reads would earn none of the cost of specifying, versioning and importing it.
  Each record carries the complete ordered block lists rather than a delta
  against the previous turn, records the trailing-partial-block convention
  explicitly in `partial_final_valid`, and declares its timestamps as virtual
  seconds on a run-global clock, marked synthetic. Block identifiers are the
  chained u64 keys directly and are **not** dense integers in mint order:
  nothing depends on the density, and mint order remains recoverable from
  `invocation_index` together with
  `new_input_blocks`/`new_output_blocks`, so no information is lost. Where a
  target format assumes density, the projection onto it MUST either renumber
  across the whole output or declare the divergence as a loss, rather than
  leave it as a silent deviation. One consequence to watch: the block lists
  repeat the whole prefix on every turn, so they grow quadratically in turns
  and random u64s compress far worse than dense small integers. If file size
  becomes a problem, attack the repetition itself; do not reintroduce a
  mint-order mapping to shave bytes per element.
- The **operation plan** is the lower-level ordered sequence of cache
  operations with virtual timestamps that the lanes consume. It needs a
  canonical serialisation because the byte-identical-plan property is asserted
  against it, but it is a diagnostic artifact rather than a published format.

**Daemon lifecycle is per run.** The daemon holds no persistent state and never
receives block data, so nothing justifies outliving a run: it is started before
a run and stopped after, and its startup and teardown lie outside the timed
window. Three requirements follow from recorded failures in earlier work on
this hardware. A build-identity handshake: the generator refuses to run against
a daemon whose build differs from its own, because a stale remote binary has
previously invalidated measurements. Idempotent startup: a leftover daemon from
a crashed run must be detected and replaced, never silently reused. Verified
teardown, including on generator crash, releasing GPU buffers and shmq channels
— a harness teardown bug that failed to release resources previously
invalidated an entire A/B series, and the failure mode was nondeterministic
rather than obvious. Note that only *launching* the daemon goes over ssh; load
is driven over the fast transport, because an ssh-batch harness cannot sustain
continual load.

## Success criteria

- P1 works against a live local server with no daemon configured.
- Sustained throughput is reported together with the validity evidence, and a
  run whose plan queue drained is reported as invalid.
- The canonical operation-plan serialisation is byte-identical across runs at a
  fixed seed, and across fast and artificially slowed servers.
- Every projection an emit run requests receives a record for each turn
  simulated, and satisfies that target format's documented invariants on every
  record.
- A hit-rate-versus-cache-size sweep produces a smooth concave curve rather
  than a step, and `rank_by: slot` versus `rank_by: recency` produce measurably
  different policy rankings — the evidence that the reuse structure is real.
- The shipped example YAML parses, validates, and runs.

## Out of scope

- Fitting a workload YAML from real trace corpora. Deliberately deferred.
- An open-loop paced mode with a virtual-to-wallclock rate and lateness
  reporting. Elastic virtual time cannot produce a saturation or
  latency-versus-offered-load curve, and that limitation is accepted for now.
- Non-Linux, non-x86-64 platforms.
- Modelling fan-in: a session's chain is a path, not a DAG.
- A `--deterministic-mint` mode. Faithful racing is the only behaviour. The
  variance the racing actually introduces should be measured before any
  mechanism is built to remove it, because the wire protocol's `PENDING` state
  already exists for store dedup, which bounds the effect: a second session
  arriving mid-store sees `PENDING` rather than `MISS` and can skip its own
  store. So what varies run to run is the miss/pending/resident split, not
  necessarily duplicated work.

## Open questions

None. Every design decision above was settled in review before this
specification was generated. Do not emit `[NEEDS CLARIFICATION]` markers for
items listed as settled or out of scope; if something genuinely underspecified
surfaces, mark it, but expect zero.
