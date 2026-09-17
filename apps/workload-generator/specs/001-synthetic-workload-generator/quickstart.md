# Quickstart & Validation Guide: Synthetic Workload Generator

**Feature**: `001-synthetic-workload-generator` | **Date**: 2026-09-17

Runnable scenarios that prove the feature works, ordered so that each needs
strictly more hardware than the last. Scenarios 1–3 need **no accelerator, no
Certus, and no cluster** — they are the bulk of the acceptance surface.

Every command below was **run** as written on 2026-09-17, except Scenario 5,
which needs a cluster, and Scenario 4's successful case, which needs a server
with free mailbox channels. Numbers quoted in the expectations are measured, not
illustrative — so a figure that no longer matches is either drift or a change
worth explaining.

Details live in the contracts rather than here: `contracts/cli.md` for options,
`contracts/key-derivation.md` for keys, `contracts/node-agent-wire.md` for the
transport, `data-model.md` for entities, `contracts/workload-input.example.yml`
for the input schema.

## Prerequisites

| Scenario | Needs |
| --- | --- |
| 1–3 | Rust toolchain only |
| 4 | a running Certus server on this host |
| 5 | two or more nodes, each with Certus and the agent deployed |

```bash
cd apps/workload-generator          # NOT the repo root — see the note at the end
```

## Scenario 1 — Validate the shipped example (no hardware)

```bash
cargo run -p workload-gen --no-default-features -- \
  validate specs/001-synthetic-workload-generator/contracts/workload-input.example.yml
```

**Expect**: exit 0, and a report of the **effective** distribution for every
field whose truncated value differs from what was written — thirteen lines on
this file. The example is deliberately informative here: `doc_analysis` draws
`long_document` counts from `exponential(mean 5)` against a pool of 20, so the
implied maximum discards 1.83% of the mass and the effective mean is **5.1435**
rather than 5. That line appearing is the check working (FR-003).

**Also expect it to refuse a bad file.** Lower that pool to 10 and re-run:

```bash
sed 's/^      size: 20$/      size: 10/' \
  specs/001-synthetic-workload-generator/contracts/workload-input.example.yml \
  > /tmp/pool10.yml
cargo run -p workload-gen --no-default-features -- validate /tmp/pool10.yml
echo "exit=$?"
```

The implied maximum then discards 13.5%, over the 5% threshold, so it exits
**2** and names both the requested mean (5) and the effective one (**3.9515**) —
see FR-004. Nothing is issued.

Those two effective means are the means of the **rounded** variable, because a
count is integral. The continuous truncated means of the same distributions are
5.565 and 4.218, and an earlier draft of this guide quoted those — but no draw
ever realises them, so they describe a quantity nothing observes. The example
file's own comment records the same correction.

## Scenario 2 — Determinism, and independence from tuning (no hardware)

The executable form of FR-072 and SC-003, and the single most important test
here.

```bash
E=specs/001-synthetic-workload-generator/contracts/workload-input.example.yml
G="cargo run -q -p workload-gen --no-default-features --"

$G plan $E --seed 42 --until 60 --output /tmp/plan-a.bin
$G plan $E --seed 42 --until 60 --output /tmp/plan-b.bin
cmp /tmp/plan-a.bin /tmp/plan-b.bin
```

**Expect**: silent, and both runs print the same `fingerprint`. Comparing the
fingerprints is the cheap form of this check — `cmp` reads the whole file, and
these are not small: the shipped example writes about 100 MB per plan at
`--until 60` and 535 MB at `--until 300`. Delete them afterwards.

`--until` is **required** on `plan`; an unbounded plan file is not a thing.

```bash
$G plan $E --seed 43 --until 60 --output /tmp/plan-d.bin
cmp /tmp/plan-a.bin /tmp/plan-d.bin      # MUST differ
rm -f /tmp/plan-*.bin
```

**Expect**: differs. A seed that changes nothing means the seed is not wired
through, which would look like determinism while being its opposite.

**The other half of FR-072 — independence from `--batch-keys` and `--lanes` —
cannot be checked here, and that is the point.** `plan` has no such flags: they
belong to the live path, and `workload-model` has no parameter through which a
batch size or a lane count could arrive. Passing them to `plan` is an
`unexpected argument` error rather than a passing comparison. So the guarantee
rests on the crate boundary, and its executable form is a test rather than a CLI
invocation:

```bash
cargo test -p workload-gen --features live --test op_stream \
  batch_keys_changes_the_requests_but_not_the_workload
```

**Expect**: passes. It drives one plan at `--batch-keys 4` and at 4096 through a
mock mailbox and asserts the requests differ while the workload does not. That
property was silently untrue for a while when `--batch-keys` was parsed but never
plumbed through — a boundary is an argument, not a check.

## Scenario 3 — Emit a trace, both containers, and feed the simulator

**Check the size before writing.** `validate --until` is the pre-flight, and the
shipped example is denser than it looks — a 3600-second span projects a 7.33 GiB
plan and a trace larger again:

```bash
$G validate $E --until 3600 | tail -8
```

An emit run does this check itself and refuses past free space (which `--force`
does not override) or past a 32 GiB ceiling (which it does). Below, a 30-second
span, which writes about 54 MiB across the two containers:

**One flag per format, and at least one is required.** There is no default output
and no format a run must produce — asking for a Mooncake file alone writes exactly
that. Pointing both native flags at the same directory is how you get one trace
holding both containers:

```bash
# The parquet container needs the parquet feature: it is non-default so that a
# plain workspace build never pulls arrow.
P="cargo run -q -p workload-gen --no-default-features --features parquet --"
$P emit $E --seed 42 --until 30 \
  --certus-unified-jsonl /tmp/trace \
  --certus-unified-parquet /tmp/trace
find /tmp/trace -type f
```

**Expect** exactly four files, and no `blocks/` directory — block role is
something real traces recover from text, and this generator does not model it
(`contracts/trace-io.md`):

```text
/tmp/trace/manifest.json
/tmp/trace/report.json
/tmp/trace/invocations/block_size_16/part-0.jsonl
/tmp/trace/invocations/block_size_16/part-0.parquet
```

- `manifest.json` declares the encoding (`source_class: pre_hashed`, full
  encoding), the block geometry, and `block_id_space: chained_u64` recording that
  identifiers are chained keys rather than dense mint-order integers (FR-057).
- The JSONL and parquet records are **identical**, and every row satisfies the
  full-encoding invariants including the trailing-partial-block convention
  (SC-004, FR-058). The run checks the two containers' counts against each other
  and refuses if they disagree, so `jsonl records` and `parquet records` in the
  report being equal is that check having passed rather than a coincidence.
  Parquet is about 3.6x smaller here — 12.2 MB against 44.1 MB — which is the
  whole reason the dependency is justified.
- The run report states completeness — sessions, turns, blocks, virtual-time
  span, records written — and **omits** latency, lane utilisation, and the
  virtual-to-wallclock ratio entirely. Their presence as zeros is a failure,
  not a cosmetic issue: a zero is indistinguishable from a measurement
  (FR-071). Check it: `grep -c latency /tmp/trace/report.json` must be 0.
- Expect **warnings**, not silence: at 30 seconds every class's mean lifetime is
  longer than the span, so the run says it cannot exhibit the pool turnover the
  description asks for. That is the projection earning its place.

Point them at **different** directories and you get two independent traces, each
with its own manifest. Ask for only `--certus-unified-parquet` and only parquet is
written — but note the projections below read the JSONL schema, so a parquet-only
trace has nothing for `convert` to read.

## Scenario 3b — The standard formats, without writing a trace first

This is the cheap path, and the one to reach for by default: no native trace, no
`convert` step, both standard formats written in the same pass.

```bash
$G emit $E --seed 42 --until 30 \
  --mooncake /tmp/mc.jsonl \
  --libcachesim /tmp/lcs.csv
ls -l /tmp/mc.jsonl /tmp/lcs.csv
```

**Expect** the two files and **nothing else** — no trace directory, not even an
empty one, because a directory holding records with no manifest is precisely what
FR-073 makes unreadable. The report is rendered to the terminal; pass `--report
<file>` to keep its structured form, which is the only way to keep it on a run
with no trace directory to hold `report.json`.

Measured at a 30-second span, so the trade is concrete:

| Output | Bytes | Carries |
| --- | --- | --- |
| `--mooncake` | 12.5 MB | 4 fields: timestamp, two lengths, prompt block ids |
| `--libcachesim` | 62.9 MB | `(time, id, size)` triples, one row **per block reference** |
| `--certus-unified-parquet` | 12.2 MB | the full 17-field schema |
| `--certus-unified-jsonl` | 44.1 MB | the same, as text |

Two things worth reading off that table. libCacheSim CSV is the **largest** of the
four while carrying the least, because it writes a row per reference rather than
per request — an existing standard format is not automatically the lean choice. And
the full schema in parquet is *smaller* than raw Mooncake JSONL, so the extra
fifteen fields cost nothing once the container stops being text.

Both projections declare what they dropped (FR-077), and `--libcachesim` prints the
`--trace-type-params` string its reader needs, since libCacheSim's columns are
configurable and a CSV file cannot say what its own columns mean.

## Scenario 3c — Converting a stored trace

Then the deferred integration question, now settled (`research.md` D1):

```bash
$G convert /tmp/trace --to simulator --output /tmp/trace-sim.jsonl
cargo run --release -p eviction-replay-benchmark -- \
  --file /tmp/trace-sim.jsonl --policy lru --cache-size 4096
```

**Expect**: the simulator loads it and reports a hit rate. Its loader derives
sessions by walking `parent_chat_id` to a root, so a converted trace whose
`parent_chat_id` chain is wrong will still *load* while collapsing every session
into one — so check the numbers agree rather than only that it ran. `convert`
prints `records / sessions / distinct keys / key references` and the simulator
prints `requests / working-set / accesses(block-refs)`; the three that overlap
must match:

```text
convert:    records 6109  sessions 4238  distinct keys 1106136  key references 1962304
simulator:  requests=6109  accesses(block-refs)=1962304  working-set(distinct blocks)=1106136
```

Its default cache sizes are 256–4096 **blocks** against a working set of over a
million, so a low hit rate there says nothing about the workload. Scenario 6 is
where a hit rate means something.

## Scenario 4 — Drive the local node (needs Certus)

**Build both binaries, `--release`.** The local node goes through an agent too
(FR-079), and the generator launches it for you — but it has to exist, and it
has to be built from the same tree, or the handshake refuses it. Build them
together, every time: the provenance digest covers every crate here, so a
generator rebuilt after its agent is correctly refused.

```bash
cargo build --release -p workload-gen -p workload-node-agent
```

A debug `workload-gen` against a live server is slow enough that a
5-virtual-second run of the shipped example does not finish in ten minutes, and
every figure it would print is a figure about the debug build.

**`--lanes` may not exceed the node's channel count.** It is now *connections
to the agent*, one mailbox channel each. Read the count off the running server
rather than guessing:

```bash
ps -eo args | awk '$1 ~ /certus-server-yaml$/'   # note --channels
```

```bash
../../target/release/workload-gen run $E --seed 42 \
  --lanes 8 --batch-keys 64 \
  --until 5 --report /tmp/run.json
```

No `--instance` and no `./cluster.yml`, so this drives **this** host — through
an agent the generator starts as a child process, with no ssh and no setup. Its
output goes to `/tmp/workload-node-agent.<port>.log`, which is the first place
to look if a run reports that no agent accepted a connection.

`run` is **unbounded** without `--until`, and stops cleanly on SIGINT or
SIGTERM; an interrupted run that kept its schedule is still valid (FR-074).

**This run is paced by default** (FR-080), which is the mode most people want
and the one that costs real time: at `--rate 1.0` a run takes wallclock equal
to its virtual span, so `--until 5` is five seconds and `--until 3600` is an
hour. The generator prints the projection before it starts, so silence is never
mistaken for a hang. For the throughput ceiling instead, `--rate inf` — an
infinite rate makes every turn due immediately, which is what work-conserving
means. Note that a large finite rate is *not* the same thing: it keeps a
schedule the machine cannot meet and is correctly reported invalid for
lateness.

**Expect**: sustained traffic, and a terminal summary plus `/tmp/run.json`
carrying the **mode**, throughput (keys/s, bytes/s,
virtual-seconds-per-wallclock-second), latency p50/p90/p99/max **per
operation**, the schedule it kept (rate asked for, rate achieved, lateness
percentiles with their turn count), plan-queue depth as **minimum and
fraction-at-zero**, lane utilisation, validity, and the reproduction
parameters.

**The two modes are not comparable, and the report says which one ran.** A
paced throughput is capped by the rate you asked for; a work-conserving one is
a ceiling. The same toy description on one machine gives a valid paced run and
an *invalid* work-conserving one — too few turns to keep eight lanes fed —
which is both modes' validity rules working, not a contradiction.

**The validity check is the point.** Force an invalid run and confirm it is
labelled, not published:

```bash
../../target/release/workload-gen run $E --seed 42 --lanes 512 --report /tmp/bad.json
echo "exit=$?"
```

**Expect**: exit **2**, before anything is issued, naming both figures —
`refusing to run: 512 lanes against a node with 8 channels`. The mailbox is
depth-1 per channel, so over-subscription would not add concurrency; it would
serialise behind a claimed channel and report a throughput for a concurrency
the run never had. Separately, a run whose plan queue reaches zero must exit
**3** with the report saying invalid, because a sweep driver that reads "exited
0" as "I have a data point" is precisely how an invalid number gets published.

**A claimed channel outlives the process that claimed it.** If a run fails with

```text
could only claim 4 of 8 channels; another client holds the rest
```

the holder may well be a **dead** process: the claim lives in the mailbox's
shared memory and nothing reaps it, so the count only recovers when the server
restarts.

Ordinary exits and panics now release, and a refused run stops the agent it
launched rather than leaving it to be killed mid-release — those three paths
were each a leak, and each was found by running this scenario five times in a
row. A `SIGKILL` still leaks, because nothing a process contains survives one.
So if that message appears, check for a live agent first (`ps -eo pid,args |
grep workload-node-agent`) and restart the server if there is none. Do not work
around it by lowering `--lanes`: the run would then measure a different
concurrency from the one asked for, which is the failure the refusal exists to
prevent.

**Confirm the op stream is faithful** (FR-039..FR-044): the issued sequence
must be reserve → transfer → commit for stores, must never contain the
single-shot store operation, must send reference reports without requesting
promotion, must poll for events, and must never send removal, pinning, or
promotion. This is checked against the production client's own sequence, not
against intuition.

## Scenario 4b — Rate sweep: what this machine can serve on time

The rate is the load knob, so sweeping it is the capacity measurement. Offered
load scales with the rate while the workload's *shape* does not — the same keys
in the same order, played faster — so the rate at which lateness leaves zero is
where this machine stops serving this workload on time.

```bash
UNTIL=60 scripts/rate-sweep.sh $E 1 2 5 10 20 50
```

**Expect** a table of rate against lateness percentiles, and one figure to read
off it: **the last rate whose runs are all valid**. Above it the generator
stopped keeping its own schedule, so those runs' latency describes its backlog
rather than Certus.

This answers "can this machine serve this workload" in a way the
work-conserving ceiling cannot. The ceiling reports the latency of a
**saturated** queue — a queue no real workload forms — which is exactly why it
is a good bandwidth number and a poor latency one.

Two ways to read it wrong, both worth naming. A rate whose lateness is non-zero
but inside tolerance is the *interesting* neighbourhood, not a failure: that is
the knee, and it is where a policy or configuration change shows up first. And
if **every** rate is invalid, the sweep is above what the machine can serve at
all, which makes every turn due immediately — work-conserving wearing a rate's
name.

The sweep refuses to run with two `certus-server-yaml` processes on the host,
because a figure from a box with two servers belongs to whichever one the
mailbox path reached and is not a fact about either.

## Scenario 5 — Multi-node with migration (needs a cluster)

```bash
# on the driving host. --instance names a CERTUS INSTANCE, not a machine:
# host[:port][:mailbox], where a component that parses as a u16 is the port and
# one starting with / is the mailbox. A machine running several instances — one
# per NUMA domain, say — appears here several times (FR-081).
../../target/release/workload-gen run $E --seed 42 \
  --instance node5 --instance node7 \
  --lanes 8 --report /tmp/multi.json

# The same cluster with two instances per host, one per NUMA domain:
../../target/release/workload-gen run $E --seed 42 \
  --instance node5:7420:/dev/shm/certus-numa0 \
  --instance node5:7421:/dev/shm/certus-numa1 \
  --instance node7:7420:/dev/shm/certus-numa0 \
  --instance node7:7421:/dev/shm/certus-numa1 \
  --lanes 8 --report /tmp/multi.json

# Or put the deployment in a file and stop retyping it. ./cluster.yml is picked
# up automatically — and the run says so, and records its digest in the report:
../../target/release/workload-gen run $E --seed 42 --lanes 8 \
  --hardware cluster.yml --report /tmp/multi.json
```

**Two instances on one host are two independent caches**, so a session
migrating between them is a genuine cache miss and a legitimate experiment
rather than a no-op. Each needs its own port: two sharing one would have the
second agent's startup take the first for a leftover and shut it down, so the
run would drive one instance while reporting two. That is refused.

The generator starts each **remote** agent over ssh and stops it afterwards, so
nothing is launched by hand — the only difference from Scenario 4 is that a
local node is launched as a child process instead. One transport, one driver,
one teardown, whichever it is.

To drive daemons an operator manages themselves, add `--no-launch` and start
them like this — noting `--bind` and `--port` as two flags, and that
`--block-bytes` must match the description's `blocks.bytes` or the handshake
refuses the node:

```bash
# on each remote node, only with --no-launch:
workload-node-agent --bind 0.0.0.0 --port 7420 \
  --shm-path /dev/shm/certus-shmq --lanes 8 --block-bytes 65536
```

Under `--no-launch` the daemons are yours: a refused run leaves them running,
because stopping one would leave you with nothing to talk to and nothing able
to start it again.

**Expect**: sessions distributed uniformly; a migrated session's turns issued
to a different node while its blocks stay where they were, so the new node
fetches remotely (SC-007).

**Two failure paths must be exercised, not assumed:**

```bash
# 1. stale agent: deploy a different build on one node
#    -> generator exits 4 and NAMES the node, before issuing anything (FR-051)
# 2. node lost mid-run: kill an agent during a run
#    -> generator aborts, exits 3, reports invalid, names the node (FR-064)
```

The second is the one people get wrong by continuing on the survivors. A lost
node makes its sessions' prefixes unreachable, shrinks the migration target
set, and redistributes load — so continuing yields a number for a different
experiment.

## Scenario 6 — The acceptance test for the reuse structure

This is what the tool exists for, and the two criteria most likely to fail
silently (SC-005, SC-006).

The check itself needs no server, no accelerator and no cluster — it replays
the plan's own reference stream through an LRU simulator — so it runs as an
ordinary test:

```bash
cargo test -p workload-gen --features live --test sweep
cargo test -p workload-gen --features live --test sweep -- --ignored  # 5 seeds, ~2 min
```

**Expect**: the **cross-session** hit rate rises monotonically across five
points spanning a hundredfold capacity range, with **no single step
contributing more than half the total rise**. The same check applied to a
description with `selection` removed must **fail** — if it passes, the
popularity machinery is not wired in and the sweep is measuring nothing.

Three things about that sentence were wrong when this scenario was first
written, and each of them made the check pass a workload that discriminates
nothing. They are worth knowing before writing a description to sweep:

- **Sweep cross-session reuse, not the hit rate the report prints.** A turn
  re-offers its whole prefix (FR-072a), so most references are a session
  re-reading what it stored moments ago. Those miss below the concurrent
  footprint and hit above it — a cliff, ~85% of the rise in one step, for
  concentrated and uniform popularity alike.
- **Keep the top of the ladder well below the key space** (0.1%-10% of it). A
  cache the size of the key space evicts nothing, so its hit rate is 1.0 for any
  workload whatsoever.
- **Scale-free popularity is not sufficient.** With `turns` and `uses.count`
  held constant every session has the same footprint, so the working set has one
  characteristic size and the curve cliffs there however the pool is selected.
  Both must be heavy-tailed, and `selection` needs a *scale-free* shape —
  `exponential` gives one hot band and steps; an `empirical` ladder
  `[1, 4, 16, 64, ...]` with `interpolate: true` puts equal mass in each octave
  of rank, which is a power law.

To sweep a real Certus instead of the simulator, restart the server at each
capacity — the cache size is a server setting, not a generator flag — and read
`cache.check_resident` against `cache.check_miss` from each report:

```bash
for cap in 1 4 16 64 256; do
  # restart certus-server-yaml with a memory tier of ${cap}GiB, then:
  cargo run -p workload-gen --features live -- run $E --seed 42 \
    --clear-cache --report /tmp/sweep-$cap.json
done
```

Then re-run with `rank_by: recency` instead of `slot` and compare two eviction
policies: **the ranking of the two policies must reverse**, significantly
across repeated runs. If it does not, the tool cannot discriminate between
policies and the eviction-testing goal is unmet however good the throughput
numbers look.

## Hit-rate comparisons need repetition

Mint races are preserved deliberately (FR-035, FR-036): two sessions may race
for the same shared prefix and both store it. So hit/miss outcomes are **not**
reproducible even though the plan is, and any A/B comparison needs repeated
runs with a stated significance test. Recorded history on this hardware
includes n=3 sampling producing conclusions that later measurement reversed;
use n ≥ 8.

## Always run from `apps/workload-generator`

Both `./.specify/` (repo root) and `apps/workload-generator/.specify/` exist
with identically-named scripts. The repo-level one resolves the feature
directory from the **git branch name** and then `mkdir -p`s it, so running a
speckit skill from the repo root creates a stray
`specs/synthetic-workload-generator-rewrite/` at the top of the repository.
Verify before any speckit command:

```bash
.specify/scripts/bash/check-prerequisites.sh --json --paths-only
# BRANCH must read 001-synthetic-workload-generator
```
