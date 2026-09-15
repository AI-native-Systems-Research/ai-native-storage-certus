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
workload-gen emit    <description.yml> --output <dir> --format jsonl|parquet|both
                                       --until <virtual-seconds>
workload-gen plan    <description.yml> --output <file>
workload-gen convert <trace-dir> --to simulator --output <file.jsonl>
workload-gen validate <description.yml>
```

- **`run`** is a *live run*: issues operations to Certus. Unbounded by default
  (spec FR-059); stops on signal.
- **`emit`** is an *emit run*: writes a trace, contacts no server, needs no
  accelerator. `--until` is **required** (FR-059) — an unbounded file is not a
  thing.
- **`plan`** writes the canonical operation-plan serialisation — the artifact
  the byte-identity property is asserted against (FR-060, SC-003).
- **`convert`** projects an emitted trace into the shape
  `apps/eviction-replay-benchmark` reads. See `research.md` D1: the simulator
  reads `{chat_id, parent_chat_id, hash_ids, type}`, not the trace-IO schema,
  and the projection needs no information the trace lacks.
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
| `--shm-path <path>` | `/dev/shm/certus-shmq` | the local node's mailbox |
| `--node <host>:<mailbox>` | none (repeatable) | remote targets, each via its agent |
| `--agent-port <port>` | 9-something, documented | agent listen port |

With no `--node`, the run is single-node and no agent is involved at all (spec
FR-045, User Story 1). With fewer than two nodes, migration is inert rather
than an error (FR-049).

## Tuning (never in the description file)

| Option | Default | Notes |
| --- | --- | --- |
| `--seed <u64>` | required for reproducibility; random otherwise, and the value used is reported | FR-012, FR-063 |
| `--lanes <n>` | | execution concurrency. MUST NOT exceed the node's channel count; the mailbox is depth-1 per channel, so concurrency *is* channel count and over-subscription silently serialises |
| `--batch-keys <n>` | documented default | keys per request. The largest measured performance lever on this hardware — the remote penalty is per batch, not per key — so it is explicit and sweepable (FR-069) |
| `--pipeline-depth <n>` | 8 | in-flight batches per node. Independent of `--lanes` (FR-072) |
| `--gpu-device <n>` | 0 | |

**`--batch-keys` and `--lanes` MUST NOT change the plan or the emitted trace**
(FR-072). They govern only how operations are grouped and dispatched. This is
directly testable and is listed in `quickstart.md`.

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

- throughput: keys/second, bytes/second, and
  virtual-seconds-per-wallclock-second
- request latency: p50, p90, p99, max
- plan-queue depth: **minimum over the run** and **fraction of the run at
  zero** — not an average, which would conceal a brief exhaustion
- lane utilisation
- validity: valid, or invalid with the reason (plan queue reached zero; a node
  was lost, named)
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
| 3 | run completed but is **invalid** (plan queue reached zero, or a node was lost). Distinct from 0 so a sweep script cannot mistake an invalid run for a result |
| 4 | peer refused: `build_id` or protocol mismatch (FR-051) |
| 1 | anything else |

Code 3 is the one that matters. A sweep driver that treats "the process exited"
as "I have a data point" is exactly how an invalid run gets published, so
invalidity is in the exit status and not only in the report.

## `workload-node-agent`

```text
workload-node-agent --shm-path <path> --listen <addr:port>
                    [--gpu-device <n>] [--block-bytes <n>]
```

Started before a run and stopped after (FR-050). Holds no persistent state.
Exits non-zero if the mailbox is absent, so a launcher learns immediately
rather than at first `Submit`.
