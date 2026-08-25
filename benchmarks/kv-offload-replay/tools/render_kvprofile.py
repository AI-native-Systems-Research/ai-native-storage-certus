#!/usr/bin/env python3
"""render_kvprofile.py — extract KV-offload profile data and render a PNG slide.

Reads one or more `profile_all.sh` run directories (the `kvprofile-*` dirs it
writes), extracts each variant's total wall time and per-round vLLM Prometheus
counter deltas, and renders a single PNG: a total-wall-time bar chart plus a
small-multiples grid of per-round counter charts. No HTML — PNG only.

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

# Hit counters that have a matching query counter: on the run-total bars the hit
# bar is annotated with its hit rate (hits / queries) so the raw count reads
# alongside the ratio that actually matters for cache effectiveness.
HIT_DENOM = {
    "prefix_cache_hits":          "prefix_cache_queries",
    "external_prefix_cache_hits": "external_prefix_cache_queries",
}

# Counters that also get a per-second average (total / active seconds) atop their
# run-total bar — bytes/sec for the byte families, count/sec for tokens and tier
# movements (formatted per the family unit, see fmt_rate). The active window
# starts at the counter's first nonzero round, not t=0 — e.g. SSD/tier loads
# begin only after a warmup during which the working set still fits DRAM, and
# tier movements don't start until the first eviction/promotion — so counting
# from t=0 would understate the sustained rate (see _active_seconds).
RATE_KEYS = {"prompt_tokens", "prompt_tokens_cached", "generation_tokens",
             "kv_offload_store_bytes", "kv_offload_load_bytes",
             "ssd_read_bytes", "ssd_write_bytes",
             "tier_promotions_to_memory", "tier_promotions_to_gpu",
             "tier_evictions_from_memory", "tier_evictions_from_ssd"}

# Fixed colour per variant (normalised name -> hex); unknown variants draw from
# FALLBACK in first-seen order.
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


def load_run(run_dir: str, tag: str):
    """Yield series dicts for one run directory.

    A series: {variant, tag, color_key, wall, data:{counter:[...]}}.
    """
    results = os.path.join(run_dir, "results.json")
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
                   "async": log_is_async(log) if log else False}
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
               "async": log_is_async(path)}
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


def fmt_rate(rate, funit):
    """Per-second label for a run-total bar. Bytes families read as B/KiB/…-per
    second; count families (tokens, tier movements) read as a compact count per
    second, but keep one/two decimals below 10 so a sub-unit rate (e.g. 0.3
    evictions/s) doesn't collapse to ``0/s`` under fmt_compact's integer floor."""
    if funit == "bytes":
        return fmt_bytes(rate) + "/s"
    if rate < 1:
        return f"{rate:.2f}/s"
    if rate < 10:
        return f"{rate:.1f}/s"
    return fmt_compact(rate) + "/s"


def build_series(run_args):
    """Parse all runs; assign colour (by variant) and linestyle (by repeat)."""
    series = []
    for tag, d in run_args:
        for s in load_run(d, tag):
            series.append(s)
    # colour per variant
    color_map, fb = {}, iter(FALLBACK)
    for s in series:
        nk = norm(s["variant"])
        if nk not in color_map:
            color_map[nk] = VARIANT_COLOR.get(nk, next(fb, "#666666"))
    # linestyle per repeat of the same variant (stable in input order)
    seen = {}
    for s in series:
        nk = norm(s["variant"])
        idx = seen.get(nk, 0)
        s["style"] = STYLES[idx % len(STYLES)]
        s["color"] = color_map[nk]
        s["dup"] = None  # filled below
        seen[nk] = idx + 1
    dup_variants = {nk for nk, n in seen.items() if n > 1}
    for i, s in enumerate(series):
        nk = norm(s["variant"])
        s["label"] = s["variant"] + (f" · {s['tag']}" if nk in dup_variants and s["tag"] else "")
        s["_ord"] = i  # freeze input order before the sort empties the list
    # bar/legend order: canonical variant order, then input order
    def okey(s):
        nk = norm(s["variant"])
        return (VARIANT_ORDER.index(nk) if nk in VARIANT_ORDER else len(VARIANT_ORDER),
                s["_ord"])
    series.sort(key=okey)
    return series


def render(series, out_path, title, subtitle, dark, dpi):
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
    ncol = 3
    nrow = (len(active) + ncol - 1) // ncol if active else 0

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

    # ── vertical band layout ──────────────────────────────────────────────────
    # The figure is a stack of bands: header, total-wall-time bar, run-total
    # family bars (2 cols), per-round small multiples (3 cols), and the zero-note.
    # Each drawn band is its own gridspec positioned by figure fraction so their
    # column counts can differ; the note occupies [0, note_h] at the very bottom.
    legend_rows = (len(series) + 4) // 5
    hdr_h = 1.1 + 0.32 * legend_rows
    bar_h = max(1.6, 0.42 * len(series) + 0.8)
    fam_ncol = 2
    fam_nrow = (len(active_fams) + fam_ncol - 1) // fam_ncol if active_fams else 0
    totals_h = 3.0 * fam_nrow
    grid_h = 2.5 * nrow
    # note band = the wrapped text lines, plus a fixed gap above them that clears
    # the last counter row's x-axis tick labels + "round" label (~0.5in), plus a
    # small bottom margin.
    note_text_h = 0.22 * len(note_lines)
    note_h = (note_text_h + 0.55) if note_lines else 0.0

    # Gap between stacked bands must clear BOTH the lower band's title (drawn
    # above its axes) and the upper band's x-axis tick + label (drawn below its
    # axes) — otherwise the wall-bar's xlabel lands on the totals titles and the
    # totals x-ticks land on the grid titles.
    GAP = 0.9
    bands = [("hdr", hdr_h), ("bar", bar_h)]
    if totals_h:
        bands.append(("totals", totals_h))
    if grid_h:
        bands.append(("grid", grid_h))
    fig_h = sum(h for _, h in bands) + note_h + GAP * (len(bands) - 1)
    fig = plt.figure(figsize=(12.5, fig_h), dpi=dpi)

    pos, cur = {}, fig_h  # (bottom_frac, top_frac) per band, stacked top→bottom
    for j, (name, h) in enumerate(bands):
        pos[name] = ((cur - h) / fig_h, cur / fig_h)
        cur -= h
        if j < len(bands) - 1:
            cur -= GAP
    # By construction cur == note_h now, so the grid's bottom sits exactly on the
    # note band (matching the footnote's reserved height below).
    L, R = 0.075, 0.975

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

    # ── run-total family bars (2-col band): related counters share one axis, one
    # group per counter, one bar per series (coloured like the legend). ─────────
    if active_fams:
        gs_tot = fig.add_gridspec(fam_nrow, fam_ncol, left=L, right=R,
                                  top=pos["totals"][1], bottom=pos["totals"][0],
                                  hspace=0.3, wspace=0.18)
        for fi, (ftitle, funit, metrics) in enumerate(active_fams):
            r, c = divmod(fi, fam_ncol)
            tax = fig.add_subplot(gs_tot[r, c])
            fmt = fmt_bytes if funit == "bytes" else fmt_compact
            n_m = len(metrics)
            # Some bars carry a derived number above the count label (hit rate on
            # hit bars, throughput on SSD bars); that stack needs extra headroom.
            has_deriv = any(k in HIT_DENOM or k in RATE_KEYS for k, _lab in metrics)
            gw = 0.8                       # width one counter's bar-group spans
            bw = gw / max(len(series), 1)  # per-series bar width within a group
            vmax = 0.0
            for si, s in enumerate(series):
                offs = -gw / 2 + bw * (si + 0.5)
                xs = [m + offs for m in range(n_m)]
                vals = [_total(s, k) for k, _lab in metrics]
                vmax = max([vmax] + vals)
                bars = tax.bar(xs, vals, width=bw * 0.9, color=s["color"], zorder=3)
                tax.bar_label(bars, labels=[fmt(v) for v in vals], padding=2,
                              fontsize=6.5, rotation=90, color=mut)
                # Derived number atop the bar, above the (vertical) count label so
                # both read: hit rate on hit bars, throughput on SSD device bars.
                for (k, _lab), x, hv in zip(metrics, xs, vals):
                    note = None
                    if k in HIT_DENOM:
                        q = _total(s, HIT_DENOM[k])
                        if q:
                            note = f"{hv / q * 100:.0f}%"
                    elif k in RATE_KEYS and hv:
                        secs = _active_seconds(s, k)
                        if secs:
                            note = fmt_rate(hv / secs, funit)
                    if not note:
                        continue
                    # Clear the rotated count label first — its height grows with
                    # the string length ("860.3 MiB" is far taller than "1k").
                    off = 14 + len(fmt(hv)) * 4.5
                    tax.annotate(note, xy=(x, hv),
                                 xytext=(0, off), textcoords="offset points",
                                 ha="center", va="bottom", fontsize=7,
                                 fontweight="bold", color=fg, zorder=4)
            tax.set_xticks(range(n_m))
            tax.set_xticklabels([lab for _k, lab in metrics], fontsize=8)
            tax.set_title(ftitle, loc="left", fontsize=10, fontweight="bold",
                          color=fg, pad=6)
            tax.yaxis.set_major_formatter(FuncFormatter(fmt))
            tax.set_ylim(0, (vmax * (2.0 if has_deriv else 1.34)) or 1)
            tax.margins(x=0.08)
            for sp in ("top", "right"):
                tax.spines[sp].set_visible(False)
            tax.grid(axis="y", color=grid, lw=0.6, zorder=0)
            tax.tick_params(labelsize=8)

    # Axis semantics of the vLLM/SSD counter panels depend on how the run was
    # driven. Batched: each [prom] point is one workload round's total, x = round.
    # Async: the sampler ticks at 1 Hz, so each point is the counter's movement
    # over ~1 s — i.e. a per-second rate, x = elapsed seconds. Only claim seconds
    # when EVERY series is async (a mixed overlay can't share one x meaning); the
    # tier panels keep their own "telemetry tick" x regardless (server.log cadence).
    x_is_seconds = bool(series) and all(s.get("async") for s in series)

    # ── per-counter small multiples ──────────────────────────────────────────
    gs_grid = (fig.add_gridspec(nrow, ncol, left=L, right=R,
                                top=pos["grid"][1], bottom=pos["grid"][0],
                                hspace=0.85, wspace=0.28)
               if grid_h else None)
    for i, (key, ctitle, unit) in enumerate(active):
        r, c = divmod(i, ncol)
        cax = fig.add_subplot(gs_grid[r, c])
        for s in series:
            vals = s["data"].get(key)
            if not vals or all(v == 0 for v in vals):
                continue
            xs = list(range(1, len(vals) + 1))
            cax.plot(xs, vals, color=s["color"], linestyle=s["style"], lw=1.8)
        is_tier = key in TIER_KEYS
        cax.set_title(ctitle, loc="left", fontsize=10, fontweight="bold", color=fg,
                      pad=20)
        src = ("certus-server:" + key[len("tier_"):]) if is_tier else ("vllm:" + key)
        cax.text(0, 1.012, src, transform=cax.transAxes, fontsize=7.5,
                 va="bottom", ha="left", color=mut, family="monospace")
        cax.yaxis.set_major_formatter(FuncFormatter(fmt_bytes if unit == "bytes"
                                                    else fmt_compact))
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
            fig.text(0.075, y, ln, fontsize=8, va="top", ha="left", color=mut)
            y -= 0.22 / fig_h

    fig.savefig(out_path, dpi=dpi)
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

    render(series, args.out, args.title, subtitle, args.dark, args.dpi)
    print(f"wrote {args.out}  ({len(series)} series, "
          f"{sum(1 for s in series if s['data'])} with per-round data)")


if __name__ == "__main__":
    main()
