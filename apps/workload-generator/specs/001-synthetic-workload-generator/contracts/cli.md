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
                                       --length <virtual-seconds | turns | sessions>
workload-gen plan    <description.yml> --output <file>
workload-gen convert <trace-dir> --to simulator --output <file.jsonl>
workload-gen validate <description.yml>
```

- **`run`** is a *live run*: issues operations to Certus. Unbounded by default
  (spec FR-059); stops on signal.
- **`emit`** is an *emit run*: writes a trace, contacts no server, needs no
  accelerator. `--length` is **required** (FR-059) because an unbounded file is
  not a thing.
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
