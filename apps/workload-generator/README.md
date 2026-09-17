# workload-generator

A synthetic workload generator for Certus: it turns a short YAML *description*
of an inferencing workload into either a trace file or live load against a
running Certus cluster.

Start with [`specs/001-synthetic-workload-generator/quickstart.md`](specs/001-synthetic-workload-generator/quickstart.md)
for the scenarios, and read this file before running any comparison whose
conclusion depends on a hit rate.

## What it is for

Certus caches KV blocks for inferencing, and the questions worth asking of it —
does this eviction policy beat that one, how much of the working set has to fit
before the hit rate moves, what does a remote tier cost — all need a workload
that has the *reuse structure* real inferencing traffic has. Replaying a
captured trace gives you one workload at one scale. This gives you a family of
them: a description names distributions, and the generator turns them into
sessions whose turns share prefixes the way conversations and agent runs do.

Two things follow from that, and they are the whole design:

- **The description is small and the plan is large.** A few dozen lines of YAML
  expand into an unbounded stream of operations. You tune the workload by
  editing distributions, not by recording a longer trace.
- **The plan is deterministic and the outcome is not.** A description plus a
  seed fixes every session, turn and key, on every machine, independent of
  batching and lane count (FR-072). What is *not* fixed is whether a given
  reference hits, because the cluster races. That asymmetry is what makes the
  tool usable for A/B work at all, and it is also the trap — see
  [Comparing two eviction policies](#comparing-two-eviction-policies) below.

Non-goal: it is not a trace replayer and not a fidelity model of any particular
captured trace. Nothing here claims a generated workload *is* some real one.

## The five crates

The split is not organisational. Three crates are CUDA-free and are workspace
default members; two link CUDA and are not. That boundary is what makes "the
plan is identical across the live and emit paths" a property the compiler
enforces rather than one a reviewer has to check, because the simulation core
cannot see a mailbox, a device or an output container.

| crate | default member | what it owns |
| --- | --- | --- |
| `workload-model` | yes | the simulation core: YAML schema and validation, the five distribution kinds, populations and residual seeding, selection over rank, the key chain, sessions and turns, the virtual-time loop, and the canonical `OperationPlan` |
| `workload-trace` | yes | output containers (JSONL, parquet behind a non-default feature), the self-describing manifest, and the Mooncake / libCacheSim / cache-simulator projections |
| `workload-wire` | yes | the generator↔agent TCP protocol: framing, the `Hello` handshake, and its conformance cases |
| `workload-gen` | **no** | the `workload-gen` binary — CLI, execution lanes, the CUDA payload path, and the structured report |
| `workload-node-agent` | **no** | the per-node relay daemon for multi-node runs. It holds no simulation state and depends on `workload-gen`'s *library*, so FR-072a's reactive rule has exactly one implementation |

The two binaries are excluded from `default-members`, so a plain `cargo build`
or `cargo test` at the repository root never reaches CUDA. Build them
explicitly:

```bash
cargo build -p workload-gen
cargo build -p workload-node-agent
```

## The emit-only build

`workload-gen`'s `live` feature is default-on and gates both the mailbox
transport and the CUDA link. Turning it off yields a tool that needs no
accelerator, no server and no cluster:

```bash
cargo build -p workload-gen --no-default-features
```

That build drops the `run` subcommand and keeps the other four — `emit`,
`convert`, `validate` and `plan` — which is enough to write a trace, project it
into another tool's format, check a description, and produce the canonical plan
serialisation that reproducibility is asserted against. Quickstart Scenarios 1–3
and the whole of User Story 2 run on it, on any machine.

Add `--features parquet` for the parquet container; it pulls the arrow family,
which is why it is not on by default.

## Is the generator fast enough?

It has to be, or every measurement made with it is a measurement of it. The claim
is that plan generation is never the bottleneck, and it is a measurement rather
than an argument:

```bash
cargo bench -p workload-model --bench plan
```

Baseline on this box, 2026-09-17, one thread, per **key reference** — the unit
Certus's own cost is quoted in:

| Case | Per key | Against a ~5.6 µs/key server |
| --- | --- | --- |
| `short_sessions` (5 turns) | 11.4–12.3 ns | 0.22% |
| `long_sessions` (40 turns) | 4.2–5.7 ns | 0.08% |
| `many_shared` (2000 instances, draw 16) | 5.25–5.45 ns | 0.10% |

So one generator thread has 450x to 1300x of headroom, and a plan queue that
reaches zero means something is wrong rather than that generation is slow. Long
sessions are *cheaper* per key than short ones, by 2–2.9x: per-turn overhead
amortises over a longer prefix. If that inverted it would mean something in the
turn path had become superlinear, which is what the benchmark is for.

`projection_10k_sessions` runs in 97.5–112 µs, so `validate --until n` stays an
interactive query.

**Those are ranges because a single run of this benchmark is not a measurement.**
Across three identical runs with no code change, `long_sessions` moved +21% and
then +12%, each at p = 0.00, and the first run of a session had a 47% outlier
rate. Run it twice and discard the first; treat under ~35% on `long_sessions`, or
~8% on the others, as noise. For a real before/after use repetition and a stated
test, exactly as for hit rates below. `benches/plan.rs` carries the full record.

## Where to go next

- [`quickstart.md`](specs/001-synthetic-workload-generator/quickstart.md) — six
  scenarios, from validating the shipped example with no hardware to a
  multi-node run with session migration. Run its commands from *this*
  directory.
- [`contracts/`](specs/001-synthetic-workload-generator/contracts/) — normative:
  the CLI, key derivation, trace I/O and interop, the node-agent wire protocol,
  and `workload-input.example.yml`, the description a test keeps parseable.
- [`data-model.md`](specs/001-synthetic-workload-generator/data-model.md) — the
  description schema field by field.
- [`spec.md`](specs/001-synthetic-workload-generator/spec.md) and
  [`plan.md`](specs/001-synthetic-workload-generator/plan.md) — requirements and
  the design decisions behind the crate split.
- [`research/population/`](research/population/) — the population simulation
  behind FR-013 to FR-016, landed so those measurements are reproducible here
  rather than external.

## Comparing two eviction policies

This is what the tool exists for, and it is the measurement most easily got
wrong, because every way of getting it wrong produces a plausible number rather
than an error.

### Hit-dependent comparisons are not reproducible, by design

The plan is deterministic: the same description and seed produce the same
sessions, the same turns and the same keys, every time, on every machine. **Hit
and miss outcomes are not.** Two sessions may race for the same shared prefix
and both store it — mint races are preserved deliberately (FR-035, FR-036),
because a real cluster has them and a generator that serialised them would
measure a workload nobody runs.

So a single run's hit rate is a sample, not a measurement. Any A/B comparison
needs **repeated runs and a stated significance test** — state which test, and
state it before looking at the numbers.

**Use n >= 8.** That is the recorded floor on this hardware, not a rule of
thumb: this repository's history includes an n=3 comparison whose conclusion
later measurement reversed, and a 16% run-to-run variance that turned out to be
cross-socket DMA rather than noise. Two runs that differ are not evidence of
anything.

Also, before quoting any figure:

- **Check that exactly one server is running.** `ps -eo args | awk '$1 ~
  /certus-server-yaml$/'`. Two servers on one mailbox invalidated an entire
  decline-rate series here, silently, because the second one answered some of
  the requests.
- **Never quote a percentile without its n.** A p50 over 20 requests is not a
  p50; `QUOTABLE_REQUESTS` is 100 and the report will tell you when it is below
  that. A "TOUCH is 3x slower than CHECK" finding was retracted for exactly this.
- **Restart the server before quoting throughput**, and pass `--clear-cache` if
  the comparison is meant to start cold — a warm tier from the previous arm is
  the most common way to measure the previous arm twice.
- **Rebuild and redeploy the node agent after any change under `src/`.** The
  handshake refuses a mismatched build (FR-051) on purpose; that refusal is the
  cheap version of this mistake.

### Sweeping cache capacity

Sweeping capacity answers a different question from comparing policies: it asks
whether the workload *can* discriminate between policies at all. A workload whose
hit-rate curve steps — flat, cliff, flat — cannot, because both policies score
the same on either side of the cliff.

Run the check, which needs no server, no accelerator and no cluster:

```bash
cargo test -p workload-gen --features live --test sweep
cargo test -p workload-gen --features live --test sweep -- --ignored  # 5 seeds
```

Expect the **cross-session** hit rate to rise monotonically across five
capacities spanning a hundredfold range, with no single step contributing more
than half the total rise, and the same check to **fail** when `selection` is
removed from every class.

Three things matter when writing a description to sweep, each of which was
learned by measuring a check that wrongly passed a uniform workload:

- **Sweep cross-session reuse, not the hit rate the report prints.** A turn
  re-offers its whole prefix (FR-072a), so most references are a session
  re-reading what it stored moments ago; those miss below the concurrent
  footprint and hit above it, putting ~85% of the rise into one step whatever
  the popularity distribution is.
- **Keep the top of the ladder below the key space.** A cache the size of the key
  space evicts nothing, so its hit rate is 1.0 for any workload at all — and it
  is the one capacity at which no policy can differ from another.
- **Both popularity and the working set must be scale-free.** `exponential`
  selection gives one hot band and therefore one cliff; an `empirical` ladder
  `[1, 4, 16, 64, ...]` with `interpolate: true` puts equal mass in each octave
  of rank, which is a power law. And with `turns` and `uses.count` held constant
  every session has the same footprint, so the working set has one characteristic
  size and the curve cliffs there however the pool is selected.

To sweep a real Certus, restart the server at each capacity — the cache size is a
server setting, not a generator flag — and read `cache.check_resident` against
`cache.check_miss` from each report.

### The ranking test

Holding the workload and the capacity fixed, switching `rank_by: slot` to
`rank_by: recency` must **reverse** the measured ranking of two eviction
policies, significantly across repeated runs (SC-006). `slot` attaches
popularity to a position, so a hot block stays hot for its whole life; `recency`
attaches it to newness, so blocks go cold with age. A policy that exploits
long-lived popularity should win the first and lose the second.

If the ranking does not reverse, the tool cannot discriminate between policies
and the eviction-testing goal is unmet **however good the throughput numbers
look**. That is the point of the criterion: throughput is easy to measure and
tells you nothing about whether the comparison was meaningful.

### Figures that do not combine across nodes

On a multi-node run, be careful which numbers you add:

| quantity | across nodes |
| --- | --- |
| counters (hits, misses, blocks moved) | **sum** — exact |
| latency percentiles | **never average** — merge the histograms, then take the percentile |
| distinct keys | **do not sum** — a shared block referenced from two nodes is one key |

The report merges histograms rather than averaging quantiles, and reports
`distinct_keys` as the generator's own count for this reason. Averaging two
nodes' medians produces a figure belonging to no distribution; adding two nodes'
distinct-key counts double-counts every shared block. They are the same
arithmetic error one type apart.
