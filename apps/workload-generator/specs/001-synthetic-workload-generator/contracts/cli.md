# Contract: Command-Line Interface

**Version**: 1
**Status**: Draft
**Applies to**: `workload-gen` (and `workload-node-agent`, at the end)

The workload description file says *what* the workload is. Everything here says
*where and how fast* to run it, which is why none of it belongs in the file
(spec FR-005) — one description stays portable across clusters unchanged.

## Subcommands

```text
workload-gen run     <description.yml> [target options] [tuning] [reporting]
workload-gen emit    <description.yml> --until <virtual-seconds> --seed <n>
                     [--certus-unified-jsonl <dir>]
                     [--certus-unified-parquet <dir>]
                     [--mooncake <file>]
                     [--libcachesim <file>]
                     [--simulator <file>]
                     [--report <file>] [--force]
workload-gen plan    <description.yml> --output <file>
workload-gen convert <trace-dir> --to simulator|mooncake|cachesim
                                 --output <file>
workload-gen validate <description.yml>
```

- **`run`** is a *live run*: issues operations to Certus. Unbounded by default
  (spec FR-059); stops on signal.
- **`emit`** is an *emit run*: writes output, contacts no server, needs no
  accelerator. `--until` is **required** (FR-059) — an unbounded file is not a
  thing.

  **One flag per format, one destination each, and none of them privileged.**
  The only rule is that **at least one is required**; there is no default
  output and no format a run is obliged to produce. A run that asked for
  nothing is refused with exit 2 and the message names all five flags.

  | Flag | Writes | Destination |
  | --- | --- | --- |
  | `--certus-unified-jsonl` | native trace, JSONL container | a **directory** |
  | `--certus-unified-parquet` | native trace, parquet container | a **directory** |
  | `--mooncake` | Mooncake projection | a **file** |
  | `--libcachesim` | libCacheSim CSV projection | a **file** |
  | `--simulator` | cache-simulator projection | a **file** |

  The native destinations are directories and the projections are files because
  that is what they are, not by convention: a native trace is
  **self-describing**, so it is a directory holding `manifest.json` beside its
  records, exactly as `contracts/trace-io.md` specifies. A projection has no
  manifest and is not a trace (FR-075b), so it is one file.

  **Both native flags pointed at the same directory** give one trace holding
  both containers, with one manifest, and the two record counts are checked
  against each other — that is SC-004's equivalence claim, verified on every
  such run rather than only in a test. **Different directories** give two
  independent traces, each with its own manifest.

  Asking for a projection and nothing else is an ordinary case, not a corner: a
  legal span on the shipped example costs roughly 27 GB as a native trace, and
  there is no reason to pay that to obtain a Mooncake file a fraction of the
  size. Such a run leaves **no trace directory at all** — not even an empty
  one, which would look like an interrupted run.

  **Every projection an emit run writes declares what it dropped**, in the
  rendered report beneath that projection's own line, from the same lists
  `convert` prints (FR-077). The declaration belongs here and not only on the
  `convert` path precisely because FR-075 makes this the ordinary way to obtain
  a projection: a projection-only run has nowhere else the loss could appear.
  The "a projection is not a trace" line (FR-075b) is printed **once for the
  run** rather than once per projection — three copies would read as three
  claims about three files instead of one property of all of them.

  `--report <file>` is an additional destination for the structured report,
  which is also written as `report.json` into every native trace directory and
  always rendered to the terminal. On a projection-only run it is the only way
  to keep the structured form.

  The pre-flight sizes **only the outputs requested**, and checks each against
  the free space on **its own filesystem**, grouping destinations that share
  one (FR-073). Five independent destinations can be on five different mounts;
  summing them against one would refuse a run that fits, and checking each
  alone would admit two large outputs that together overflow a mount they
  share.
- **`plan`** writes the canonical operation-plan serialisation — the artifact
  the byte-identity property is asserted against (FR-060, SC-003).
- **`convert`** applies the same projections to a trace **already on disk**
  (FR-075a). It is not a second way to do what `emit` just did: its input is
  the *schema*, and the corpus of real traces is in that schema, so this is how
  a real workload and a generated one are pushed through an identical
  transformation. Use it also for a trace whose description is no longer to
  hand. `contracts/trace-interop.md` specifies each target, what it drops, and
  which candidates were rejected.
  - `--to simulator` — the shape `apps/eviction-replay-benchmark` reads. See
    `research.md` D1: the simulator reads `{chat_id, parent_chat_id, hash_ids,
    type}`, not the emitted schema, and the projection needs no information the
    trace lacks.
  - `--to mooncake` — the Mooncake FAST'25 trace format, the recommended
    standard-format export. Renumbers keys densely across the **whole output**
    (FR-078), and reports the losses named in the interop contract.
  - `--to cachesim` — libCacheSim CSV. Prints the `--trace-type-params` string
    to use, since that reader's columns are configurable rather than fixed.
  - `--to oracle-general` — libCacheSim's binary `oracleGeneral`, the same
    reference stream carrying `next_access_vtime`. Only an emit run can fill
    that field, which is what makes Belady baselines available; it buffers the
    whole stream to do it, and refuses a span past 49.7 days rather than
    wrapping its 32-bit clock.
  Every conversion MUST name what it dropped (FR-077), and a converted file is
  never accepted in place of the native trace for a reproducibility check.
  **That refusal is executable, in two shapes**: Mooncake and both libCacheSim
  containers need block geometry, which only a manifest carries, so a
  projection offered as input is refused *at the manifest* (exit 2); the
  simulator target needs no manifest and is refused by the row schema (exit 1),
  which is sufficient because no projection satisfies any target's schema.
- **`validate`** performs the load-time checks and the effective-distribution
  report (FR-002, FR-003, FR-004) and exits. Cheap, and the fastest way to find
  a configuration error.

An **emit-only build** is available: `--no-default-features` drops the `live`
feature, so `run` disappears along with the CUDA and mailbox dependencies, and
`emit`/`plan`/`convert`/`validate` build on a machine with no accelerator and
no Certus.

## Run length

**One cap: `--until <virtual-seconds>`.** It is optional for `run` and required
for `emit`.

| | `run` (live) | `emit` |
| --- | --- | --- |
| No `--until` | **allowed — the default.** Runs until interrupted | **refused** (FR-059) |
| `--until <n>` | optional; makes the run deterministic in length | required |

Virtual seconds is the only unit with intrinsic meaning here, because it is the
unit the description itself is written in: lifetimes, think times, and
migration intervals are all virtual seconds, so "long enough for the `tool`
pool to turn over" is sayable only this way. A trace short relative to
`E[lifetime]` does not contain the turnover the description specifies, however
many records it holds, and no record count reveals that.

`--until` takes plain virtual seconds and MUST reject wallclock-looking
suffixes such as `1h` or `30m`. Virtual time is not wallclock (FR-031), and
notation that implies otherwise invites exactly the confusion that requirement
exists to prevent.

### There is no wallclock cap, deliberately

To bound a live run by real time — to give Certus's own statistics collection a
fixed window, say — use `timeout 300 workload-gen run …`. That works because an
unbounded run stops cleanly on `SIGINT`/`SIGTERM`: it drains in-flight
requests, tears down node agents (FR-053), writes both report forms, and
classifies itself valid or invalid on the usual criteria. Being interrupted is
not a failure — an interrupted run whose plan queue never reached zero is
**valid** and exits 0.

The tool therefore has **no wallclock concept anywhere**, which is a stronger
guarantee than a wallclock flag with an explanation of why it is exempt from
FR-031. A `timeout`-driven run is honestly an interrupted run, and is reported
as one.

### No other caps, and why

Caps on invocations, minted keys, key references, or sessions were considered
and **rejected**. Their justification was size control, and the pre-flight
projection below covers that completely — query, adjust `--until`, run — as
well as covering the runaway case of a mis-specified description. What remained
was matching a real trace's record count, which is a crude fidelity match and
does not warrant a flag.

Against that: five flags for one job, and a first-to-fire rule under which two
runs nominally using "the same cap" can stop for different reasons, which the
manifest would then have to disambiguate. Richness belongs on the query side
instead, where `validate` can invert the projection and suggest a span for a
target record count.

### Pre-flight projection, and why a required cap is not enough on its own

Requiring `--until` does not by itself protect the filesystem: `--until 100000`
is legal on the shipped example and costs roughly 27 GB. So before writing
anything, `emit` MUST project the run from the description's own rates —
invocations, distinct keys minted, key references, and bytes per container —
compare that against free space on the output filesystem, and **refuse** if the
projection exceeds either the free space or a documented size ceiling, naming
both numbers. `--force` overrides the ceiling but never the free-space check.

Note the two projected key quantities are not interchangeable and are reported
separately: minted keys grow linearly with the span while references grow
quadratically with session length. On the shipped example a 100,000-second span
mints ~2 × 10^8 keys but makes ~3.4 × 10^9 references.

The byte figure is the **uncompressed** single-container size, quoted as a
conservative upper bound — a compressed container's real size depends on its
compression ratio and is not projected, only guaranteed to be smaller. If a
write fails for lack of space anyway, the error simply propagates; no
completeness flag is needed, because the manifest is written **last**, so a
trace directory without one is incomplete by construction (FR-056).

`validate --until <n>` reports the same projection without writing, turning
"pick a number, wait, discover it was 27 GB" into a one-second query. It also
warns when the span is short relative to the longest finite lifetime in the
description, and can invert the projection to suggest a span for a target
record count.

## Target options (`run` only)

| Option | Default | Notes |
| --- | --- | --- |
| `--instance <host[:port][:mailbox]>` | this host, `7420`, `/dev/shm/certus-shmq` (repeatable) | one **Certus instance** to drive, through its agent (FR-081) |
| `--hardware <file>` | `./cluster.yml` if it exists | the hardware file (FR-082) |
| `--agent-binary <path>` | `workload-node-agent` | the agent **on each host**. A bare name is looked for beside the generator's own executable when the instance is local |
| `--no-launch` | off | drive agents that are already running. Weakens FR-052, so it is opt-in |

### An instance is not a machine

The unit a run addresses is a **Certus instance**, identified by `(host,
mailbox, port)` — because a machine routinely runs several, one per NUMA domain
or per NVMe device, as `deploy/multi-instance/` already does. A hostname alone
is not an identity (FR-081).

The three components may be given in any of four shapes. A component that
parses as a `u16` is the port; one starting with `/` is the mailbox:

```text
--instance node5                              # defaults for port and mailbox
--instance node5:7421                         # own port, default mailbox
--instance node5:/dev/shm/certus-numa1         # own mailbox, default port
--instance node5:7421:/dev/shm/certus-numa1    # both
```

**Two instances given the same port MUST be refused.** Each needs its own
agent, so a shared port means the second agent's startup finds the first, takes
it for a leftover of a crashed run and shuts it down (FR-052) — a two-instance
run would then drive one instance and report it as two.

**Two instances on one host given the same mailbox MUST also be refused**, and
this one is quieter: nothing collides and nothing is shut down. Two agents on
one mailbox are two agents on **one Certus instance**, so it is one cache under
two names — the simulation would place sessions across what it believes are two
independent caches, and a migration between them would be served as a hit where
FR-048 intends a miss. The run would simply measure a deployment other than the
one described. Found while implementing the port refusal, which is the same
failure with a louder symptom.

There is deliberately **no `--shm-path` and no `--agent-port`.** Their only
remaining job would be to set a uniform *non-default* mailbox or port across
every instance, which is cheap to say per instance on a command line meant for
a few of them, and belongs in the hardware file for more. `--agent-binary`
stays global on a different principle: the handshake **requires** every
instance to run the same build (FR-051), so a per-instance binary is a
misconfiguration the protocol already refuses.

**Every instance goes through an agent, including a local one** (FR-079). With
no `--instance` and no hardware file the run drives this host, through an agent
the generator launches as a **child process with no ssh** — `ssh localhost`
would want the host's own key trusted for its own account, which is a
configuration step for the simplest possible invocation. So the run still needs
no setup, and the transport, the driver and the teardown are one mechanism
everywhere.

With fewer than two instances, migration is inert rather than an error
(FR-049). Note that two *co-resident* instances are two independent caches, so
a session migrating between them is a genuine cache miss and a legitimate
experiment — unless the deployment enables Certus's remote lookup, in which
case the arriving instance may fetch the prefix from the one the session left
and the migration is a remote hit instead. See `hardware.md`.

## The hardware file

YAML, `./cluster.yml` by default, `--hardware` to name another. It carries what
describes the **target** rather than the workload, and MUST NOT carry any part
of the workload description (FR-005, FR-082). See `hardware.md` for the schema
and `hardware.example.yml` for a file a test keeps parseable.

**The command line overrides it, per field.** A rate sweep varies the rate
while holding the deployment fixed, so the rate has to be settable without
editing the file that describes the hardware; `--instance` likewise replaces
the file's instance list rather than adding to it.

**A file picked up implicitly is announced.** When `./cluster.yml` is read
without being asked for, the run says so on its error stream before it starts,
and the report records the file's path and digest either way (FR-063) — so a
run whose deployment came from a file is still reproducible from its own
report.

## Tuning (never in the description file)

| Option | Default | Notes |
| --- | --- | --- |
| `--seed <u64>` | required for reproducibility; random otherwise, and the value used is reported | FR-012, FR-063 |
| `--lanes <n>` | 4 | execution concurrency **per instance**: one agent connection and one mailbox channel each. MUST NOT exceed the instance's channel count; the mailbox is depth-1 per channel, so concurrency *is* channel count and over-subscription silently serialises |
| `--batch-keys <n>` | documented default | keys per request. The largest measured performance lever on this hardware — the remote penalty is per batch, not per key — so it is explicit and sweepable (FR-069) |
| `--gpu-device <n>` | 0 | passed through to each agent, which owns the device buffer |
| `--rate <f64\|inf>` | 1.0 | virtual seconds per wallclock second. `inf` is the work-conserving mode (FR-080) |
| `--lateness-tolerance-ms <n>` | 100 | p99 lateness a paced run accepts before calling itself invalid |

**`--rate` is the only pacing knob, and `inf` is not a special case.** Since
`due = t0 + (virtual timestamp) / rate`, an infinite rate makes every request
due at `t0` — which is what work-conserving means. A separate `--pacing
real|none` was implemented first and withdrawn: two knobs that can disagree
need a rule to police them, and one knob makes `--pacing none --rate 10`
unsayable rather than refused. The report still names the mode, derived from
the rate.

**`inf` switches the schedule off; it does not merely divide.** Left to the
arithmetic, every request would be recorded as late by however long the run had
been going, and every work-conserving run would report itself invalid.

**A large finite rate is not `inf`.** `--rate 1e12` keeps a schedule the
machine cannot meet and is correctly invalid for lateness. The help text must
say so, because someone will type a large number meaning "flat out".

**`--rate` MUST NOT change the workload** (FR-080). It changes only the tempo:
the same keys in the same order for the same sessions, submitted faster or
slower, and a run's plan fingerprint is independent of it. It is a
**calibration** control — a description's durations are arbitrary with respect
to any particular machine — which is why it is here or in the hardware file,
and never a field of the description (FR-069's reasoning, FR-005 portability).

**`--batch-keys` and `--lanes` MUST NOT change the plan or the emitted trace**
(FR-072). They govern only how operations are grouped and dispatched. This is
directly testable and is listed in `quickstart.md`.

**There is deliberately no `--pipeline-depth`.** The pipelining window — turns
outstanding on one agent connection — is a **library parameter**
(`workload_wire::client::DEFAULT_DEPTH`, 8, threaded through `Agents::start`),
not a command-line option. An earlier revision of this contract listed the
flag; it was never implemented, and it should not be.

What the window buys is **hiding round-trip latency, not concurrency**. The
agent handles one connection's frames one at a time in arrival order, so a
deeper window does not overlap execution at the mailbox; it only keeps the
socket from idling for an RTT between turns. Concurrency comes from `--lanes`.
This is also why depth is safe for FR-035: two turns of one session may be in
flight on the wire, but the agent still executes them in order.

Three reasons the flag is not wanted:

- **The headroom is not close.** Depth 8 at a ~30 µs loopback round trip is
  about 260 000 turns/second against roughly 1 000 observed. A 100 µs fabric
  round trip still leaves 80 000/second.
- **Pacing shrank its relevance.** Under the default finite rate the generator
  is not sprinting: turns go out when they are due, so the in-flight count sits
  near 1 whatever the depth. Depth is load-bearing only under `--rate inf`.
- **FR-072's requirement does not need a flag.** What satisfies it is that
  depth never reaches the producer; what *demonstrates* it is a test that
  varies depth and compares what crossed the wire, which
  `crates/workload-gen/tests/loopback.rs` does. A flag would let an operator
  vary it; it would not make the property truer or better checked.

If a rate sweep ever reports the generator as the limit, depth is the first
knob to reach for, and exposing it is then a two-line change. Recording the
arithmetic here is what makes that decision cheap; the flag itself is
speculative.

## Reporting

| Option | Default | Notes |
| --- | --- | --- |
| `--report <file>` | none | structured machine-readable report. Required for sweeps (FR-065) |
| `--clear-cache` | off | one cache clear at startup, the only permitted hint (FR-043) |

A human-readable summary always goes to the terminal. `--report` adds the
structured file; both carry the same facts (FR-065). Sweeps aggregate the
structured files, so no one has to scrape terminal text.

## What the report contains

**A live run** (FR-061, FR-066, FR-068, FR-070):

- **the mode**: `paced` or `work-conserving`. First, because the two are not
  comparable and a report without it invites quoting them side by side (FR-080)
- throughput: keys/second, bytes/second, and
  virtual-seconds-per-wallclock-second
- request latency: p50, p90, p99, max, **per operation** — an aggregate over a
  mixed stream describes the mix rather than the system
- plan-queue depth: **minimum over the run** and **fraction of the run at
  zero** — not an average, which would conceal a brief exhaustion
- lane utilisation
- under a **finite** rate, the **schedule**: the rate asked for, the rate
  achieved, and lateness percentiles with their turn count
- validity: valid, or invalid with the reason — and **which reason depends on
  the mode**: a work-conserving run's plan queue reaching zero, a paced run's
  lateness exceeding tolerance, a lost node (named), or a wrong block returned
- reproduction: seed, description identity, effective parameters after
  truncation, `--batch-keys`, `--lanes`

**An emit run** (FR-071) reports completeness instead: sessions started and
completed, turns and blocks emitted, virtual-time span covered, records written
per container, plus the same reproduction parameters. It MAY report generation
speed, explicitly labelled as such. It **MUST omit** latency, lane utilisation,
and the virtual-to-wallclock ratio — absent, not zero, because a zero is
indistinguishable from a measurement and invites comparison against live
results.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success; for a live run, a **valid** run |
| 2 | configuration rejected at load (FR-002, FR-004) — nothing was issued |
| 3 | run completed but is **invalid**: a work-conserving run's plan queue reached zero, a paced run missed its schedule, a node was lost, or a wrong block came back. Distinct from 0 so a sweep script cannot mistake an invalid run for a result |
| 4 | peer refused: `build_id` or protocol mismatch (FR-051) |
| 1 | anything else |

Code 3 is the one that matters. A sweep driver that treats "the process exited"
as "I have a data point" is exactly how an invalid run gets published, so
invalidity is in the exit status and not only in the report.

## `workload-node-agent`

```text
workload-node-agent --shm-path <path> --port <n> [--bind <addr>]
                    --lanes <n> --block-bytes <n> --batch-keys <n>
                    [--gpu-device <n> | --no-payload]
                    [--stamp-keys] [--verify-payload]
```

Started before a run and stopped after (FR-050). Holds no persistent state, and
it is the generator that decides what it does — every `SubmitTurn` carries the
keys, the session and the virtual time, so the agent never chooses any of them.

Exits non-zero if the mailbox is absent, so a launcher learns immediately
rather than at first submission. It **releases its mailbox channels on the way
out**, which matters more than it sounds: a claim lives in the shared segment
and outlives the process that made it, so a claim not released is held until
the server restarts. Under FR-079 an agent is started and stopped for every
run, including local ones, which turns a leak of one claim into a leak per run.

The generator normally launches this itself — as a child process locally, over
ssh remotely — and `--no-launch` is for an operator who would rather run it
themselves.
