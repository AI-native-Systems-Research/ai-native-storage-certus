# Quickstart: Synthetic KV Workload Generator

**Phase 1 output** · the shortest path from a preset to a report.

Everything on this page except § Against a real server runs with no SPDK, no GPU, no RDMA, and no
columnar dependency — that is what SC-012 is about.

## 1. Characterise a workload without running anything

```sh
certus-workload plan --config presets/mixed.yaml -o /tmp/conv.plan
certus-workload report -p /tmp/conv.plan
```

`report` prints what the workload *is*, all of it computed from the plan alone: the reuse-distance
CDF, the compulsory-miss floor, the prefix-sharing depth histogram, request-length percentiles,
unique keys over time, realised trunk width and occupancy per depth, and the realised working-set
size over `run.wss_window`.

**Read the floor first.** It is the miss rate at unbounded capacity, so it is the best any consumer
could possibly do on this workload. If it is close to 1.0, no cache helps and there is nothing to
measure — knowable before you spend a minute of hardware time.

**Then read occupancy.** If `occupancy(p99(shared_depth))` is below 4.0 you will get a warning, and
below 1.0 the plan is rejected: sessions would land on virgin trunk and realise far less sharing than
the model asks for. That check is the one that catches a config which is internally consistent,
passes everything else, and still doesn't measure what it claims to.

## 2. A minimal workload from scratch

Ten lines is a real workload:

```yaml
version: 1
seed: 0xC0FFEE
duration: 120s
corpus:
  block_bytes: 128KiB
  trees:
    roots: {count: 12, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: lognormal, median: 18, sigma: 0.6}
    branching: auto
workload:
  arrival: {model: open_loop, rate: 4000/s}
  sessions:
    turns: {dist: geometric, mean: 6}
    think_time: {dist: lognormal, median: 3s, sigma: 1.1}
    private_depth: {dist: lognormal, median: 8, sigma: 0.8}
    growth_per_turn: {dist: lognormal, median: 6, sigma: 0.5}
run: {mode: hardware, batch_size: 64, workers: 8, warmup: 30s, wss_window: 240_000}
```

**There is no `system:` section, and adding one is an error.** Capacities, eviction policy,
watermarks and pinning are properties of whatever *consumes* this workload, not of the workload. If
you want a cache at a quarter of the working set, read the working-set size out of the report and
configure your consumer with it.

## 3. Emit a trace file

```sh
certus-workload emit -p /tmp/conv.plan --blocks 50_000_000 -o /tmp/conv.jsonl
certus-trace convert /tmp/conv.jsonl -o /tmp/conv.parquet     # needs --features parquet
```

`--blocks` is **required** for file output and is refused without it. Blocks are the unit that
converts to a file size; `duration` and `requests` don't, because request length is drawn per
session — so a request cap leaves the output size at the mercy of the length distribution, and a long
duration at a high rate fills the disk.

Both files carry a `manifest.json` declaring `provenance: synthetic` and
`timestamp_is_synthetic: true`, so a generated trace is never mistaken for a measured one.

## 4. Fit a model from a real trace

```sh
certus-trace fit --trace /path/to/some-trace -o fitted.yaml
certus-workload plan --config fitted.yaml -o /tmp/fitted.plan
certus-trace validate --plan /tmp/fitted.plan --trace /path/to/some-trace
```

Trace collections are **not in this repository** — they are large and variously licensed, so you
supply a path. `fit` reads the trace's own `manifest.json` to learn what it can support and will:

- **refuse** a parameter whose source field is `unavailable` rather than defaulting it. A trace with
  no `session_id` cannot give you `turns` or `growth_per_turn`;
- **refuse** to fit from a partial trace, judged by record count against the manifest's
  `block_stats` — not by filename, since a `sample_` prefix is a convention rather than a guarantee.
  The sample files shipped beside real traces run to 6 records against ~2 million;
- **leave `churn.half_life` unset**, because a half-life longer than the trace's span is
  indistinguishable from no churn and any fitted value would be biased short;
- **leave every placement field unset**, because no known trace carries node attribution at all.

## 5. Check the tool against itself

```sh
certus-workload plan --config fitted.yaml -o /tmp/rt.plan
certus-workload emit -p /tmp/rt.plan --blocks 10_000_000 -o /tmp/rt.jsonl
certus-trace fit --trace /tmp/rt.jsonl -o /tmp/recovered.yaml
diff <(certus-workload plan --config fitted.yaml --print-normalised) \
     <(certus-workload plan --config /tmp/recovered.yaml --print-normalised)
```

This round trip is the strongest check the tool has, because ground truth is **exact** rather than
estimated: any divergence is a defect in `fit`, the emitter, or the reader — never a property of some
real workload. It is also the only test that exercises the emitter and the reader against each other.

What it cannot tell you is whether the model resembles reality. Only step 4 does that.

## 6. Against a real server

```sh
certus-workload-run run -p /tmp/conv.plan --endpoint localhost:50051
```

Needs a `full-p2p` or `full` profile server. Latency percentiles come out **per outcome class**,
because a p99 that mixes memory hits with disk reads describes nothing.

Two honest failure modes to expect rather than debug:

- A server predating serving-tier attribution returns `SERVED_BY_UNSPECIFIED`, and the report will say
  "attribution unsupported by server" rather than guessing a tier.
- Without `rw-telemetry` reaching the active dispatcher, the `GetIoStats` byte cross-check reports
  `unavailable` **with its reason**. A zeroed counter never reads as agreement.

For multi-node, `preflight` inspects every node and **refuses to run** a comparative measurement on
an asymmetric cluster, naming what differs. That converts a silent measurement confound into a loud,
actionable error — a NIC on one socket and a GPU on another has previously produced a 16% versus 2%
coefficient of variation on this bench.

## Where things live

| Want to | Run |
| --- | --- |
| Build a plan, characterise it, emit JSONL | `certus-workload` (`plan`, `report`, `emit`) |
| Fit, validate, convert to parquet | `certus-trace` (`fit`, `validate`, `convert`) |
| Drive a Certus server | `certus-workload-run run` (hardware only) |

`events.bin` inside a `.plan/` directory is the native artifact — fixed-width, indexable by ordinal,
streamable. JSONL and parquet are interchange, and both are producible from an existing plan without
regenerating it.
