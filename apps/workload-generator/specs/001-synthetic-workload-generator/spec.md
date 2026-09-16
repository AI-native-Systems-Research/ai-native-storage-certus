# Feature Specification: Synthetic Workload Generator

**Feature Directory**: `specs/001-synthetic-workload-generator`

**Git Branch**: `synthetic-workload-generator-rewrite` (independent of the
feature directory name)

**Created**: 2026-09-15

**Status**: Draft

**Input**: `specify-prompt.md` in this directory, which is the reviewed feature
description. The workload input schema in
`contracts/workload-input.example.yml` is normative.

## Overview

A measurement instrument that drives realistic LLM KV-cache traffic into one or
more Certus nodes, and can emit the same workload to a file instead of a
server. Its purpose is performance testing and, specifically, evaluating
Certus's cache eviction policy — so the credibility of its numbers is the
product, not a side effect.

Two programs plus shared framing: a **generator** that reads a workload
description, simulates it in virtual time, and issues or records the resulting
operations; and a **per-node daemon** that exists because Certus's only ingress
is a host-local shared-memory queue, so remote nodes cannot be driven directly.
Only keys cross the network — the daemon reconstructs block payloads from the
key — so the transport carries roughly 8 bytes per key rather than block data.

## Clarifications

### Session 2026-09-15

- Q: What scale must a single generator instance sustain, as a design target? →
  A: ~10K concurrent sessions, ~10M live keys, runs of hours
- Q: When a node or its daemon becomes unreachable mid-run, what should happen?
  → A: Abort immediately, report the run invalid, name the node
- Q: What form must the run report take? → A: Both — human-readable summary to
  the terminal plus a structured file per run
- Q: Is throughput measured against wallclock time? → A: Yes; virtual time
  orders operations, wallclock measures execution, and their ratio is itself a
  reported metric
- Q: What was "lookahead occupancy"? → A: Renamed to plan-queue depth: how many
  operations are built and waiting; reaching zero invalidates the run
- Q: How is the number of keys per request determined? → A: A command-line
  option with a documented default, sweepable independently of the workload
- Q: What latency measurement must the run report provide? → A: Per-request
  latency into a histogram, reporting p50/p90/p99/max
- Q: Do the throughput, latency, and plan-queue measurements apply when writing
  to a file? → A: No. They are scoped to live runs; an emit run reports
  completeness instead, and omits the fields it cannot measure
- Q: How is output length specified, and should there be several stop criteria?
  → A: One control only — a virtual-second span, required for emit and optional
  for live. Record/key/session caps and a wallclock cap were considered and
  rejected; a pre-flight projection with a free-space check does the work they
  were meant to do

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Drive a local Certus node (Priority: P1)

A performance engineer with a running Certus server on the same host points the
generator at that server's mailbox and a workload description file, and gets
sustained, realistic cache traffic plus a run report that says both what the
throughput was and whether the run was valid.

**Why this priority**: This is the tool's primary purpose and the smallest
configuration that delivers it. It requires no daemon, no remote nodes, and no
network, so it is both the MVP and the substrate every other story builds on.

**Independent Test**: Start a Certus server locally, run the generator against
the shipped example workload, and confirm sustained traffic plus a run report
carrying throughput, plan-queue depth, and lane utilisation. Delivers value on
its own: a repeatable load for any single-node experiment.

**Acceptance Scenarios**:

1. **Given** a running local Certus server and a valid workload file, **When**
   the engineer runs the generator with no node list, **Then** operations are
   issued continuously until stopped, and a run report is produced.
2. **Given** a run in progress, **When** Certus applies backpressure, **Then**
   the generator blocks rather than dropping or reordering work, virtual time
   stops advancing for the duration of the stall, and the workload's shape is
   unchanged.
3. **Given** a workload file whose configuration is contradictory, **When** the
   engineer runs the generator, **Then** it refuses to start and names the
   offending parameter rather than silently reshaping it.
4. **Given** a completed run in which the generator could not keep the lanes
   fed, **When** the report is produced, **Then** the run is reported as
   **invalid** and its throughput number is not presented as a result.

---

### User Story 2 - Emit a workload trace to a file (Priority: P2)

The same engineer writes a workload to a file instead of issuing it, in either
of two containers, so that a generated workload and a real serving trace are
interchangeable inputs to third-party analysis.

**Why this priority**: Independently valuable and independently testable — it
needs no server, no accelerator, and no cluster, so it is the cheapest way to
inspect and share a workload. It also exercises the whole simulation core,
which makes it the natural place to pin determinism.

**Independent Test**: Emit the shipped example workload to both containers with
a fixed seed and no server present; confirm the two contain identical records,
that every row satisfies the trace schema's own invariants, and that repeating
the run reproduces the output byte for byte.

**Acceptance Scenarios**:

1. **Given** a workload file and an explicit run length, **When** the engineer
   requests file output, **Then** a self-describing trace is written whose
   manifest declares its own encoding and identifier conventions.
2. **Given** the same workload file, seed, and length, **When** the run is
   repeated, **Then** the output is byte-identical.
3. **Given** a request for file output with no run length, **When** the
   generator starts, **Then** it refuses and explains that file output must be
   bounded.
4. **Given** an emit run, **When** its report is produced, **Then** it states
   completeness — sessions, turns, blocks, virtual-time span, records written —
   and omits latency, lane utilisation, and the virtual-to-wallclock ratio
   entirely rather than reporting them as zero.
5. **Given** an emitted trace, **When** it is converted to a third-party tool's
   format, **Then** the conversion preserves cross-session key reuse and the
   run-global clock, and names whatever the target cannot carry.

---

### User Story 3 - Drive a multi-node cluster (Priority: P3)

The engineer supplies several node targets. Sessions are placed across nodes
and migrate between them during their lifetime, so a migrated session's prefix
is cold on its new node and must be fetched from the node that holds it.

**Why this priority**: This is what exercises the remote path, but it depends
on the daemon and its transport, so it is the largest increment and the last to
land. Single-node work is unaffected by its absence.

**Independent Test**: With two or more nodes configured, run a workload whose
migration interval is short relative to session lifetime, and confirm that
migrated sessions produce remote fetches on their new node while local hit rate
on the origin node is unchanged.

**Acceptance Scenarios**:

1. **Given** a node list, **When** sessions are created, **Then** they are
   distributed uniformly across the configured nodes.
2. **Given** a session on one node, **When** its migration interval elapses,
   **Then** its subsequent turns are issued to a different node chosen
   uniformly among the others, while its previously stored blocks stay where
   they were.
3. **Given** a node whose daemon was built from different sources than the
   generator, **When** the run starts, **Then** the generator refuses to
   proceed rather than producing measurements against a mismatched peer.
4. **Given** a daemon left behind by a crashed run, **When** a new run starts,
   **Then** the leftover is detected and replaced rather than silently reused.

---

### User Story 4 - Compare eviction policies (Priority: P4)

The engineer runs the same workload against two Certus configurations that
differ only in eviction policy, and gets hit-rate curves that actually
discriminate between them.

**Why this priority**: It is the reason the tool exists, but it is a *use* of
the earlier stories rather than new machinery — it needs only that the
popularity and reuse structure be real. It is listed separately because it is
the acceptance test for that structure.

**Independent Test**: Sweep cache size against hit rate for one workload under
two policies, and separately under both instance-ranking modes; confirm the
curves are smooth and concave rather than step-shaped, and that the two ranking
modes reorder the policies.

**Acceptance Scenarios**:

1. **Given** a workload with a concentrated working set, **When** cache size is
   swept, **Then** hit rate rises smoothly and concavely rather than jumping at
   a single threshold.
2. **Given** the same workload under position-based and recency-based
   popularity, **When** two eviction policies are compared, **Then** the
   measured ranking of those policies differs between the two modes.

---

### Edge Cases

- **The generator cannot keep up.** The plan queue drains. The run is invalid
  and must be reported as such; a throughput number without its validity
  evidence is not a result.
- **Requested concurrency exceeds the server's capacity for it.** Detected at
  startup and refused or reported, never silently serialised.
- **A session asks for more shared instances than currently exist.** Under a
  fluctuating population the count draw is bounded by the live count, so the
  request always succeeds; how often that bound binds is reported, because it
  silently narrows the requested distribution.
- **A count distribution cannot fit its pool.** If the implied maximum discards
  more than 5% of the distribution's mass, the configuration is refused with
  the effective and requested means both named.
- **A class has many instances and no popularity distribution.** Selection is
  uniform, working set equals key space, and every eviction policy scores the
  same. Permitted, but the implied working-set size is reported so it cannot
  pass unnoticed.
- **A class has an unbounded lifetime.** Birth rate derived from mean lifetime
  is undefined, so such a class is minted once and never turns over.
- **Two sessions race to mint the same shared prefix.** Both may miss and both
  may store; the protocol's in-flight state means the later one may instead see
  a store in progress. This is faithful to production and is preserved, so
  hit/miss outcomes vary run to run even at a fixed seed.
- **Migration is configured but only one node is.** Migration is inert, not an
  error.
- **A node vanishes mid-run.** The run aborts and is reported invalid with the
  lost node named. Continuing on the survivors would quietly change the
  workload rather than degrade it visibly.
- **A conversion target cannot express something the trace holds.** The loss is
  declared in the output or the report, never absorbed silently, and the
  converted file is not accepted as a substitute for the native trace when
  reproducibility is being checked. A conversion that quietly drops
  cross-session reuse would still load and still replay, which is why this is a
  stated rule rather than left to judgement.
- **An emit run is asked for system measurements it cannot make.** Latency,
  lane utilisation, and the virtual-to-wallclock ratio are absent from an emit
  report rather than present and zero, because a zero would be
  indistinguishable from a measured result.
- **The generator crashes mid-run.** Remote resources must still be released; a
  teardown that leaves them held has previously invalidated whole measurement
  series, and did so nondeterministically rather than visibly.
- **A discrete quantity is drawn from a bimodal sample set with interpolation
  on.** Interpolation places mass in gaps the data says are empty. Permitted
  and documented; discrete draws are available and are the default for block
  counts.
- **A truncated distribution's effective mean differs from the written one.**
  Reported at load time rather than applied silently.

## Requirements *(mandatory)*

### Functional Requirements

**Workload description**

- **FR-001**: System MUST accept a workload description file defining shared
  object classes and session classes, per the normative schema in
  `contracts/workload-input.example.yml`.
- **FR-002**: System MUST validate the whole description before issuing any
  operation, and MUST refuse a contradictory configuration naming the offending
  parameter.
- **FR-003**: System MUST report, at load time, the effective distribution for
  every parameter whose effective value differs from what was written —
  including truncation that shifts a mean and an implied maximum that discards
  tail mass.
- **FR-004**: System MUST refuse a configuration in which an implied maximum
  discards more than 5% of a distribution's mass.
- **FR-005**: A workload description MUST NOT contain host-specific or tuning
  settings; node targets, run length, output destination, concurrency, keys per
  request, and seed are supplied per invocation, so one description is portable
  across clusters unchanged.
- **FR-006**: System MUST carry a schema version in the description and reject
  versions it does not understand.

**Distributions**

- **FR-007**: System MUST support constant, uniform, empirical, normal, and
  exponential distributions, all treated as continuous.
- **FR-008**: System MUST truncate distributions by inverse transform between
  the cumulative probabilities at the stated bounds — a true truncation — and
  MUST NOT clamp, which would pile mass at a boundary and shift the mean.
- **FR-009**: For integer-valued quantities, System MUST truncate on bounds
  extended by half a unit at each end and round to nearest, so that every
  integer in range carries equal weight.
- **FR-010**: System MUST accept empirical distributions from inline samples or
  from a file, and MUST support both interpolated and discrete draws,
  defaulting to discrete for counts of blocks or objects.
- **FR-011**: System MUST accept an unbounded value wherever a number is
  accepted.
- **FR-012**: All sampling MUST be reproducible from a stated seed.

**Shared object populations**

- **FR-013**: Each shared object class MUST have a target population expressed
  either as a fluctuating population — free-running births at a rate derived
  from the target and the mean lifetime, giving a live count whose relative
  spread falls as the inverse square root of the target — or as an exact
  population held constant by replacing each death immediately.
- **FR-014**: The fluctuating form MUST be the default; the exact form MUST be
  available and is the appropriate choice for small populations.
- **FR-015**: System MUST seed every population at start from the equilibrium
  residual-lifetime distribution, NOT from the lifetime distribution itself,
  which synchronises the population into cohorts whose periodic churn persists
  for many lifetimes.
- **FR-016**: System MUST NOT use a feedback controller or expose a control
  gain for population regulation. A regulator suppresses the very fluctuation
  FR-013 asks for: measured, it holds the live count's `var/mean` at 0.48 where
  an uncontrolled birth-death population sits at 1.00, so tight regulation is a
  fidelity loss rather than a gain. It would also add two tuning parameters
  that FR-005 forbids a portable description from carrying.

  *Note.* This requirement was originally justified by a measured "8-23%
  positive population bias from the non-negative creation rate". **That
  justification was wrong** and is retracted. Checked against the reference C
  implementation in `~scooter/birth-death/`, whose own `xystats` reports
  `y_mean = 99.1974` against a target of 100, the controller regulates the mean
  to under 1%, and at those parameters the non-negative constraint never binds
  at all. The 8-23% figure came from a badly-scaled parameter sweep — the
  per-sample loop gain scales as the square of the sampling period — not from a
  property of feedback. The requirement stands on the variance argument above,
  which the same measurements support. Both the corrected study and the
  retracted one are reproducible in `research/population/controller.py`.
- **FR-017**: A class whose lifetime is unbounded MUST be minted once at start
  and never turn over.
- **FR-018**: When a shared object reaches end of life it MUST stop being
  selectable for new sessions; its bookkeeping MUST be released once the last
  session using it ends. System MUST NOT tell Certus anything about this.

**Popularity and selection**

- **FR-019**: Each shared object class MAY carry a selection distribution over
  the instance index, whose spread sets the working-set size independently of
  the population's key-space size.
- **FR-020**: System MUST support popularity attached to a position, which a
  newly created instance inherits and holds for life, and popularity attached
  to newness, where heat decays as newer instances arrive. Both MUST exist,
  because that is the axis on which recency-based and frequency-based eviction
  policies disagree.
- **FR-021**: Selection MUST default to uniform, and System MUST report the
  implied working-set size when a class has more than one instance and no
  selection distribution.
- **FR-022**: A session MUST draw instances without replacement within a class,
  bounded by the smaller of the nominal population and the live count, so the
  draw always succeeds without waiting or failing.
- **FR-023**: System MUST report how often that bound binds, because it narrows
  the requested count distribution.

**Sessions, turns, and keys**

- **FR-024**: Each session class MUST have a target population interpreted as
  concurrent sessions, taking the same fluctuating and exact forms as a shared
  object population; session arrival rate is therefore an output, settling at
  the target divided by the mean session lifetime.
- **FR-025**: A session's context MUST be append-only: turn *n* reads its whole
  prefix — its shared objects plus every input and output block minted by turns
  1 through *n*-1 — so per-turn cost grows linearly with turn index.
- **FR-026**: Input and output block growth MUST be separate distributions;
  both are stored and both extend the prefix.
- **FR-027**: Keys MUST be prefix-chained: a block's key depends on that block
  and everything ahead of it, so reuse between sessions requires a matching
  leading run and an object held in common at a differing position yields no
  reuse.
- **FR-028**: The ordering of a session's shared objects MUST be canonical:
  classes in the order the session class lists them, and instances sorted by
  index within a class. This makes a session's prefix a pure function of the
  set it chose, so sessions with overlapping sets produce nested rather than
  divergent chains.
- **FR-029**: A key MUST be a pure function of the block's identity and its
  parent's key, computable by any tool from the description and the seed
  alone, with no shared state and no lookup table. This is what lets a
  session's prefix be derived rather than remembered, and lets a consumer
  verify the keys in a trace it did not produce.

  *Note.* An earlier wording made this a requirement about keys being
  "globally consistent across nodes, so a remote hit is possible". That is
  true, but it is not a requirement on the key function: a run has one
  generator process, the per-node daemons relay keys without deriving them
  (FR-046, FR-047), and Certus treats a key as opaque, so during a live run
  no second party computes a key and two nodes cannot disagree about one
  however keys are chosen. Cross-node consistency is a consequence of the
  single-generator architecture. The real cost of abandoning derived keys is
  a global prefix trie in the generator, and the real second implementation
  is an offline trace consumer.
- **FR-030**: A session's own growth blocks MUST chain onto that session's
  unique prefix, making them private by construction and reusable only within
  that session.

**Virtual time and execution**

- **FR-031**: In the default **work-conserving** mode, virtual time MUST exist
  only to decide the order of operations, and MUST NOT be mapped to wallclock
  time. This is what makes the run a measurement of capability: it issues as fast
  as the mailbox allows and FR-062's plan-queue check proves the generator was not
  the constraint. FR-078 adds a second, **paced** mode in which the mapping is the
  point; until that lands, this requirement is unqualified in practice.
- **FR-032**: The virtual clock MUST be the minimum timestamp among operations
  still in flight. When the server applies backpressure, a lane blocks and the
  clock holds — it MUST NOT advance, skip, or dilate unevenly.
- **FR-033**: Think time MUST be consumed entirely when the plan is built,
  where it determines interleaving, and MUST NOT reappear during execution as
  server idle time.
- **FR-034**: For a fixed description and seed, the plan MUST be identical
  regardless of how fast or slow the server is.
- **FR-035**: Ordering within a session MUST be strict. Operations from
  different sessions MAY overlap freely, including two sessions racing to mint
  the same shared prefix.
- **FR-036**: System MUST document that hit/miss outcomes are consequently not
  reproducible even where the plan is, and MUST NOT present single-run
  comparisons of hit-dependent metrics as conclusive.
- **FR-037**: System MUST produce the plan ahead of the lanes that consume it,
  and MUST detect and report when that plan queue reaches zero.
- **FR-038**: No per-operation work may be proportional to payload size; block
  payloads MUST be pre-filled reusable buffers carrying at most a small
  identifying stamp.

**Interaction with Certus**

- **FR-039**: The operation stream MUST be what the production client would
  emit for the same workload — no more and no less.
- **FR-040**: Stores MUST follow the production client's reserve, transfer,
  then commit-or-abort sequence, and MUST NOT use the single-shot store
  operation that the production client does not have.
- **FR-041**: System MUST report block references the way the production client
  does, without requesting tier promotion.

  *Note.* This is a **separate operation** from asking which blocks are
  present, and an earlier implementation of the plan conflated the two. The
  shipped client makes two distinct calls: one asking what is resident, and one
  — documented as "update eviction ordering for the given keys" — reporting the
  reference. Only the second informs a recency-based policy, so omitting it
  scores every such policy against a workload in which nothing is ever recently
  used. Nothing in the emitted trace or the plan's own invariants would reveal
  that, which is why the requirement is called out here rather than left
  implicit in FR-039.
- **FR-042**: System MUST poll for cache events, because the production client
  does and it costs the server real work.
- **FR-043**: System MUST NOT send removal, pinning, unpinning, or promotion
  requests. A single cache clear at startup is permitted.
- **FR-044**: System MUST NOT supply the eviction policy with information the
  production client would not supply, and MUST NOT withhold information the
  production client would supply.

**Multiple nodes and the node daemon**

- **FR-045**: System MUST drive Certus on the local host with no daemon and no
  node list configured.
- **FR-046**: System MUST drive Certus on remote hosts by way of a per-node
  daemon that accepts work and submits it to that node's local ingress.
- **FR-047**: Only keys MUST cross the network; the daemon MUST reconstruct
  block payloads from the key.
- **FR-048**: New sessions MUST be placed uniformly across configured nodes; a
  migrating session MUST move to a node chosen uniformly among the others, and
  its already-stored blocks MUST stay where they were.
- **FR-049**: With fewer than two nodes configured, migration MUST be inert
  rather than an error.
- **FR-050**: The daemon MUST be started before a run and stopped after it,
  with startup and teardown outside the measured window.
- **FR-051**: System MUST verify that each daemon was built from the same
  sources as the generator, and MUST refuse to run otherwise.
- **FR-052**: Daemon startup MUST be idempotent: a leftover daemon from a
  crashed run MUST be detected and replaced, never silently reused.
- **FR-053**: Teardown MUST release the daemon's resources even when the
  generator exits abnormally, and MUST be verified rather than assumed. The
  daemon MUST therefore exit when **every connection it once had has closed**: a
  closed socket is TCP reporting that the control process is gone, however it
  went, and it is more reliable than any silence-based guess. Without it a
  generator killed outright would leave a daemon holding mailbox channels and a
  device allocation until someone next started a run.

  A daemon MUST NOT exit merely because it is **idle**. Under FR-078's paced mode
  a session's think time is real waiting, so a node may legitimately receive
  nothing for minutes; a daemon that took silence for failure would exit in the
  middle of the workload it was serving. A grace period after the last
  disconnection is permitted, and is needed because a generator opening its lanes
  one at a time passes briefly through zero connections. A daemon nobody ever
  connects to MUST also give up eventually, since it was launched for a run that
  never came.
- **FR-054**: Continuous load MUST be driven over the fast transport, not by
  repeated remote command invocation, which cannot sustain it.
- **FR-064**: When a configured node or its daemon becomes unreachable during a
  run, System MUST abort the run immediately, report it invalid, and name the
  node that was lost. System MUST NOT continue on the surviving nodes, because
  a lost node silently alters the workload — its sessions' prefixes become
  unreachable, the set of migration targets shrinks, and the survivors absorb
  its load — so any subsequent measurement describes a different experiment
  than the one requested.

**File output**

- **FR-055**: System MUST emit a workload trace at the same level of
  abstraction as a real serving trace — sessions, turns, block lists, token
  counts — in two containers holding identical records. Interoperability with
  other tools' formats is delivered by **projections** (FR-075), which are not
  containers of the trace and are not required to hold identical records.
- **FR-056**: The trace MUST be self-describing: a reader MUST learn what it
  supports by reading its manifest, never by recognising which trace it is.
- **FR-057**: The manifest MUST declare the identifier space in use, since
  identifiers are chained keys rather than dense values in creation order.
- **FR-058**: Every emitted row MUST satisfy the trace schema's stated
  invariants for the encoding it declares, including the convention for a
  trailing partial block.
- **FR-059**: Run length is bounded by exactly one control, a span in **virtual
  seconds**. An emit run MUST require it, because an unbounded file has no
  meaning. A live run MUST be unbounded by default and MAY be given the same
  bound; an unbounded live run is a legitimate mode, useful for exercising code
  paths and for giving Certus's own statistics collection a window. System MUST
  reject wallclock-looking notation for the span (FR-031), and MUST NOT offer a
  wallclock bound at all: an unbounded run stops cleanly on interruption
  (FR-074), so an external `timeout` supplies a real-time window without giving
  the tool a wallclock concept.
- **FR-073**: Before writing anything, an emit run MUST project the output from
  the description's own rates — invocations, distinct keys minted, key
  references, and bytes for each output actually requested — and MUST refuse if
  the projection exceeds either the free space on the output filesystem or a
  documented size ceiling, naming both figures. A run asked only for a
  projection MUST be projected on that basis, not on the native trace it is not
  writing. Requiring a span is not sufficient protection
  on its own: a legal span on the shipped example costs tens of gigabytes. The
  projection MUST report minted keys and key references separately, since the
  first grows linearly with the span and the second quadratically with session
  length. The byte figure MUST be the uncompressed single-container size, used
  as a conservative upper bound; a compressed container's actual size cannot be
  projected and will be smaller. If a write nonetheless fails for lack of
  space, the ordinary error MUST propagate — no completeness flag is needed,
  because the manifest is written last, so a trace directory without one is
  incomplete by construction (FR-056). The same projection MUST be available
  without writing, and MUST warn when the span is short relative to the longest
  finite lifetime in the description, because such a trace cannot exhibit the
  pool turnover the description specifies.
- **FR-074**: An unbounded live run MUST stop cleanly on interruption: drain
  in-flight requests, tear down node agents (FR-053), write both report forms,
  and classify itself valid or invalid on the ordinary criteria. Interruption
  is not a failure — an interrupted run whose plan queue never reached zero is
  valid.
- **FR-060**: System MUST provide a canonical serialisation of the operation
  plan, as the artifact the reproducibility property is asserted against.

**Interoperability with other tools' trace formats**

- **FR-075**: Every other trace format System speaks MUST be a **projection of
  the operation plan**, and MUST be obtainable **in one pass during an emit
  run**. System MUST NOT require the native trace to be written first as an
  intermediate: producing a projection of a workload nobody wants stored would
  otherwise cost tens of gigabytes, plus a possible free-space refusal, to
  arrive at a file that may be a few hundred megabytes. There is no
  standards-body format at this level of abstraction, so the emitted schema is
  chosen for being a superset of the public ones and every target is a
  projection of it. See `research.md` D8 and `contracts/trace-interop.md`.
- **FR-075a**: System MUST also apply the same projection to an
  **already-stored trace** in the emitted schema, and both entry points MUST
  use the same projection so that they cannot disagree. This is not a redundant
  path to the same place: the emitted schema is shared with a corpus of real
  traces, so projecting a stored trace is the only way to put a real workload
  and a generated one through an identical transformation — which is what makes
  them comparable at all.
- **FR-075b**: A projection is **not a trace**. It has no manifest, is not
  self-describing, and MUST NOT be accepted in place of the native trace or the
  canonical plan when reproducibility is being checked (FR-060, FR-072).
  Consumer-specific shapes therefore stay out of the trace contract: what a
  direct projection changes is where bytes are written, never what a trace
  means.
- **FR-076**: A conversion target MUST be judged on two properties before it is
  adopted, and System MUST NOT emit a workload in a format that lacks either:
  identifiers scoped to the whole run, and timestamps on a run-global clock.
  Cross-session interleaving and cross-session key reuse are the two things a
  cache actually responds to, so a format that cannot carry them describes a
  different workload however faithfully it records each session.
- **FR-077**: A conversion MUST declare what it drops. Where a target cannot
  carry something the trace holds — session grouping, the separation of input
  from output blocks, the trailing-partial-block count — the conversion MUST
  record the loss in its output or its report, and MUST NOT be usable as a
  reproducibility check in place of the native trace.
- **FR-078**: Where a target requires dense identifiers, the conversion MUST
  renumber across the **whole output**, never per session. Per-session
  numbering would destroy cross-session reuse while producing a file that loads
  and replays, which is the failure mode this requirement exists to forbid.

**Reporting and validity**

- **FR-061**: For a **live run**, every run report MUST carry plan-queue depth,
  lane utilisation, and request-latency percentiles alongside throughput. These
  measure the system under test and have no meaning in an **emit run**, which
  has no lanes, no requests, and no server; see FR-071.
- **FR-062**: A **live run** whose plan queue reached zero MUST be reported as
  invalid, and its throughput MUST NOT be presented as a result. In an emit run
  a drained plan queue carries no such meaning — it means only that the writer
  outran the simulation — and MUST NOT be treated as invalidity.
- **FR-063**: Run reports MUST record the seed, the description file identity,
  and the effective parameter values after truncation, so a run can be
  reproduced from its own report.
- **FR-065**: System MUST emit the run report in two forms: a human-readable
  summary to the terminal, and a structured machine-readable file per run at a
  caller-specified path, carrying the same facts. Both are required because the
  acceptance criteria for popularity structure (SC-005, SC-006) are inherently
  multi-run sweeps, so aggregation across runs must not require scraping
  human-formatted text.
- **FR-066**: For a **live run**, throughput MUST be denominated in wallclock
  time, and System MUST report both keys per second and bytes per second, plus
  the ratio of virtual time advanced to wallclock elapsed. **Bytes per second
  MUST count only payload that crossed the client boundary**, and MUST be
  reported separately for each direction: a block is read on a `LOOKUP` **hit**
  and written on an accepted `COPY_TO_STORE`, and nothing else moves a byte —
  `CHECK`, `TOUCH`, `RESERVE` and `COMMIT_STORE` are control. Bandwidth is
  therefore computable only from the hit/miss results, which is one reason
  FR-072a requires capturing them. Deriving it from key references instead
  charges a full block to every control operation and overstated bandwidth by
  4.9x when measured.
- **FR-066a**: Latency MUST be reported **per operation**, not only in aggregate.
  A control operation costs tens of microseconds uncontended; a `LOOKUP` DMAs a
  block per key and was measured at eleven times that. An aggregate percentile
  therefore describes the operation mix a description happens to produce rather
  than anything about Certus, and changes when the hit rate changes even if the
  server does not.

  Each operation's **request count MUST be reported beside its percentiles**, and
  a count too small to support them MUST be marked. A p50 over a few dozen
  requests is noise and a p99 over a few dozen *is* the maximum. This is not
  hypothetical: a 24-request run appeared to show `TOUCH` costing three times
  `CHECK`, and a second 24-request run appeared to show it four times faster. At
  8000 requests each they differ by one microsecond.
- **FR-066b**: A store MUST transfer only into a reservation the server granted.
  `RESERVE` answers per key, and transferring for a declined key sends a payload
  to no slot and then fails its commit for want of a pending write. Measured
  without the filter: 88 blocks written against 12 declined reserves and 12
  declined commits; with it, 76 written and zero declined commits. Reporting wallclock
  throughput does not conflict with FR-031: virtual time decides the *order* of
  operations while wallclock measures how fast that order was executed, and
  because the virtual clock holds during a stall (FR-032) the ratio between
  them is a measurement rather than an identity. Keys and bytes per second are
  comparable across workload descriptions; the virtual-time ratio is not, since
  it depends on the description's own virtual-time density.
- **FR-067**: For a **live run**, the timed window MUST exclude daemon startup
  and teardown and any startup cache clear, so setup cost is never attributed
  to the system under test.
- **FR-068**: For a **live run**, plan-queue depth MUST be reported as, at
  minimum, its minimum over the run and the fraction of the run spent at zero —
  the evidence for FR-062 — rather than as an average, which would conceal a
  brief exhaustion.
- **FR-069**: The number of keys grouped into one request MUST be a per-run
  option with a documented default, not a constant and not part of the workload
  description. It is a property of the client's request scheduling rather than
  of the workload, so putting it in the description would break FR-005
  portability; but it is also the largest measured performance lever on this
  class of hardware — the remote penalty is per batch, not per key — so it MUST
  be explicit and independently sweepable rather than hidden. The effective
  value MUST appear in the run report under FR-063.
- **FR-070**: For a **live run**, System MUST measure latency per request and
  report it as a distribution — at minimum the 50th, 90th, and 99th percentiles
  and the maximum — not as a mean, which hides the tail that matters. Timing
  MUST be taken per request and MUST NOT add per-key measurement work, so that
  instrumentation cannot itself put the generator on the critical path (FR-038,
  and Principle I of the constitution).
- **FR-071**: An **emit run** MUST report its own completeness rather than
  borrowed system metrics: sessions started and completed, turns and blocks
  emitted, the virtual-time span covered, the records written per container,
  and the reproduction parameters of FR-063. It MAY additionally report the
  generator's own wallclock rate, explicitly labelled as generation speed and
  not as a measurement of any system under test. It MUST NOT report lane
  utilisation, request latency, or a virtual-to-wallclock ratio, because no
  system was exercised and a number in those fields would invite comparison
  against live results.
- **FR-072**: The plan and the emitted trace MUST be identical for the same
  description and seed whether the run is live or emit, and MUST NOT vary with
  the tuning options of FR-069 or with lane count. Batching and concurrency
  govern only how operations are grouped and dispatched on the wire, never what
  the workload is. Without this, a trace would not be a function of description
  and seed, and SC-003 would be false; with it, a trace generated on a laptop
  is known to be the workload a cluster would have been driven with.
- **FR-072a**: Cache outcomes MUST NOT change the workload, and MUST change the
  interaction. The virtual clock, the session interleaving, the turn schedule and
  the **key path** of each turn are functions of description and seed alone; a
  hit or a miss may not perturb any of them. But the **operations issued** to
  Certus are a function of that path *and of what the cache reports*, because a
  real prefix-caching client does not know in advance what it must store: it
  offers every key from the root of the prefix through the end of the turn's new
  growth, loads what came back resident, and stores what came back absent.

  This is what FR-036's race describes — two sessions racing to mint the same
  shared prefix both miss and both store — and it is what a fixed operation list
  cannot express. Two consequences follow, and both are requirements rather than
  side effects. A block **evicted mid-run MUST be stored again** when a later turn
  finds it absent; otherwise a run's hit rate can only decay and the generator
  would be measuring a cache it never refills. And **shared prefix blocks MUST be
  stored**, by whichever turn first finds them missing; no turn mints them, so a
  fixed operation list stored them never, and cross-session prefix sharing — the
  phenomenon this generator exists to exercise — produced no cache hits at all.

  A key reported `PENDING` MUST NOT be re-stored or loaded: another lane's store
  is in flight, so storing would duplicate it and loading would race the writer.
- **FR-072b**: For a remote node, the **key path MUST cross the wire, not the
  operations**. The reactive rule of FR-072a costs several round trips per turn,
  which is nothing at `/dev/shm` latency and unacceptable over a fabric, so a
  resident per-node agent receives the turn's path and performs the check, the
  loads and the stores against its own local Certus. The wire therefore carries
  what is deterministic — paths, session identity and virtual timing — and never
  cache outcomes, which keeps FR-072's guarantee intact across nodes while
  keeping the chatter host-local.

### Agreed: proxy the local node through an agent too

- **FR-079** *(AGREED, to be implemented after T074)*: The generator SHOULD reach every node
  through its per-node agent, **including the local one**, rather than talking to a local
  mailbox directly. One transport, one driver, one cleanup mechanism.

  **The measurement objection does not apply**, which is what makes this affordable. A
  loopback hop would normally inflate the latency being measured, but FR-066/FR-072a already
  require the mailbox-facing code to be the sole collector: the agent times its own mailbox
  requests, so reported per-op latency and bandwidth exclude the hop. It affects only how
  fast the generator can *feed* work, which FR-062's plan-queue check already guards.
  Loopback round trips at the pipelining depth of `contracts/node-agent-wire.md` are three
  orders of magnitude above any rate measured on this hardware.

  What it buys, in order:

  1. **One driver.** FR-072a's *rule* already has one implementation, but the local and remote
     paths are still two drivers. Collapsing them removes the divergence class FR-072 exists
     to guard, rather than testing for it.
  2. **One cleanup mechanism.** FR-052 and FR-053 are then satisfied the same way everywhere.
     Two mechanisms have already produced two defects here — a mailbox channel not returned
     when a connection closed, and an agent listening forever after its control process died.
  3. **The generator stops depending on CUDA and on the mailbox**, and therefore stops
     enabling `interfaces/spdk` transitively. It could become a workspace default member, and
     the `live` feature's purpose largely disappears.
  4. **The generator can run off-cluster**, on a host with no Certus and no accelerator. Not a
     requirement; a consequence.

  Costs, recorded rather than discovered later: an extra process for the simplest run, which
  the generator SHOULD hide by launching a local agent itself without ssh; and
  `CLEAR_MEMORY_TIER`, which the generator issues directly today and would need as a frame.

  **When to do it.** After T074, which now exists — and the recommendation is to wait for
  FR-078 rather than to do it next. Two reasons, one of which retires the original urgency:

  * **The divergence it prevents is already guarded.** T074 compares the two paths and the
    executor is single, so they cannot silently drift today. That was the argument for doing it
    soon, and it is spent. What remains is structural tidiness and the dependency win — real,
    but not time-critical, and paid for against a local path that is complete and *measured*.
  * **FR-078 is the moment the cost becomes payable.** Pacing changes *when* a request is
    submitted, which lives in the driver, so with two drivers it is built twice. Unify first,
    then implement pacing once.

  One rule holds either way: **do not add features to both paths while both exist.** That is
  precisely how they diverge, and it is what would make T074 begin failing for a real reason.

  A cost recorded in the interest of not overselling this: the loopback hop is free for reported
  latency, since the agent times its own mailbox requests, but it is not free for the rate at
  which the generator can *feed* work — and FR-078's rate sweep exists to push that rate until
  something breaks. Measured: pipelining depth 8 at a ~30 µs loopback round trip is ~260 000
  turns/second against ~1 000 turns/second observed, so roughly 250x headroom. Not binding, but
  it is the number to re-check if a sweep ever reports the generator as the limit.

  And one hazard it *removes* rather than adds: a generator killed outright leaks its mailbox
  channel claims today, with no recovery path until the server restarts. Under FR-079 the
  agent's leftover replacement covers that case.

  **Sequencing is part of the decision.** This MUST come after the loopback-equivalence test
  (T074), not before. That test is what compares the two paths, so it is the instrument that
  demonstrates the unification changed nothing — unifying first would collapse onto a path
  never shown equivalent, and would destroy the means of showing it.

### Deferred: paced mode

- **FR-078** *(DEFERRED — agreed design, to be implemented once everything else
  is settled)*: System MUST offer a **paced** mode alongside the default
  work-conserving one, in which a request is **held back until its virtual time
  is due**, so that the offered load matches the workload's own rate. The two
  modes answer different questions and both are wanted: work-conserving measures
  *how fast Certus can go*, paced measures *the latency Certus delivers under the
  load this workload actually represents*. Latency is the reason for the feature —
  a percentile gathered while the generator sprints describes a queue that the
  real workload would never form.

  **The validity metric changes with the mode, and this is the substance of the
  work rather than a detail.** FR-062 invalidates a run whose plan queue reached
  zero, which is meaningful only when the generator is trying to sprint. Under
  pacing an empty queue is the normal, intended state — nothing is due yet — so
  that test must be replaced by **lateness**: for each request, `due =
  t0 + (virtual timestamp − virtual start) / rate`, and `lateness = submitted −
  due`, positive meaning the generator missed its slot. Pacing never submits early
  by construction, so lateness is one-sided. A run whose lateness exceeds its
  tolerance is invalid for the same reason FR-062 exists: the generator, not
  Certus, set the pace.

  Requirements that follow:

  - The report MUST name the mode, because a throughput from a paced run and one
    from a work-conserving run are not comparable and would otherwise be quoted
    side by side.
  - Lateness MUST be reported as percentiles with its request count, per FR-066a.
  - FR-072 and FR-072a MUST continue to hold: pacing changes *when* a request is
    submitted, never which keys a turn names or how sessions interleave.
  - A **rate multiplier MUST exist**, expressed as virtual seconds per wallclock
    second, and it is a calibration control rather than a convenience. A
    description's durations are arbitrary with respect to any particular machine:
    `think_time` and session lifetimes reflect whatever hardware the workload was
    observed on or imagined for, so on faster hardware the same description
    under-drives the system and on slower hardware it over-drives it. The
    multiplier is how one description is aimed at different targets without being
    rewritten, and it also controls cost, since a paced run takes wallclock equal
    to its virtual span divided by the rate.

    It belongs on the command line and **MUST NOT be a field of the description**,
    for FR-069's reason exactly: it describes the target hardware rather than the
    workload, so putting it in the YAML would conflate the two and break FR-005
    portability.

  - **The rate is the load knob, so a rate sweep is the capacity measurement.**
    Offered load scales with the rate while the workload's shape does not, so
    sweeping it and watching where lateness leaves zero gives the rate at which
    this machine stops serving this workload on time — a load-versus-latency curve
    rather than a single number. That is a better instrument for "can this machine
    serve this workload" than the work-conserving ceiling, which reports a
    saturated queue's latency; the work-conserving mode remains useful for a pure
    bandwidth ceiling with no schedule to keep.

  - **Rate MUST NOT change the workload.** It changes only the tempo at which the
    plan is played: the same keys in the same order with the same virtual
    interleaving and the same session concurrency, submitted faster or slower.
    FR-072 continues to hold, and a run's plan fingerprint MUST be independent of
    the rate.

  - **In paced mode the reported `virtual/wallclock` ratio becomes a check on the
    requested rate.** It should come out at approximately the rate asked for; a
    measured ratio below the requested one is the same information as accumulated
    lateness, arriving by a second route, and the two MUST agree.
  - FR-031 MUST be scoped to the work-conserving mode when this lands.

  **Paced MUST be the default, and the argument is the constitution's own.** Its
  rationale for the three measurement principles is that each guards "a specific
  failure mode that produces plausible numbers rather than an error". Apply that
  test to the choice of default and it is asymmetric:

  * Paced by default, when the operator wanted a ceiling: throughput comes back
    capped at the rate they asked for and lateness is ~0. The number equals the
    request, which is conspicuous and hard to misquote.
  * Work-conserving by default, when the operator wanted their workload's latency:
    percentiles come back from a **saturated** queue. They look entirely plausible
    and describe a queue the workload would never form.

  The second is the failure mode the constitution names, so the default must be
  paced. A supporting argument from the data model: `think_time`, arrival rates and
  session lifetimes are most of what a description says, and a work-conserving
  default makes those fields decorative for a live run.

  **The selector MUST be positive rather than `--unpaced`.** A negative flag cannot
  be read without already knowing the default, and it collides with the rate
  multiplier — `--unpaced --rate 10` has no meaning and would have to be rejected.
  `--pacing real|none`, defaulting to `real`, with `--rate` valid only under
  `real`, keeps the flag one-to-one with the mode the report is required to name.

  **Due times MUST be absolute, and this is a correctness requirement rather than
  a style choice.** `due` is computed from `t0` and the request's virtual
  timestamp, **never** from the previous submission. If a late request pushed later
  due times back, a slow server would silently stretch think time and dilate the
  workload — which is precisely the hazard the constitution names when it warns
  that "a wallclock-coupled clock lets a slow server quietly reshape the workload
  it is being judged on". With absolute due times the schedule is fixed in advance
  and lateness accumulates as a *measured error*, so pacing measures the deviation
  instead of absorbing it.

  **A paced run MUST project its wallclock cost before starting**, symmetrically
  with FR-073's size projection for an emit run: at rate 1.0 a paced run takes
  wallclock equal to its virtual span, so `--until 3600` costs an hour and a tool
  that simply went quiet would look hung. Naming the figure up front is the
  difference between a long run and an apparently broken one.

  **Known cost**: two modes means two validity rules, two meanings for the
  virtual-to-wallclock ratio, and a report that must be unambiguous about which it
  is showing. That complexity is accepted deliberately and is the reason this is
  deferred rather than folded into US1.

### Key Entities

- **Workload description**: the user's input; a set of shared object classes
  and session classes with their distributions. Portable across clusters.
- **Shared object class**: a named population of interchangeable objects with a
  size distribution, a lifetime distribution, a population target and form, and
  optionally a popularity distribution and ranking mode.
- **Shared object instance**: one member of that population, with an index that
  determines both its selection probability and its position in a prefix.
- **Session class**: a named population of conversations, with an ordered list
  of shared object classes and per-session counts, plus turn count, input and
  output growth, think time, migration interval, and population target.
- **Session**: one conversation. Owns an ordered prefix, a growing private
  continuation, a current node, and a turn cursor.
- **Turn**: one request within a session. Reads the whole current prefix, mints
  new input and output blocks, extends the prefix.
- **Block**: the unit of cache residency. Identified by a chained key derived
  from itself and everything ahead of it.
- **Operation plan**: the totally ordered sequence of cache operations with
  virtual timestamps. Deterministic from description and seed.
- **Lane**: one unit of execution concurrency, holding one session's turn at a
  time.
- **Node target**: a host plus the name of its local ingress.
- **Node daemon**: the per-node process that receives keys and submits work to
  that node's ingress. Holds no persistent state.
- **Workload trace**: the emitted, self-describing record of the workload, at
  serving-trace level.
- **Live run**: a run that issues operations to one or more Certus nodes. The
  only kind that measures a system under test, and therefore the only kind to
  which throughput, latency, lane utilisation, and plan-queue validity apply.
- **Emit run**: a run that writes the workload to a file and contacts no
  server. Needs no accelerator, no daemon, and no cluster. Reports
  completeness, not performance.
- **Run report**: for a live run, throughput plus the validity evidence and the
  reproduction parameters; for an emit run, completeness plus the same
  reproduction parameters. Emitted both as a terminal summary and as a
  structured per-run file.
- **Plan queue**: the bounded buffer of built-but-not-yet-issued operations
  that sits between the simulation and the lanes. Its depth is the evidence
  that the generator stayed ahead of the system under test; reaching zero means
  the lanes idled waiting for the generator, which invalidates the run.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An engineer can drive a local Certus node from the shipped
  example workload with a single command and no daemon or node list configured.
- **SC-002**: Every live run reports throughput together with its validity
  evidence, and a live run that could not keep its lanes fed, or that lost a
  configured node mid-run, is labelled invalid rather than reported as a
  measurement. An emit run reports its own completeness instead, and is never
  labelled with a system measurement it did not make.
- **SC-003**: Repeating a run with the same description and seed reproduces the
  operation plan byte for byte — including when the server is deliberately
  slowed, when the run is an emit run with no server at all, and across
  differing request-batching and lane settings — so the workload is provably
  independent of server speed, of execution mode, and of tuning.
- **SC-004**: A workload written to both containers yields identical records,
  and 100% of rows satisfy the trace schema's invariants for the encoding
  declared in the manifest.
- **SC-005**: Sweeping cache size across at least five points spanning a
  hundredfold range produces a monotonically rising hit rate in which no single
  step contributes more than half of the total rise — the evidence that the
  workload's popularity structure is concentrated rather than flat. A uniform
  workload fails this by construction, which is what makes it a real test.
- **SC-006**: Holding workload and cache size fixed, the two popularity ranking
  modes reverse the measured ranking of two eviction policies, with the
  difference significant across repeated runs — demonstrating the tool can
  discriminate between policies rather than merely load them.
- **SC-007**: A migrated session's turns produce remote fetches on its new
  node, confirming the multi-node path is exercised rather than nominally
  configured.
- **SC-008**: A configuration whose requested counts cannot fit their
  population is refused before any operation is issued, with both the requested
  and effective means named.
- **SC-009**: A run against a node whose daemon does not match the generator's
  build is refused rather than measured.
- **SC-010**: Remote resources are released after every live run, including
  runs that ended abnormally, verified rather than assumed.
- **SC-011**: The generator's own per-operation cost stays far enough below the
  end-to-end cost of the path it feeds that the plan queue never reaches zero
  anywhere within the scale target of SC-012.
- **SC-012**: A single generator instance sustains 10,000 concurrent sessions
  and 10,000,000 live keys for a run lasting hours, with no unbounded growth in
  resident memory, and — for a live run — without the plan queue ever reaching
  zero, and steady throughput between the first and last hour.

## Assumptions

- **Users are performance engineers**, not end users; the interface is a
  command line plus a description file, and technical vocabulary in reports is
  appropriate.
- **Scale is a design target, not just configuration.** SC-012 fixes it at 10K
  concurrent sessions and 10M live keys over runs of hours. Two consequences
  follow and constrain the design rather than merely describing it: per-session
  state grows linearly with turn count while per-turn *work* grows linearly
  with turn index, so total work is quadratic in session length; and live key
  tracking at 10M keys must stay within a few hundred megabytes, which rules
  out per-key allocation and heavyweight per-key bookkeeping. Workloads larger
  than the target are permitted to run — the tool reports the limit it reached
  rather than refusing — but are not guaranteed to satisfy SC-011.
- **A Certus server is already running and configured** by other means.
  Starting or configuring Certus is not this feature's responsibility.
- **The production client's behaviour is the fidelity reference.** Where this
  specification says "what the production client would emit", the current
  client in this repository is the authority, and a change to it is a change to
  this feature's requirements.
- **Nodes are symmetric enough to compare.** Cross-socket asymmetry between a
  node's network device and its accelerator has previously produced large
  variance between otherwise identical nodes; workload placement does not
  attempt to correct for it.
- **Hit-dependent comparisons need repetition.** Because mint races are
  preserved deliberately, A/B work uses repeated runs and a stated significance
  test rather than single runs.
- **Fitting a description from real traces is out of scope**, but the trace
  format is chosen so that flow can read the same shape when it returns.
- **Bit-identical plans across different machines are out of scope**, and this
  is a decision rather than a gap. FR-034 asks only that a plan not depend on
  server speed, which it does not. Across *machines*, `f64::exp` and `f64::ln`
  are not guaranteed bit-identical between libm versions, so a draw landing
  within an ULP of a rounding boundary could round the other way and change a
  plan structurally. The values are otherwise identical to within an ULP, keys
  are integer-only and so identical everywhere, and no live run's validity
  depends on it — so the consequence is confined to a plan *digest* failing to
  match across boxes. Compare statistics rather than digests when doing that;
  the reasoning is in `crates/workload-model/src/special.rs`.
- **Reading third-party trace corpora is out of scope**, and so is the
  `metadata_only` (arrival-plus-counts) export. The first is deferred rather
  than rejected — the reasoning for the one corpus worth importing is preserved
  in `contracts/trace-interop.md` so it need not be re-derived. The second was
  rejected outright: it discards every trace of reuse, which is the one thing
  this feature exists to model.
- **An OpenTelemetry writer is out of scope**, though it is the only governed
  standard in this space. It holds no block identity, so the reuse structure
  would have to live in fabricated text whose every block tokenises to exactly
  the block size — pinning one tokenizer and chat template into the artifact,
  at roughly 50× the size, to arrive back at the keys we started from. Worth
  building only if driving a real inference engine becomes a goal; the full
  reasoning is in `contracts/trace-interop.md` so it need not be re-derived.
- **An open-loop paced mode is out of scope.** The consequence is accepted:
  this feature cannot produce a saturation or latency-versus-offered-load
  curve, because there is no fixed offered load to hold.
- **Fan-in is out of scope**: a session's chain is a path, not a graph.
- **Linux on x86-64 only.** The ingress transport's correctness depends on that
  platform's memory ordering.
