# KV-offload profile tools

## `render_kvprofile.py` — profile → PNG slide

Turns one or more `profile_all.sh` run directories into a single PNG, stacked as:

1. a **total-wall-time bar chart** (with each offload variant's ratio vs the
   NoOffload baseline),
2. a set of **run-total family panels** — every counter rolled up to its
   whole-run total and grouped onto one shared axis per family (**Tokens**,
   **Prefix-cache queries & hits**, **Bytes moved**, **KV tier movements**). Each
   bar is annotated with a **per-second average** (bytes/s for the byte families,
   count/s otherwise), and each cache-hit bar additionally carries its **hit rate**
   (hits ÷ queries). A family with no nonzero total across all variants is dropped,
3. a **GPU processor-utilization bar chart** — one bar per variant, the **mean**
   `nvidia-smi util.gpu` (GPU *processor* busy-%, not KV-cache/memory occupancy)
   over that variant's window, annotated with its **p95** and **peak**; followed
   by a **GPU-utilization-over-time line panel** (util% vs elapsed within each
   variant's window, a 10 s moving average since the raw signal bounces 0↔100).
   Both are dropped when the run carries no GPU telemetry,
4. a **small-multiples grid** of the same counters plotted **per round**.

PNG only — no HTML, no browser. The only dependency is `matplotlib` (system
`python3` is fine).

### What it reads

Each argument is a `profile_all.sh` run directory (the `kvprofile-*` dir it
writes). Per directory, in priority order:

- **`results.json`** — the authoritative index of `{variant, wall_s, status,
  log}`. This is where each variant's display name and total wall time come
  from (notably the *only* place Certus-SPDK's wall lives — its log has no
  `[run] done` line).
- **`<variant>.log`** — the teed driver stderr. Per-round
  `[prom] round N: k=v ...` lines are parsed for the counter deltas. If
  `results.json` is missing, wall time falls back to the log's
  `[run] done. wall=Xs` line.
- **`server.log`** (Certus-SPDK only) — the certus-server's own log. Its periodic
  `tier-events promotions[->memory M, ->gpu G] evictions[memory E, ssd S]` lines
  (and the `FINAL tier-events` summary) give the cumulative KV tier-movement
  counts. These feed the "KV tier movements" family and the four Certus-only
  per-round tier panels; absent → those are dropped.
- **`gpu-timeline.csv` + `gpu-markers.csv`** — `profile_all.sh`'s `nvidia-smi`
  sampler: per-tick `util.gpu`/clock/mem/power, plus each variant's start/end
  window. Parsed via `gpu_report.py` and reduced to per-variant mean/p95/peak GPU
  processor utilization for the GPU band (same numbers as `gpu-summary.txt`).
  Absent → that band is dropped.

The per-round counters only exist if the run was captured with metrics on —
which is the **default** in all four drivers (`CAPTURE_METRICS=1`). A run made
with `CAPTURE_METRICS=0` still renders a wall-time bar chart, just with no
per-round grid for those variants.

### Usage

```bash
# one full 4-way run
tools/render_kvprofile.py /mnt/fs-backend-bench/kvprofile-vllm0.26.0-225237_16222

# overlay three repeats of one variant, with custom legend tags
tools/render_kvprofile.py \
    run1=/…/kvprofile-…-225237_16222 \
    run2=/…/kvprofile-…-105057_44497 \
    run3=/…/kvprofile-…-110404_47600 \
    --variants tiered-cpu-fs -o tiered-3way.png
```

Each `RUN` argument is either a bare run directory, or `TAG=DIR` to set an
explicit short legend tag. When the same variant appears in more than one
directory, each instance gets its own line — same colour, cycling
solid → dotted → dashed → dash-dot — with the run tag in the legend so they
stay distinguishable.

### Options

| Flag | Default | Meaning |
|---|---|---|
| `RUN [RUN ...]` | — | One or more run dirs, or `TAG=DIR` forms (required) |
| `-o`, `--out` | `kvprofile-slide.png` | Output PNG path |
| `--title` | `KV-offload profile` | Slide title |
| `--subtitle` | — | Second header line (free text) |
| `--variants` | all | Comma-separated subset to plot, e.g. `nooffload,tiered-cpu-fs` |
| `--color` | — | `TAG=HEX` override for a variant/run colour; repeatable |
| `--dark` | off | Dark theme |
| `--dpi` | `200` | Output resolution |

### Counters plotted

vLLM Prometheus counters (from the `[prom]` lines):
`prompt_tokens`, `prompt_tokens_cached`, `generation_tokens`,
`prefix_cache_queries`, `prefix_cache_hits`, `external_prefix_cache_queries`
(offload-tier queries), `external_prefix_cache_hits` (offload-tier hits),
`kv_offload_store_bytes`, `kv_offload_load_bytes`, `num_preemptions`.

Certus-SPDK NVMe device bytes (from the shmq driver's `[prom]` line, real only
when the server is built `--features rw-telemetry`): `ssd_read_bytes`,
`ssd_write_bytes`.

Certus-SPDK KV tier movements (from `server.log`, cumulative):
`tier_promotions_to_memory` (SSD→DRAM), `tier_promotions_to_gpu`,
`tier_evictions_from_memory`, `tier_evictions_from_ssd`.

Each appears both as a per-round small multiple and — grouped by family — as a
run-total bar with its per-second average (and hit rate, for the cache-hit
counters). A counter absent from every series is dropped automatically, so a
write-only run or a non-Certus backend simply omits the counters it never emits.

Variant colours are fixed (NoOffload blue, CPUOffload orange, Tiered-CPU-FS
green, SharedStorage teal, Certus-SPDK gold) and the bar/legend order is
canonical, so slides across runs stay visually comparable.

## `gen_synthetic_sharegpt.py` — synthetic ShareGPT dataset generator

Generates **N** synthetic conversations with a mean **human-turn** count of **M**,
written in the exact ShareGPT schema the multi-turn bench consumes (a JSON array of
`{"id", "conversations": [{"from": "human"|"gpt", "value": ...}]}` — see
`../../../data/sharegpt/README.md`). Use it to produce controllable stand-ins for
`data/sharegpt_12turn_450.json` at any size or turn distribution without pulling
from the real corpus.

Each conversation is authored in a **single** Claude API call returning a JSON object
(not one call per turn), so cost scales with N, not N×M. The response is parsed
tolerantly — markdown fences, surrounding prose, and truncated output are all handled
— so the tool works with older `anthropic` SDKs (0.x) and LiteLLM-style proxy gateways
that don't support the structured-output API. Human-turn counts are drawn from a
**Poisson(M)** distribution with a seeded RNG, so a given `--seed` reproduces the same
turn-count distribution and ids. Output strictly alternates `human → gpt` starting with
`human` (roles are re-asserted by position), always has an even turn count and ≥ 2
human turns, so every conversation is kept by `load_convs`.

`--max-tokens` auto-scales with the requested turn count, but very large `M` (say,
≳ 30 human turns = 60+ messages in one response) strains single-call generation: the
model may stop early or truncate, so the realized mean can run below `M`. The stats
report a `truncated` count and the realized mean; lower `M` or raise `--max-tokens` if
you need the count to hold exactly.

### Setup

```bash
pip install anthropic
export ANTHROPIC_API_KEY=...      # or: ant auth login
```

### Usage

```bash
# Mirror data/sharegpt_12turn_450.json: 450 convs, mean 12 human turns
python tools/gen_synthetic_sharegpt.py -n 450 -m 12 -o data/synth_12turn_450.json

# Preview the turn-count distribution + rough cost, no API calls
python tools/gen_synthetic_sharegpt.py -n 450 -m 12 -o /tmp/x.json --dry-run

# Larger, cheaper/faster bulk run
python tools/gen_synthetic_sharegpt.py -n 2000 -m 8 \
    --model claude-sonnet-5 --concurrency 12 -o data/synth.json
```

Point a driver at the result with `DATASET_PATH=data/synth_12turn_450.json` (see
`../../../data/sharegpt/README.md`). Partial output is flushed on Ctrl-C or a
mid-run error; per-conversation failures and refusals are counted and skipped.
A usage + estimated-cost summary is printed to stderr at the end.

### Options

| Flag | Default | Meaning |
|---|---|---|
| `-n`, `--num-convs` | — | Number of conversations to generate (N, required) |
| `-m`, `--mean-turns` | — | Mean human-turn count (M); actual counts are Poisson-distributed (required) |
| `-o`, `--out` | — | Output ShareGPT `.json` path (required) |
| `--model` | `claude-opus-5` | Claude model id; `claude-sonnet-5` / `claude-haiku-4-5` are cheaper for bulk |
| `--concurrency` | `8` | Max concurrent API requests |
| `--max-tokens` | `16000` | Floor for max output tokens per conversation; auto-raised for high turn counts (capped at 64000) |
| `--min-turns` | `2` | Floor for human-turn count (clamped to ≥ 2, the bench minimum) |
| `--max-turns` | `40` | Ceiling for human-turn count |
| `--seed` | `1234` | RNG seed for turn-count distribution, ids, and topic selection |
| `--topics-file` | built-in | File of topics, one per line, for conversation diversity |
| `--indent` | compact | Pretty-print with this indent (default matches the repo's compact sharegpt files) |
| `--dry-run` | off | Print distribution + rough cost estimate, make no API calls |

The only dependency is the `anthropic` SDK (system `python3` is fine).
