# KV-offload profile tools

## `render_kvprofile.py` — profile → PNG slide

Turns one or more `profile_all.sh` run directories into a single PNG: a
total-wall-time bar chart plus a small-multiples grid of per-round vLLM
Prometheus counters. PNG only — no HTML, no browser. The only dependency is
`matplotlib` (system `python3` is fine).

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
| `--dark` | off | Dark theme |
| `--dpi` | `200` | Output resolution |

### Counters plotted (per round)

`prompt_tokens`, `prompt_tokens_cached`, `generation_tokens`,
`prefix_cache_queries`, `prefix_cache_hits`, `external_prefix_cache_queries`
(offload-tier queries), `external_prefix_cache_hits` (offload-tier hits),
`kv_offload_store_bytes`, `kv_offload_load_bytes`, `num_preemptions`.

Variant colours are fixed (NoOffload blue, CPUOffload orange, Tiered-CPU-FS
green, SharedStorage teal, Certus-SPDK gold) and the bar/legend order is
canonical, so slides across runs stay visually comparable.
