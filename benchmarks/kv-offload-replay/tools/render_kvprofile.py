#!/usr/bin/env python3
"""render_kvprofile.py — extract KV-offload profile data and render a PNG slide.

Reads one or more `profile_all.sh` run directories (the `kvprofile-*` dirs it
writes), extracts each variant's total wall time and per-round vLLM Prometheus
counter deltas, and renders a single PNG stacked as: a total-wall-time bar
chart, a set of run-total family panels (each counter rolled up to its whole-run
total and grouped onto one shared axis per family — Tokens, Prefix-cache queries
& hits, Bytes moved, KV tier movements — with a per-second average and, for hit
counters, a hit rate annotated atop each bar), a GPU processor-utilization bar
chart (mean nvidia-smi util.gpu per variant) plus a GPU-utilization-over-time
line panel, and a small-multiples grid of the same counters plotted per round.
No HTML — PNG only.

Data sources, per run directory (in priority order):
  * results.json  — authoritative index: {variants:[{variant, wall_s, status,
                    log}, ...]}. Gives the display name + wall time (the only
                    place Certus-SPDK's wall lives; its log has no `[run] done`).
  * <variant>.log — the teed driver stderr; per-round `[prom] round N: k=v ...`
                    lines are parsed for the counter deltas. Falls back to the
                    `[run] done. wall=Xs` line for wall time if results.json is
                    absent.
  * server.log    — Certus-SPDK only: the certus-server's own log. Its periodic
                    `tier-events promotions[->memory M, ->gpu G] evictions[memory
                    E, ssd S]` lines (plus the `FINAL tier-events` summary) give
                    the cumulative KV tier-movement counts, rendered as four extra
                    Certus-only panels. Absent → those panels are dropped.
  * gpu-timeline.csv + gpu-markers.csv — profile_all.sh's nvidia-smi sampler
                    (per-tick util.gpu/clock/mem/power) plus per-variant start/end
                    windows. Parsed via gpu_report and reduced to each variant's
                    mean / p95 / peak GPU processor utilization for the GPU band.
                    Absent → that band is dropped.

Pass several directories to overlay several runs. When the SAME variant appears
in more than one directory (e.g. three Tiered-CPU-FS repeats) each gets its own
line — same colour, cycling solid -> dotted -> dashed -> dash-dot — and a run
tag in the legend so they stay distinguishable.

Usage:
  render_kvprofile.py RUN [RUN ...] [-o out.png] [options]

  RUN is a run directory, or TAG=DIR to set an explicit short legend tag.

Examples:
  # one full 4-way run
  render_kvprofile.py /mnt/fs-backend-bench/kvprofile-vllm0.26.0-225237_16222

  # three tiered repeats overlaid, custom tags
  render_kvprofile.py \\
      run1=/…/kvprofile-…-225237_16222 \\
      run2=/…/kvprofile-…-105057_44497 \\
      run3=/…/kvprofile-…-110404_47600 \\
      --variants tiered-cpu-fs -o tiered-3way.png

Requires: matplotlib (import-only; no browser).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import OrderedDict

# gpu_report.py (sibling of this tools/ dir) already parses the profile_all.sh GPU
# telemetry: gpu-timeline.csv (per-tick nvidia-smi util.gpu/clock/mem/power) sliced
# per variant by gpu-markers.csv. Reuse it so the GPU-utilization band shows the
# exact same numbers as `gpu-summary.txt`. Optional — a failed import just drops
# the band (older run dirs with no GPU telemetry render exactly as before).
_BENCH_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _BENCH_DIR not in sys.path:
    sys.path.insert(0, _BENCH_DIR)
try:
    import gpu_report as _gpu_report
except Exception:  # noqa: BLE001 - GPU band is optional
    _gpu_report = None

# ── Curated counters: (key, title, unit) in render order. Only these are
# plotted; the noisy ones (request_success, *_time, *_by_source, *_total dupes,
# kv_offload_total_bytes) are skipped. A counter absent from every series is
# dropped from the grid automatically.
COUNTERS = [
    ("prompt_tokens",                 "Prompt tokens processed",    "int"),
    ("prompt_tokens_cached",          "Cached prompt tokens",       "int"),
    ("generation_tokens",             "Generation tokens produced", "int"),
    ("prefix_cache_queries",          "GPU prefix-cache queries",   "int"),
    ("prefix_cache_hits",             "GPU prefix-cache hits",      "int"),
    ("external_prefix_cache_queries", "Offload-tier queries",       "int"),
    ("external_prefix_cache_hits",    "Offload-tier hits",          "int"),
    ("kv_offload_store_bytes",        "Bytes stored to tier",       "bytes"),
    ("kv_offload_load_bytes",         "Bytes loaded from tier",     "bytes"),
    # Certus-SPDK only: real NVMe device bytes per round, from the server's
    # rw-telemetry (read_write_stats) queried over the shmq ring's GetIoStats op
    # and printed by the shmq driver on its [prom] line. Absent (→ dropped) for
    # backends whose driver does not emit them.
    ("ssd_read_bytes",                "SSD bytes read (device)",    "bytes"),
    ("ssd_write_bytes",               "SSD bytes written (device)", "bytes"),
    ("num_preemptions",               "Engine preemptions",         "int"),
]
COUNTER_KEYS = [c[0] for c in COUNTERS]

# ── Certus-server KV tier-movement counters, parsed from server.log (not the
# vLLM [prom] stream). Cumulative counts; rendered as their own panels showing
# the run's growth curve, with the final total in the panel subtitle. Only the
# Certus-SPDK variant has a certus-server, so these appear for that series alone.
TIER_COUNTERS = [
    ("tier_promotions_to_memory", "KV promotions SSD→DRAM (cumulative)",  "int"),
    ("tier_promotions_to_gpu",    "KV promotions →GPU (cumulative)",      "int"),
    ("tier_evictions_from_memory","KV evictions from DRAM (cumulative)",       "int"),
    ("tier_evictions_from_ssd",   "KV evictions from SSD (cumulative)",        "int"),
]
TIER_KEYS = [c[0] for c in TIER_COUNTERS]
# Matches both the periodic "tier-events …" line and the "FINAL tier-events …"
# summary (the leading FINAL is outside the capture). Numbers are cumulative.
TIER_RE = re.compile(
    r"tier-events\s+promotions\[->memory\s+(\d+),\s*->gpu\s+(\d+)\]"
    r"\s+evictions\[memory\s+(\d+),\s*ssd\s+(\d+)\]"
)

# ── Run-total families: the per-round panels show movement over time; these
# roll each counter up to a single whole-run total and group related counters
# onto one shared axis so the magnitudes are directly comparable. (title, unit,
# [(key, short bar label), …]). Every counter in a family shares a unit — the
# skill's one-axis rule — so no family mixes bytes with counts. A family with no
# nonzero total across all series is dropped; a zero counter inside a shown
# family stays as a labelled 0 bar (a measured zero, like store-only vs load).
FAMILIES = [
    ("Tokens — run total", "int", [
        ("prompt_tokens",        "prompt"),
        ("prompt_tokens_cached", "cached"),
        ("generation_tokens",    "generation"),
    ]),
    ("Prefix-cache queries & hits — run total", "int", [
        ("prefix_cache_queries",          "GPU q"),
        ("prefix_cache_hits",             "GPU hit"),
        ("external_prefix_cache_queries", "offload q"),
        ("external_prefix_cache_hits",    "offload hit"),
    ]),
    ("Bytes moved — run total", "bytes", [
        ("kv_offload_store_bytes", "store"),
        ("kv_offload_load_bytes",  "load"),
        ("ssd_read_bytes",         "SSD read"),
        ("ssd_write_bytes",        "SSD write"),
    ]),
    ("KV tier movements — run total", "int", [
        ("tier_promotions_to_memory",  "→DRAM"),
        ("tier_promotions_to_gpu",     "→GPU"),
        ("tier_evictions_from_memory", "evict DRAM"),
        ("tier_evictions_from_ssd",    "evict SSD"),
    ]),
]
# num_preemptions gets its OWN run-total panel (one column per series, the rate
# atop each) beside the GPU-util-over-time chart rather than a FAMILIES entry: its
# magnitude (tens) is orders below the token counts so it can't share their axis,
# and it pairs naturally with GPU utilization as an engine-health signal.

# Small-multiples grid layout: each of the three vLLM groups is a COLUMN of
# per-round/time counters, stacked top→bottom in the order given, and the
# columns sit side-by-side. So a group reads as a vertical strip (tokens+
# preemptions / cache queries+hits / bytes moved) — the same grouping the
# run-total FAMILIES bars use. Each small-multiple has its OWN y-axis, so a
# column's panels needn't share a unit. Counters absent from a run are dropped
# (shortening that column); a fully-empty column is skipped. For the common vLLM
# run this is a tidy 4×3 (three groups of four down).
SMALLMULT_COLS = [
    ["prompt_tokens", "prompt_tokens_cached", "generation_tokens",
     "num_preemptions"],
    ["prefix_cache_queries", "prefix_cache_hits",
     "external_prefix_cache_queries", "external_prefix_cache_hits"],
    ["kv_offload_store_bytes", "kv_offload_load_bytes",
     "ssd_read_bytes", "ssd_write_bytes"],
]
# The Certus-SPDK tier-movement counters don't belong to any vLLM column; they
# are appended BELOW the 4×3 block as full-width trailing row(s), row-major
# across the columns (a Certus run with promotions→DRAM, promotions→GPU and one
# demotion active fills a single fifth row of three). Only the Certus-SPDK
# variant has a certus-server, so this is empty for every other run.
SMALLMULT_TIER = [
    "tier_promotions_to_memory", "tier_promotions_to_gpu",
    "tier_evictions_from_memory", "tier_evictions_from_ssd",
]

# Hit counters that have a matching query counter: on the run-total bars the hit
# bar is annotated with its hit rate (hits / queries) so the raw count reads
# alongside the ratio that actually matters for cache effectiveness.
HIT_DENOM = {
    "prefix_cache_hits":          "prefix_cache_queries",
    "external_prefix_cache_hits": "external_prefix_cache_queries",
}

# Counters that also get a per-second average (total / active seconds) atop their
# run-total bar — bytes/sec for the byte families, count/sec for tokens, cache
# queries/hits, and tier movements (formatted per the family unit, see fmt_rate).
# A hit bar carries both its rate and its hit rate (it's in HIT_DENOM too), the
# two stacked. The active window starts at the counter's first nonzero round,
# not t=0 — e.g. SSD/tier loads begin only after a warmup during which the
# working set still fits DRAM, and tier movements don't start until the first
# eviction/promotion — so counting from t=0 would understate the sustained rate
# (see _active_seconds).
RATE_KEYS = {"prompt_tokens", "prompt_tokens_cached", "generation_tokens",
             "prefix_cache_queries", "prefix_cache_hits",
             "external_prefix_cache_queries", "external_prefix_cache_hits",
             "kv_offload_store_bytes", "kv_offload_load_bytes",
             "ssd_read_bytes", "ssd_write_bytes",
             "tier_promotions_to_memory", "tier_promotions_to_gpu",
             "tier_evictions_from_memory", "tier_evictions_from_ssd"}

# Palette base: the canonical colour for each variant, in VARIANT_ORDER. Colours
# are handed out one-per-series in display order (see build_series) — these
# first, then FALLBACK — so a single run of each variant keeps its canonical
# colour while repeated variants pick up distinct hues rather than sharing one.
VARIANT_COLOR = {
    "nooffload":     "#2a78d6",  # blue
    "cpuoffload":    "#eb6834",  # orange
    "tieredcpufs":   "#2f9e44",  # green
    "sharedstorage": "#1baf7a",  # teal
    "certusspdk":    "#eda100",  # gold
}
# Canonical bar/legend order; unknown variants appended after.
VARIANT_ORDER = ["nooffload", "cpuoffload", "tieredcpufs", "sharedstorage", "certusspdk"]
FALLBACK = ["#9b5de5", "#00b4d8", "#f15bb5", "#8d99ae", "#d62828", "#3a5a40"]
# Line styles for the 1st, 2nd, 3rd… run of the same variant.
STYLES = ["-", (0, (1, 2)), (0, (6, 3)), (0, (3, 2, 1, 2)), (0, (1, 1))]

PROM_RE = re.compile(r"\[prom\]\s+round\s+(\d+):\s+(.*)")
DONE_RE = re.compile(r"\[run\]\s+done\.\s+wall=([\d.]+)s")


def norm(name: str) -> str:
    """Normalise a variant name for colour/order lookup: lowercase alnum only."""
    return re.sub(r"[^a-z0-9]", "", name.lower())


def parse_prom_log(path: str) -> "OrderedDict[int, dict]":
    """Return {round_no: {counter_key: value}} for the curated counters."""
    rounds: "OrderedDict[int, dict]" = OrderedDict()
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                m = PROM_RE.search(line)
                if not m:
                    continue
                rn = int(m.group(1))
                d = rounds.setdefault(rn, {})
                for tok in m.group(2).split():
                    if "=" not in tok:
                        continue
                    k, v = tok.split("=", 1)
                    if k in COUNTER_KEYS:          # exact match; ignores _total etc.
                        try:
                            d[k] = float(v)
                        except ValueError:
                            pass
    except OSError as e:
        print(f"warning: cannot read {path}: {e}", file=sys.stderr)
    return rounds


def rounds_to_series(rounds: "OrderedDict[int, dict]") -> dict:
    """{counter_key: [per-round value]} ordered by round number (missing -> 0)."""
    if not rounds:
        return {}
    order = sorted(rounds)
    return {k: [rounds[r].get(k, 0.0) for r in order] for k in COUNTER_KEYS}


def parse_tier_log(path: str) -> dict:
    """Parse a certus-server.log into cumulative per-tick series for the four KV
    tier-movement counters. Returns {tier_key: [cumulative value per tick]} in
    log order (the periodic ticks plus the FINAL summary). Empty if none found."""
    cols = {k: [] for k in TIER_KEYS}
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                m = TIER_RE.search(line)
                if not m:
                    continue
                pm, pg, em, es = (int(g) for g in m.groups())
                cols["tier_promotions_to_memory"].append(pm)
                cols["tier_promotions_to_gpu"].append(pg)
                cols["tier_evictions_from_memory"].append(em)
                cols["tier_evictions_from_ssd"].append(es)
    except OSError as e:
        print(f"warning: cannot read {path}: {e}", file=sys.stderr)
        return {}
    return {k: v for k, v in cols.items() if v}


def wall_from_log(path: str):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            last = None
            for line in f:
                m = DONE_RE.search(line)
                if m:
                    last = float(m.group(1))
            return last
    except OSError:
        return None


def resolve_log(run_dir: str, recorded: str) -> str | None:
    """Prefer <run_dir>/<basename of recorded log>, else the recorded absolute
    path, else a case-insensitive *.log match — dirs move, casing varies."""
    cands = []
    if recorded:
        cands.append(os.path.join(run_dir, os.path.basename(recorded)))
        cands.append(recorded)
    for c in cands:
        if c and os.path.isfile(c):
            return c
    return None


def log_is_async(path: str) -> bool:
    """True if <variant>.log came from a WORKLOAD_MODE=async run.

    The async driver prints a one-time ``WORKLOAD_MODE=async`` banner (see
    run_multiturn_async.run_async_driver). It matters for axis labels: the async
    path samples the counters at 1 Hz, so its per-``round`` ``[prom]`` deltas are
    per-second movements over elapsed seconds — not per-workload-round totals like
    the batched path. The banner lands once the engine is built and generation
    starts, so a bounded head scan finds it without reading a whole verbose log."""
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            for i, line in enumerate(f):
                if "WORKLOAD_MODE=async" in line:
                    return True
                if i > 20000:
                    break
    except OSError:
        pass
    return False


def load_gpu_windows(run_dir: str) -> dict:
    """``{variant_name: gpu-util summary}`` for a run dir, or ``{}`` if unavailable.

    Reuses gpu_report to parse gpu-timeline.csv sliced by gpu-markers.csv, so the
    numbers match `gpu-summary.txt` exactly (window mean / max / p95 of
    util_gpu_pct = GPU *processor* utilization, sampled by nvidia-smi). The variant
    names gpu-markers.csv records match results.json, so each summary maps straight
    onto its series by variant name."""
    if _gpu_report is None:
        return {}
    tl = os.path.join(run_dir, "gpu-timeline.csv")
    mk = os.path.join(run_dir, "gpu-markers.csv")
    if not os.path.isfile(tl):
        return {}
    try:
        ticks = _gpu_report.read_timeline(tl)
        if not ticks:
            return {}
        windows = _gpu_report.read_windows(mk) if os.path.isfile(mk) else []
        out = {}
        for variant, start, end in windows:
            win = {t: v for t, v in ticks.items() if start <= t <= end}
            if win:
                summ = _gpu_report.summarize(variant, win)
                # Keep the raw per-tick util series (elapsed_s, util_pct) for the
                # over-time line panel; elapsed measured from the window's first
                # sample so variants overlay from a common origin.
                epochs = sorted(win)
                t0 = epochs[0]
                summ["series"] = [(t - t0, win[t]["util"]) for t in epochs]
                out[variant] = summ
        return out
    except Exception as e:  # noqa: BLE001 - GPU band is optional
        print(f"warning: GPU telemetry parse failed in {run_dir}: {e}",
              file=sys.stderr)
        return {}


def load_run(run_dir: str, tag: str):
    """Yield series dicts for one run directory.

    A series: {variant, tag, color_key, wall, data:{counter:[...]}, gpu}.
    """
    results = os.path.join(run_dir, "results.json")
    # Per-variant GPU processor utilization (nvidia-smi sampler), parsed once for
    # the whole run dir and matched onto each variant's series by name (exact, then
    # normalized so display-name vs slug mismatches in the fallback path still map).
    gpu = load_gpu_windows(run_dir)
    gpu_norm = {norm(k): v for k, v in gpu.items()}

    def _gpu_for(name):
        return gpu.get(name) or gpu_norm.get(norm(name))

    # certus-server tier-event counters live in a sibling server.log, keyed to
    # the Certus-SPDK variant (the only one with a server). Parsed once here and
    # merged into that variant's per-round data below.
    server_log = os.path.join(run_dir, "server.log")
    tier = parse_tier_log(server_log) if os.path.isfile(server_log) else {}
    entries = []
    if os.path.isfile(results):
        try:
            j = json.load(open(results, encoding="utf-8"))
            entries = j.get("variants", [])
        except (OSError, ValueError) as e:
            print(f"warning: bad {results}: {e}", file=sys.stderr)
    if entries:
        for e in entries:
            if e.get("status") not in (None, "OK"):
                continue
            name = e.get("variant") or "?"
            log = resolve_log(run_dir, e.get("log", ""))
            data = rounds_to_series(parse_prom_log(log)) if log else {}
            if norm(name) == "certusspdk":
                data = {**data, **tier}
            wall = e.get("wall_s")
            if wall is None and log:
                wall = wall_from_log(log)
            yield {"variant": name, "tag": tag, "wall": wall, "data": data,
                   "async": log_is_async(log) if log else False,
                   "gpu": _gpu_for(name)}
        return
    # ── fallback: no results.json — discover *.log with [prom] rounds
    found = False
    for fn in sorted(os.listdir(run_dir)):
        if not fn.endswith(".log"):
            continue
        path = os.path.join(run_dir, fn)
        rounds = parse_prom_log(path)
        if not rounds:
            continue
        found = True
        name = os.path.splitext(fn)[0]
        data = rounds_to_series(rounds)
        if norm(name) == "certusspdk":
            data = {**data, **tier}
        yield {"variant": name, "tag": tag,
               "wall": wall_from_log(path), "data": data,
               "async": log_is_async(path), "gpu": _gpu_for(name)}
    if not found:
        print(f"warning: no results.json and no [prom] logs in {run_dir}",
              file=sys.stderr)


# ── formatting helpers ───────────────────────────────────────────────────────
def fmt_compact(v, _pos=None):
    v = float(v)
    if v == 0:
        return "0"
    a = abs(v)
    if a >= 1e9:
        return f"{v/1e9:.1f}B"
    if a >= 1e6:
        return f"{v/1e6:.1f}M"
    if a >= 1e3:
        return f"{v/1e3:.0f}k"
    return f"{v:.0f}"


def fmt_bytes(v, _pos=None):
    v = float(v)
    if v == 0:
        return "0"
    units = ["B", "KiB", "MiB", "GiB", "TiB"]
    i = 0
    while abs(v) >= 1024 and i < len(units) - 1:
        v /= 1024.0
        i += 1
    return f"{v:.1f} {units[i]}"


def fmt_rate(rate, funit, stacked=False):
    """Per-second label for a run-total bar. The value is rounded to a whole
    number — bytes read as B/KiB/…-per second (``12 MiB/s``), counts (tokens,
    tier movements) as a plain or compact count per second. The one exception is
    a genuinely nonzero sub-unit rate (e.g. 0.3 evictions/s): it keeps a single
    decimal so it doesn't round down to a misleading ``0/s``.

    With ``stacked``, the value and its unit are returned on two lines
    (``"12\\nMiB/s"``) so the horizontal label stays narrow — about as wide as
    its longest single token — and doesn't spill into the neighbouring bar group
    when several series share one group."""
    if funit == "bytes":
        v = float(rate)
        units = ["B", "KiB", "MiB", "GiB", "TiB"]
        i = 0
        while abs(v) >= 1024 and i < len(units) - 1:
            v /= 1024.0
            i += 1
        val, unit = f"{v:.0f}", units[i] + "/s"
    elif 0 < rate < 0.5:
        val, unit = f"{rate:.1f}", "/s"   # keep a digit so a real rate isn't "0/s"
    elif rate < 1000:
        val, unit = f"{rate:.0f}", "/s"
    else:
        val, unit = fmt_compact(rate), "/s"
    sep = "\n" if stacked else (" " if funit == "bytes" else "")
    return f"{val}{sep}{unit}"


def build_series(run_args):
    """Parse all runs; assign colour (by variant) and linestyle (by repeat)."""
    series = []
    for tag, d in run_args:
        for s in load_run(d, tag):
            series.append(s)
    # linestyle per repeat of the same variant (kept as a secondary cue), and
    # freeze input order for the stable sort below.
    seen = {}
    for i, s in enumerate(series):
        nk = norm(s["variant"])
        idx = seen.get(nk, 0)
        s["style"] = STYLES[idx % len(STYLES)]
        s["dup"] = None  # filled below
        s["_ord"] = i    # freeze input order before the sort empties the list
        seen[nk] = idx + 1
    dup_variants = {nk for nk, n in seen.items() if n > 1}
    for s in series:
        nk = norm(s["variant"])
        s["label"] = s["variant"] + (f" · {s['tag']}" if nk in dup_variants and s["tag"] else "")
    # bar/legend order: canonical variant order, then input order
    def okey(s):
        nk = norm(s["variant"])
        return (VARIANT_ORDER.index(nk) if nk in VARIANT_ORDER else len(VARIANT_ORDER),
                s["_ord"])
    series.sort(key=okey)
    # One distinct colour PER SERIES, walked in display order from the palette we
    # already have (the canonical variant colours in VARIANT_ORDER, then the
    # FALLBACK hues). Colour now tracks the series, not its variant, so repeated
    # variants — e.g. several Certus configs — are told apart by hue instead of
    # all sharing one. A single run of each variant still lands on its canonical
    # colour (display order matches the palette prefix); extra runs pick up the
    # next unused hue. A per-run --color override still wins (applied later, in
    # main). Grey once the palette is exhausted.
    palette = list(dict.fromkeys([VARIANT_COLOR[k] for k in VARIANT_ORDER] + FALLBACK))
    pit = iter(palette)
    for s in series:
        s["color"] = next(pit, "#666666")
    return series


def render(series, out_path, title, subtitle, dark, dpi, width=24.0):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.lines import Line2D
    from matplotlib.ticker import FuncFormatter

    fg = "#e8e8e8" if dark else "#141414"
    mut = "#8a8a8a"
    bg = "#141414" if dark else "#ffffff"
    grid = "#333333" if dark else "#e3e3e0"
    plt.rcParams.update({
        "figure.facecolor": bg, "axes.facecolor": bg, "savefig.facecolor": bg,
        "text.color": fg, "axes.labelcolor": fg, "axes.edgecolor": grid,
        "xtick.color": mut, "ytick.color": mut, "font.size": 9,
        "axes.linewidth": 0.8,
    })

    import textwrap

    # which counters have any nonzero data across series (vLLM per-round counters
    # first, then the Certus-server tier-movement counters)
    active = [c for c in COUNTERS + TIER_COUNTERS
              if any(any(v != 0 for v in s["data"].get(c[0], [])) for s in series)]
    active_keys = {c[0] for c in active}
    # Build the RIGHT-region small-multiples as a 2D matrix `grid[r][c]` (each
    # cell a counter-meta tuple or None). The three vLLM groups are COLUMNS
    # (SMALLMULT_COLS transposed → up to 4 rows × 3 cols); the Certus tier
    # counters (SMALLMULT_TIER) are appended BELOW as full-width trailing rows,
    # row-major across those columns — so a Certus run's promotions/demotion
    # charts land on a fifth row rather than a fourth column.
    _meta = {c[0]: c for c in COUNTERS + TIER_COUNTERS}
    main_cols = [[_meta[k] for k in col_keys if k in active_keys]
                 for col_keys in SMALLMULT_COLS]
    main_cols = [c for c in main_cols if c]
    grid_ncol = len(main_cols)
    grid_cells = []
    for r in range(max((len(c) for c in main_cols), default=0)):
        grid_cells.append([(c[r] if r < len(c) else None) for c in main_cols])
    tier_active = [_meta[k] for k in SMALLMULT_TIER if k in active_keys]
    for i in range(0, len(tier_active), max(grid_ncol, 1)):
        chunk = tier_active[i:i + grid_ncol]
        grid_cells.append(chunk + [None] * (grid_ncol - len(chunk)))
    grid_nrow = len(grid_cells)

    # Curated counters that are all-zero but WERE measured are a real result (e.g.
    # a write-only run: the offload tier is queried but hit 0×, nothing is loaded
    # back, no SSD reads), not missing data — so name them explicitly in a footnote
    # rather than silently dropping the panel (which reads as "forgot to capture").
    # A zero is "measured" when: for a vLLM/SSD counter, the run captured metrics
    # at all (prompt/generation tokens moved somewhere); for a tier counter, the
    # key was merged in (server.log tier-events parsed). SSD keys additionally need
    # a Certus-SPDK series present (no other backend emits device I/O).
    has_certus = any(norm(s["variant"]) == "certusspdk" for s in series)
    captured = any(v != 0 for s in series for key in ("prompt_tokens", "generation_tokens")
                   for v in (s["data"].get(key) or []))

    def _measured(c):
        key = c[0]
        if key in TIER_KEYS:
            return any(key in s["data"] for s in series)
        if key in ("ssd_read_bytes", "ssd_write_bytes"):
            return has_certus and captured
        return captured

    zeroed = [c for c in COUNTERS + TIER_COUNTERS if c not in active and _measured(c)]
    zero_note = ""
    if zeroed:
        names = ", ".join(c[1] for c in zeroed)
        zero_note = ("Measured but zero across all runs (shown for completeness, "
                     f"not omitted): {names}.")
    note_lines = textwrap.wrap(zero_note, width=150) if zero_note else []

    # ── run-total families: roll each counter up to one whole-run number ──────
    def _total(s, key):
        vals = s["data"].get(key) or []
        if not vals:
            return 0.0
        # vLLM/SSD [prom] values are per-interval deltas → the run total is their
        # sum; tier counters are parsed cumulative → the total is the last (max).
        return max(vals) if key in TIER_KEYS else float(sum(vals))

    def _active_seconds(s, key):
        """Wall seconds from ``key``'s first nonzero round to the run's end.

        SSD I/O begins only after a warmup — early KV loads hit the DRAM tier, so
        device reads stay zero for the first rounds. Dividing the run's total
        bytes by the whole wall time would count that idle head as throughput and
        understate it; instead start the clock when the counter first moves.
        Uses the round-index fraction against wall (per-round wall isn't parsed),
        so it's an estimate: wall × (rounds from first activity to end) / rounds."""
        vals = s["data"].get(key) or []
        wall = s.get("wall")
        if not vals or not wall:
            return None
        first = next((i for i, v in enumerate(vals) if v), None)
        if first is None:
            return None
        secs = wall * (len(vals) - first) / len(vals)
        return secs if secs > 0 else None

    active_fams = [f for f in FAMILIES
                   if any(_total(s, k) for s in series for k, _lab in f[2])]

    # ── two-region landscape layout ─────────────────────────────────────────────
    # LEFT column  = the summary bands stacked top→bottom (header+legend, total
    #                wall-time bars, GPU-util bars, then a half-width grid holding
    #                the GPU-util-over-time line, preemptions bars, and the
    #                run-total family bars, 2 to a row).
    # RIGHT column = the per-round/time small multiples, one semantic family per
    #                row (grid_rows), spanning the full figure height.
    # This lays the slide out landscape for a widescreen monitor. The figure is as
    # tall as the taller of the two columns; the note band spans the full width
    # beneath both. When there are no per-round panels the left column widens to
    # the full page (no right region).
    if grid_nrow:
        LEFT_L, LEFT_R = 0.05, 0.40
        RIGHT_L, RIGHT_R = 0.44, 0.985
    else:
        LEFT_L, LEFT_R = 0.05, 0.975
        RIGHT_L, RIGHT_R = 0.0, 0.0
    left_usable = (LEFT_R - LEFT_L) * width

    legend_rows = (len(series) + 4) // 5
    hdr_h = 1.1 + 0.32 * legend_rows
    bar_h = max(1.6, 0.42 * len(series) + 0.8)
    # GPU-utilization band: one horizontal bar per series, same geometry as the
    # wall-time band. Only drawn when at least one series carries GPU telemetry.
    has_gpu = any(s.get("gpu") for s in series)
    gpu_h = max(1.0, 0.26 * len(series) + 0.45) if has_gpu else 0.0
    # The LEFT summary grid holds heterogeneous half-width panels that flow
    # row-major, 2 to a row: the GPU-util-over-time line, the Engine-preemptions
    # run-total bars, and one panel per run-total family. Each is a tagged cell so
    # the drawing loop dispatches by type — no special bands, no split gridspecs.
    has_gpu_ts = any((s.get("gpu") or {}).get("series") for s in series)
    has_preempt = any(any(v for v in (s["data"].get("num_preemptions") or []))
                      for s in series)
    left_cells = []
    if has_gpu_ts:
        left_cells.append(("gputs", None))
    if has_preempt:
        left_cells.append(("preempt", None))
    left_cells += [("fam", f) for f in active_fams]
    # 2 to a row (each panel targets ~2.8" of width); a single cell takes the row.
    if left_cells:
        fam_ncol = 2 if len(left_cells) >= 2 else 1
        fam_ncol = min(fam_ncol, max(1, round(left_usable / 2.8)))
    else:
        fam_ncol = 0
    fam_nrow = (len(left_cells) + fam_ncol - 1) // fam_ncol if fam_ncol else 0
    totals_h = 4.2 * fam_nrow
    # note band = the wrapped text lines, plus a fixed gap above them that clears
    # the last panel row's x-axis tick labels + "round" label (~0.5in), plus a
    # small bottom margin.
    note_text_h = 0.22 * len(note_lines)
    note_h = (note_text_h + 0.55) if note_lines else 0.0

    # LEFT stack: gaps must clear BOTH the lower band's title (drawn above its
    # axes) and the upper band's x-axis tick + label (drawn below its axes). The
    # header is the exception — no x-axis, legend already near its bottom — so it
    # gets a small gap rather than opening a void before the wall bars.
    GAP = 0.9
    HDR_GAP = 0.15
    left_bands = [("hdr", hdr_h), ("bar", bar_h)]
    if has_gpu:
        left_bands.append(("gpu", gpu_h))
    if totals_h:
        left_bands.append(("totals", totals_h))
    gap_below = lambda name: HDR_GAP if name == "hdr" else GAP  # noqa: E731
    left_h = (sum(h for _, h in left_bands)
              + sum(gap_below(left_bands[j][0]) for j in range(len(left_bands) - 1)))

    # RIGHT grid natural height: one group per row. Each row needs its panel plus
    # an inter-row gap that clears BOTH the upper row's x-tick + "round" label
    # (~0.5") and this row's title + source-key line (~0.5"), so ~3.4" per row.
    GRID_ROW_H = 3.75
    GRID_ROW_GAP = 1.05  # inches reserved at the bottom of each row's slot
    right_h = grid_nrow * GRID_ROW_H

    fig_h = max(left_h, right_h) + note_h
    fig = plt.figure(figsize=(width, fig_h), dpi=dpi)

    # LEFT bands: (bottom_frac, top_frac) each, stacked top→bottom from the top.
    # When the right grid makes the figure taller than the natural left stack,
    # spread the surplus across the inter-band gaps so the left column fills the
    # height instead of leaving a void below the family bars — but keep the
    # header→wall gap tight (its whole point), so distribute only over the others.
    slack = max(0.0, (fig_h - note_h) - left_h)
    elig = [j for j in range(len(left_bands) - 1) if left_bands[j][0] != "hdr"]
    extra_gap = slack / len(elig) if elig else 0.0
    pos, cur = {}, fig_h
    for j, (name, h) in enumerate(left_bands):
        pos[name] = ((cur - h) / fig_h, cur / fig_h)
        cur -= h
        if j < len(left_bands) - 1:
            cur -= gap_below(name) + (extra_gap if name != "hdr" else 0.0)
    # RIGHT grid spans (nearly) the full height: a top margin clears the first
    # row's panel titles, a bottom margin (above the note) clears the last row's
    # x-tick + "round" labels.
    grid_top = (fig_h - 0.55) / fig_h
    grid_bottom = (note_h + 0.35) / fig_h
    L, R = LEFT_L, LEFT_R  # left bands draw within the left column

    # ── title band + legend: title / subtitle / legend stacked, none overlap ──
    gs_hdr = fig.add_gridspec(1, 1, left=L, right=R,
                              top=pos["hdr"][1], bottom=pos["hdr"][0])
    hdr = fig.add_subplot(gs_hdr[0, 0]); hdr.axis("off")
    hdr.text(0, 1.0, title, fontsize=17, fontweight="bold", va="top", color=fg)
    if subtitle:
        hdr.text(0, 0.62, subtitle, fontsize=10, va="top", color=mut)
    handles = [Line2D([0], [0], color=s["color"], linestyle=s["style"], lw=2,
                      label=s["label"]) for s in series]
    hdr.legend(handles=handles, loc="upper left", bbox_to_anchor=(0, 0.34),
               ncol=min(len(series), 5), frameon=False, fontsize=9,
               handlelength=2.6, columnspacing=1.6, borderaxespad=0)

    # ── total wall-time bars ──────────────────────────────────────────────────
    gs_bar = fig.add_gridspec(1, 1, left=L, right=R,
                              top=pos["bar"][1], bottom=pos["bar"][0])
    ax = fig.add_subplot(gs_bar[0, 0])
    base = next((s["wall"] for s in series if norm(s["variant"]) == "nooffload"
                 and s["wall"]), None)
    ys = list(range(len(series)))[::-1]  # top-down
    for y, s in zip(ys, series):
        w = s["wall"] or 0
        ax.barh(y, w, color=s["color"], height=0.62, zorder=3)
        lab = f"{w:.1f}s" if w else "n/a"
        if base and s["wall"] and norm(s["variant"]) != "nooffload":
            r = s["wall"] / base
            lab += f"   ({r:.2f}× slower)" if r >= 1 else f"   ({1/r:.2f}× faster)"
        ax.text(w, y, "  " + lab, va="center", ha="left", fontsize=9.5,
                fontweight="bold", color=fg)
    ax.set_yticks(ys)
    ax.set_yticklabels([s["label"] for s in series], fontsize=9.5, color=fg)
    ax.set_xlabel("total wall time (s) — lower is better", color=mut)
    ax.set_xlim(0, max((s["wall"] or 0) for s in series) * 1.28 or 1)
    ax.set_title("Total wall time", loc="left", fontsize=11, fontweight="bold",
                 color=fg, pad=6)
    for sp in ("top", "right", "left"):
        ax.spines[sp].set_visible(False)
    ax.tick_params(left=False)
    ax.grid(axis="x", color=grid, lw=0.7, zorder=0)

    # ── GPU processor utilization bars (mean busy-% per variant window) ─────────
    # Source: profile_all.sh's nvidia-smi sampler (gpu-timeline.csv) sliced per
    # variant by gpu-markers.csv — the same numbers gpu_report writes to
    # gpu-summary.txt. This is GPU *processor* utilization (util.gpu = fraction of
    # the sampling interval a kernel was resident), NOT KV-cache/memory occupancy.
    # It is a level, so the bar is the window mean and peak/p95 ride alongside.
    if has_gpu:
        gs_gpu = fig.add_gridspec(1, 1, left=L, right=R,
                                  top=pos["gpu"][1], bottom=pos["gpu"][0])
        gax = fig.add_subplot(gs_gpu[0, 0])
        gys = list(range(len(series)))[::-1]  # top-down, same order as wall band
        for y, s in zip(gys, series):
            g = s.get("gpu")
            avg = g["util_avg"] if g else 0.0
            gax.barh(y, avg, color=s["color"], height=0.62, zorder=3)
            if g:
                lab = (f"{avg:.0f}%   ·   p95 {g['util_p95']:.0f}%   ·   "
                       f"peak {g['util_max']:.0f}%")
            else:
                lab = "n/a"
            # In the narrow left column the stats overran the bar and spilled past
            # the column edge. When the bar is long enough (util is typically
            # ~90%+) draw them INSIDE the bar in white; only fall back to a label
            # to the right of a short bar (where they wouldn't fit inside).
            inside = bool(g) and avg >= 45
            gax.text(1.5 if inside else (avg if g else 0), y,
                     (" " if inside else "  ") + lab, va="center", ha="left",
                     fontsize=9.5, fontweight="bold",
                     color=("#ffffff" if inside else fg), zorder=4)
        gax.set_yticks(gys)
        gax.set_yticklabels([s["label"] for s in series], fontsize=9.5, color=fg)
        gax.set_xlabel("mean GPU processor utilization — nvidia-smi util.gpu, "
                       "averaged over each variant's window (%)", color=mut)
        gax.set_xlim(0, 105)
        gax.set_xticks([0, 25, 50, 75, 100])
        gax.set_title("GPU processor utilization", loc="left", fontsize=11,
                      fontweight="bold", color=fg, pad=6)
        for sp in ("top", "right", "left"):
            gax.spines[sp].set_visible(False)
        gax.tick_params(left=False)
        gax.grid(axis="x", color=grid, lw=0.7, zorder=0)

    # ── LEFT summary grid: GPU-util-over-time line, preemptions bars, family bars
    # All are half-width cells flowing row-major (2 to a row) in one gridspec; the
    # loop dispatches by cell kind. No per-panel bands or split gridspecs. ───────
    def _smooth(ys, w=5):
        if len(ys) < 2:
            return ys
        half = w // 2
        return [sum(ys[max(0, i - half):min(len(ys), i + half + 1)])
                / (min(len(ys), i + half + 1) - max(0, i - half))
                for i in range(len(ys))]

    def _draw_gputs(ax):
        # The raw util.gpu series bounces 0↔100 tick-to-tick (a per-interval busy
        # flag), so plot a short moving average. x = elapsed within each variant's
        # window, so the sequentially-run variants overlay from t=0.
        for s in series:
            ser = (s.get("gpu") or {}).get("series")
            if not ser:
                continue
            ax.plot([t for t, _u in ser], _smooth([u for _t, u in ser]),
                    color=s["color"], linestyle=s["style"], lw=1.8)
        ax.set_ylim(0, 105)
        ax.set_yticks([0, 25, 50, 75, 100])
        ax.set_xlabel("elapsed within variant window (s)", color=mut, fontsize=9)
        ax.set_ylabel("GPU util % (10 s moving avg)", color=mut, fontsize=9)
        ax.set_title("GPU processor utilization over time", loc="left",
                     fontsize=10, fontweight="bold", color=fg, pad=6)
        ax.margins(x=0.02)
        for sp in ("top", "right"):
            ax.spines[sp].set_visible(False)
        ax.grid(color=grid, lw=0.6)
        ax.tick_params(labelsize=8)

    def _draw_preempt(ax):
        # Preemptions are per-round deltas (scrape_prom emits the counter's
        # movement each round), so the run total is their sum; the rate is that
        # total over the counter's active window (see _active_seconds). Series are
        # told apart by colour (the header legend), same as the family bars.
        xs = list(range(len(series)))
        vmax = 0.0
        for x, s in zip(xs, series):
            tot = _total(s, "num_preemptions")
            vmax = max(vmax, tot)
            bars = ax.bar(x, tot, width=0.72, color=s["color"], zorder=3)
            ax.bar_label(bars, labels=[fmt_compact(tot)], padding=2,
                         fontsize=8, color=mut)
            secs = _active_seconds(s, "num_preemptions")
            if tot and secs:
                ax.annotate(fmt_rate(tot / secs, "int", stacked=True),
                            xy=(x, tot), xytext=(0, 15),
                            textcoords="offset points", ha="center",
                            va="bottom", fontsize=7.5, fontweight="bold",
                            color=fg, zorder=4)
        ax.set_xticks(xs)
        ax.set_xticklabels([])
        ax.set_title("Engine preemptions — run total", loc="left",
                     fontsize=10, fontweight="bold", color=fg, pad=6)
        ax.yaxis.set_major_formatter(FuncFormatter(fmt_compact))
        ax.set_ylim(0, (vmax * 1.5) or 1)   # headroom for the count + rate stack
        ax.margins(x=0.12)
        for sp in ("top", "right"):
            ax.spines[sp].set_visible(False)
        ax.tick_params(left=True, bottom=False, labelsize=8)
        ax.grid(axis="y", color=grid, lw=0.6, zorder=0)

    def _draw_fam(ax, fam):
        # Related counters share one axis, one group per counter, one bar per
        # series (coloured like the legend).
        ftitle, funit, metrics = fam
        fmt = fmt_bytes if funit == "bytes" else fmt_compact
        n_m = len(metrics)
        # Some bars carry a derived number above the count label (hit rate on hit
        # bars, throughput on SSD bars); that stack needs extra headroom.
        has_deriv = any(k in HIT_DENOM or k in RATE_KEYS for k, _lab in metrics)
        gw = 0.8                       # width one counter's bar-group spans
        bw = gw / max(len(series), 1)  # per-series bar width within a group
        vmax = 0.0
        for si, s in enumerate(series):
            offs = -gw / 2 + bw * (si + 0.5)
            xs = [m + offs for m in range(n_m)]
            vals = [_total(s, k) for k, _lab in metrics]
            vmax = max([vmax] + vals)
            bars = ax.bar(xs, vals, width=bw * 0.9, color=s["color"], zorder=3)
            ax.bar_label(bars, labels=[fmt(v) for v in vals], padding=2,
                         fontsize=6.5, rotation=90, color=mut)
            # Derived number(s) atop the bar, above the (vertical) count label: a
            # per-second average (RATE_KEYS) and/or a hit rate (HIT_DENOM). A hit
            # bar carries both — stacked, rate on top, hit% nearest the bar (last
            # line, va="bottom" grows the block upward from here).
            for (k, _lab), x, hv in zip(metrics, xs, vals):
                parts = []
                if k in RATE_KEYS and hv:
                    secs = _active_seconds(s, k)
                    if secs:
                        parts.append(fmt_rate(hv / secs, funit, stacked=True))
                if k in HIT_DENOM:
                    q = _total(s, HIT_DENOM[k])
                    if q:
                        parts.append(f"{hv / q * 100:.0f}%")
                if not parts:
                    continue
                # Clear the rotated count label first — its height grows with the
                # string length ("860.3 MiB" is far taller than "1k").
                off = 14 + len(fmt(hv)) * 4.5
                # Horizontal label, but the rate carries its unit on a second line
                # (fmt_rate(stacked=True)) so it stays about as wide as its longest
                # token instead of spilling into the neighbouring bar group when
                # several series share one — while reading far easier than a
                # rotated-vertical label.
                ax.annotate("\n".join(parts), xy=(x, hv),
                            xytext=(0, off), textcoords="offset points",
                            ha="center", va="bottom", fontsize=7,
                            fontweight="bold", color=fg, zorder=4)
        ax.set_xticks(range(n_m))
        ax.set_xticklabels([lab for _k, lab in metrics], fontsize=8)
        # These panels are narrow (2 to a row), so wrap the long family titles
        # ("Prefix-cache queries & hits — run total") onto multiple lines instead
        # of letting them run into the neighbour.
        ax.set_title(textwrap.fill(ftitle, width=26), loc="left", fontsize=10,
                     fontweight="bold", color=fg, pad=6)
        ax.yaxis.set_major_formatter(FuncFormatter(fmt))
        # The derived label sits above the rotated count label; a two-line rate
        # (value + unit) or a two-line hit block needs a little more headroom
        # above the tallest bar than a bare count (1.34x).
        headroom = 2.2 if has_deriv else 1.34
        ax.set_ylim(0, (vmax * headroom) or 1)
        ax.margins(x=0.08)
        for sp in ("top", "right"):
            ax.spines[sp].set_visible(False)
        ax.grid(axis="y", color=grid, lw=0.6, zorder=0)
        ax.tick_params(labelsize=8)

    if left_cells:
        gs_tot = fig.add_gridspec(fam_nrow, fam_ncol, left=L, right=R,
                                  top=pos["totals"][1], bottom=pos["totals"][0],
                                  hspace=0.45, wspace=0.18)
        for i, (kind, payload) in enumerate(left_cells):
            r, c = divmod(i, fam_ncol)
            cell_ax = fig.add_subplot(gs_tot[r, c])
            if kind == "gputs":
                _draw_gputs(cell_ax)
            elif kind == "preempt":
                _draw_preempt(cell_ax)
            else:
                _draw_fam(cell_ax, payload)

    # Axis semantics of the vLLM/SSD counter panels depend on how the run was
    # driven. Batched: each [prom] point is one workload round's total, x = round.
    # Async: the sampler ticks at 1 Hz, so each point is the counter's movement
    # over ~1 s — i.e. a per-second rate, x = elapsed seconds. Only claim seconds
    # when EVERY series is async (a mixed overlay can't share one x meaning); the
    # tier panels keep their own "telemetry tick" x regardless (server.log cadence).
    x_is_seconds = bool(series) and all(s.get("async") for s in series)

    # ── per-round small multiples (RIGHT region) ──────────────────────────────
    # The three vLLM groups are columns (tokens+preemptions / cache queries+hits
    # / bytes moved), each reading as a vertical strip; the Certus tier charts
    # trail below on their own full-width row(s). The grid spans the full figure
    # height, balancing the left summary stack, drawn row-by-row (each row its own
    # single-row gridspec so the inter-row gap can clear titles). An empty matrix
    # cell (ragged column bottom, or a short trailing row) is just skipped.
    if grid_nrow:
        row_frac = (grid_top - grid_bottom) / grid_nrow
        for r in range(grid_nrow):
            row_top = grid_top - r * row_frac
            row_bot = row_top - row_frac + (GRID_ROW_GAP / fig_h)  # clear titles
            gs_row = fig.add_gridspec(1, grid_ncol, left=RIGHT_L, right=RIGHT_R,
                                      top=row_top, bottom=row_bot, wspace=0.28)
            for c in range(grid_ncol):
                cell = grid_cells[r][c]
                if cell is None:
                    continue  # empty matrix cell — no panel here
                key, ctitle, unit = cell
                cax = fig.add_subplot(gs_row[0, c])
                for s in series:
                    vals = s["data"].get(key)
                    if not vals or all(v == 0 for v in vals):
                        continue
                    xs = list(range(1, len(vals) + 1))
                    cax.plot(xs, vals, color=s["color"], linestyle=s["style"],
                             lw=1.8)
                is_tier = key in TIER_KEYS
                cax.set_title(ctitle, loc="left", fontsize=10, fontweight="bold",
                              color=fg, pad=20)
                src = (("certus-server:" + key[len("tier_"):]) if is_tier
                       else ("vllm:" + key))
                cax.text(0, 1.012, src, transform=cax.transAxes, fontsize=7.5,
                         va="bottom", ha="left", color=mut, family="monospace")
                cax.yaxis.set_major_formatter(
                    FuncFormatter(fmt_bytes if unit == "bytes" else fmt_compact))
                if is_tier:
                    cax.set_xlabel("telemetry tick", color=mut, fontsize=8)
                elif x_is_seconds:
                    cax.set_xlabel("elapsed (s)", color=mut, fontsize=8)
                    cax.set_ylabel("per second", color=mut, fontsize=8)
                else:
                    cax.set_xlabel("round", color=mut, fontsize=8)
                cax.margins(x=0.02)
                cax.set_ylim(bottom=0)
                for sp in ("top", "right"):
                    cax.spines[sp].set_visible(False)
                cax.grid(axis="y", color=grid, lw=0.6)
                cax.tick_params(labelsize=8)

    # ── footnote: curated counters that were measured but stayed zero ─────────
    if note_lines:
        # Draw the note at the BOTTOM of its reserved band (text block is
        # note_text_h tall, +0.08in bottom margin); the ~0.47in of slack above it
        # is what clears the last panel row's hanging x-axis tick + "round" labels.
        y = (note_text_h + 0.08) / fig_h
        for ln in note_lines:
            fig.text(LEFT_L, y, ln, fontsize=8, va="top", ha="left", color=mut)
            y -= 0.22 / fig_h

    # bbox_inches="tight" expands the saved bbox to enclose EVERY artist, not just
    # the [0,1] figure rectangle. Without it two families of artist get clipped:
    # the bottom panel row's x-tick labels + "round"/"elapsed" xlabel hang below the
    # axes box, which sits at y=0 when there's no footnote band (note_h==0), so they
    # fall off the canvas; and the wall/totals bar value-labels (drawn ha="left" at
    # the bar tip, or rotated atop a bar) can overrun R=0.975 off the right edge. The
    # manual gridspec positions are untouched — tight only grows the crop. The pad
    # keeps a small margin; savefig.facecolor (set in rcParams) fills it in-theme.
    fig.savefig(out_path, dpi=dpi, bbox_inches="tight", pad_inches=0.3)
    plt.close(fig)


def main(argv=None):
    ap = argparse.ArgumentParser(
        description="Extract KV-offload profile data from run dirs and render a PNG.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="RUN is a run directory, or TAG=DIR to set an explicit legend tag.")
    ap.add_argument("runs", nargs="+", metavar="RUN",
                    help="run directory, or TAG=DIR")
    ap.add_argument("-o", "--out", default="kvprofile-slide.png",
                    help="output PNG path [kvprofile-slide.png]")
    ap.add_argument("--title", default="KV-offload profile")
    ap.add_argument("--subtitle", default=None,
                    help="subtitle line (default: auto from results.json)")
    ap.add_argument("--variants", default=None,
                    help="comma list; keep only these (matched loosely by name)")
    ap.add_argument("--color", action="append", default=[], metavar="TAG=HEX",
                    help="override a run's line/bar colour by its legend tag, e.g. "
                         "--color shmq+fix-sq='#7048e8' (repeatable). Lets one run "
                         "of a shared variant stand out from its same-coloured kin.")
    ap.add_argument("--dark", action="store_true", help="dark theme")
    ap.add_argument("--dpi", type=int, default=200)
    ap.add_argument("--width", type=float, default=24.0,
                    help="figure width in inches [24.0]; the layout is two columns "
                         "(left summary stack, right per-round grid) — wider widens "
                         "both regions for a more landscape slide")
    args = ap.parse_args(argv)

    # parse RUN args (TAG=DIR or DIR); default tag = trailing token of basename
    run_args = []
    for spec in args.runs:
        if "=" in spec and not os.path.isdir(spec):
            tag, d = spec.split("=", 1)
        else:
            d = spec
            base = os.path.basename(os.path.normpath(d))
            tag = base.rsplit("-", 1)[-1] if "-" in base else base
        if not os.path.isdir(d):
            ap.error(f"not a directory: {d}")
        run_args.append((tag, d))

    series = build_series(run_args)
    if args.variants:
        keep = {norm(v) for v in args.variants.split(",")}
        series = [s for s in series if norm(s["variant"]) in keep]
    if not series:
        ap.error("no variants found (bad dirs, or --variants filtered everything)")

    # Per-run colour overrides (by legend tag), applied after variant colouring so
    # one run of a shared variant can be given a distinct hue.
    if args.color:
        overrides = {}
        for spec in args.color:
            if "=" not in spec:
                ap.error(f"--color expects TAG=HEX, got: {spec}")
            tag, hexv = spec.split("=", 1)
            overrides[tag] = hexv
        for s in series:
            if s["tag"] in overrides:
                s["color"] = overrides[s["tag"]]

    subtitle = args.subtitle
    if subtitle is None:
        # auto: pull model/vllm/convs/rounds from the first run's results.json
        first_dir = run_args[0][1]
        rj = os.path.join(first_dir, "results.json")
        bits = []
        if os.path.isfile(rj):
            try:
                j = json.load(open(rj, encoding="utf-8"))
                if j.get("model"):
                    bits.append(j["model"])
                if j.get("vllm_version"):
                    bits.append("vLLM " + j["vllm_version"])
                if j.get("num_convs"):
                    bits.append(f"{j['num_convs']} conv")
                if j.get("max_rounds"):
                    bits.append(f"{j['max_rounds']} rounds")
            except (OSError, ValueError):
                pass
        bits.append(f"{len(series)} series from {len(run_args)} run dir"
                    + ("s" if len(run_args) != 1 else ""))
        subtitle = " · ".join(bits)

    render(series, args.out, args.title, subtitle, args.dark, args.dpi,
           width=args.width)
    print(f"wrote {args.out}  ({len(series)} series, "
          f"{sum(1 for s in series if s['data'])} with per-round data)")


if __name__ == "__main__":
    main()
