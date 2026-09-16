# workload-generator

A synthetic workload generator for Certus: it turns a short YAML *description*
of an inferencing workload into either a trace file or live load against a
running Certus cluster.

Start with [`specs/001-synthetic-workload-generator/quickstart.md`](specs/001-synthetic-workload-generator/quickstart.md)
for the scenarios, and read this file before running any comparison whose
conclusion depends on a hit rate.

> Orientation — purpose, the five crates, and the emit-only build — is T081 and
> is not written yet. What follows is T080: how to compare two eviction policies
> without fooling yourself.

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
