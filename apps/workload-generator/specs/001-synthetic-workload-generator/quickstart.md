# Quickstart & Validation Guide: Synthetic Workload Generator

**Feature**: `001-synthetic-workload-generator` | **Date**: 2026-09-15

Runnable scenarios that prove the feature works, ordered so that each needs
strictly more hardware than the last. Scenarios 1–3 need **no accelerator, no
Certus, and no cluster** — they are the bulk of the acceptance surface.

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
field whose truncated value differs from what was written. The example is
deliberately informative here: `doc_analysis` draws `long_document` counts from
`exponential(mean 5)` against a pool of 20, so the implied maximum discards
~1.8% of the mass and the effective mean is ~5.57 rather than 5. That line
appearing is the check working (FR-003).

**Also expect it to refuse a bad file.** Lower that pool to 10 and re-run: the
implied maximum then discards 13.5%, over the 5% threshold, so it exits **2**
and names both the requested mean (5) and the effective one (4.218) — see
FR-004. Nothing is issued.

## Scenario 2 — Determinism, and independence from tuning (no hardware)

The executable form of FR-072 and SC-003, and the single most important test
here.

```bash
E=specs/001-synthetic-workload-generator/contracts/workload-input.example.yml
G="cargo run -q -p workload-gen --no-default-features --"

$G plan $E --seed 42 --output /tmp/plan-a.bin
$G plan $E --seed 42 --output /tmp/plan-b.bin
$G plan $E --seed 42 --batch-keys 1024 --lanes 64 --output /tmp/plan-c.bin
cmp /tmp/plan-a.bin /tmp/plan-b.bin && cmp /tmp/plan-a.bin /tmp/plan-c.bin
```

**Expect**: both comparisons silent. The third is the one that catches real
bugs — if batching or lane count leaks into the plan, the trace stops being a
function of description and seed, and every generated trace becomes
unreproducible without recording the tuning that produced it.

```bash
$G plan $E --seed 43 --output /tmp/plan-d.bin
cmp /tmp/plan-a.bin /tmp/plan-d.bin      # MUST differ
```

**Expect**: differs. A seed that changes nothing means the seed is not wired
through, which would look like determinism while being its opposite.

## Scenario 3 — Emit a trace, both containers, and feed the simulator

```bash
$G emit $E --seed 42 --until 3600 --format both --output /tmp/trace
ls /tmp/trace                            # manifest.json, invocations/, blocks/
```

**Expect**:

- `manifest.json` declares the encoding (`source_class: pre_hashed`, full
  encoding), the block geometry, and `block_id_space` recording that
  identifiers are chained u64 keys rather than dense mint-order integers
  (FR-057).
- The JSONL and parquet records are **identical**, and every row satisfies the
  full-encoding invariants including the trailing-partial-block convention
  (SC-004, FR-058).
- The run report states completeness — sessions, turns, blocks, virtual-time
  span, records written — and **omits** latency, lane utilisation, and the
  virtual-to-wallclock ratio entirely. Their presence as zeros is a failure,
  not a cosmetic issue: a zero is indistinguishable from a measurement
  (FR-071).

Then the deferred integration question, now settled (`research.md` D1):

```bash
$G convert /tmp/trace --to simulator --output /tmp/trace-sim.jsonl
cargo run -p eviction-replay-benchmark -- --trace /tmp/trace-sim.jsonl
```

**Expect**: the simulator loads it and reports a hit-rate curve. Its loader
derives sessions by walking `parent_chat_id` to a root, so a converted trace
whose `parent_chat_id` chain is wrong will still *load* while collapsing every
session into one — check that its reported distinct-key count and session count
match the emit report rather than only checking that it ran.

## Scenario 4 — Drive the local node (needs Certus)

```bash
cargo run -p workload-gen -- run $E --seed 42 \
  --shm-path /dev/shm/certus-shmq --lanes 16 --batch-keys 64 \
  --report /tmp/run.json
```

**Expect**: sustained traffic, and a terminal summary plus `/tmp/run.json`
carrying throughput (keys/s, bytes/s, virtual-seconds-per-wallclock-second),
latency p50/p90/p99/max, plan-queue depth as **minimum and fraction-at-zero**,
lane utilisation, validity, and the reproduction parameters.

**The validity check is the point.** Force an invalid run and confirm it is
labelled, not published:

```bash
cargo run -p workload-gen -- run $E --lanes 512 --report /tmp/bad.json; echo "exit=$?"
```

**Expect**: `--lanes` beyond the server's channel count is rejected at startup
— the mailbox is depth-1 per channel, so over-subscription would silently
serialise. Separately, a run whose plan queue reaches zero must exit **3** with
the report saying invalid, because a sweep driver that reads "exited 0" as "I
have a data point" is precisely how an invalid number gets published.

**Confirm the op stream is faithful** (FR-039..FR-044): the issued sequence
must be reserve → transfer → commit for stores, must never contain the
single-shot store operation, must send reference reports without requesting
promotion, must poll for events, and must never send removal, pinning, or
promotion. This is checked against the production client's own sequence, not
against intuition.

## Scenario 5 — Multi-node with migration (needs a cluster)

```bash
# on each remote node, started by the launcher, not by hand:
workload-node-agent --shm-path /dev/shm/certus-shmq --listen 0.0.0.0:9420

# on the driving node:
cargo run -p workload-gen -- run $E --seed 42 \
  --node node5:/dev/shm/certus-shmq --node node7:/dev/shm/certus-shmq \
  --lanes 16 --report /tmp/multi.json
```

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
