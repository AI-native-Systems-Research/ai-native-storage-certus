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
    ("num_preemptions",               "Engine preemptions",         "int"),
]
COUNTER_KEYS = [c[0] for c in COUNTERS]

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


def load_run(run_dir: str, tag: str):
    """Yield series dicts for one run directory.

    A series: {variant, tag, color_key, wall, data:{counter:[...]}}.
    """
    results = os.path.join(run_dir, "results.json")
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
            wall = e.get("wall_s")
            if wall is None and log:
                wall = wall_from_log(log)
            yield {"variant": name, "tag": tag, "wall": wall, "data": data}
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
        yield {"variant": os.path.splitext(fn)[0], "tag": tag,
               "wall": wall_from_log(path), "data": rounds_to_series(rounds)}
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

    # which counters have any nonzero data across series
    active = [c for c in COUNTERS
              if any(any(v != 0 for v in s["data"].get(c[0], [])) for s in series)]
    ncol = 3
    nrow = (len(active) + ncol - 1) // ncol if active else 0

    # header band scales a little with the number of legend rows so the legend
    # never lands on the subtitle.
    legend_rows = (len(series) + 4) // 5
    hdr_h = 1.1 + 0.32 * legend_rows
    bar_h = max(1.6, 0.42 * len(series) + 0.8)
    grid_h = 2.5 * nrow
    fig_h = hdr_h + bar_h + grid_h
    fig = plt.figure(figsize=(12.5, fig_h), dpi=dpi)
    gs = fig.add_gridspec(
        2 + nrow, ncol,
        height_ratios=[hdr_h, bar_h] + [2.5] * nrow,
        hspace=0.85, wspace=0.28,
        left=0.075, right=0.975, top=0.985, bottom=0.03,
    )

    # ── title band + legend (row 0, spans all cols): title / subtitle / legend
    # stacked top-to-bottom, none overlapping. ───────────────────────────────
    hdr = fig.add_subplot(gs[0, :]); hdr.axis("off")
    hdr.text(0, 1.0, title, fontsize=17, fontweight="bold", va="top", color=fg)
    if subtitle:
        hdr.text(0, 0.62, subtitle, fontsize=10, va="top", color=mut)
    handles = [Line2D([0], [0], color=s["color"], linestyle=s["style"], lw=2,
                      label=s["label"]) for s in series]
    hdr.legend(handles=handles, loc="upper left", bbox_to_anchor=(0, 0.34),
               ncol=min(len(series), 5), frameon=False, fontsize=9,
               handlelength=2.6, columnspacing=1.6, borderaxespad=0)

    # ── total wall-time bars (row 1, spans all cols) ─────────────────────────
    ax = fig.add_subplot(gs[1, :])
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

    # ── per-counter small multiples ──────────────────────────────────────────
    for i, (key, ctitle, unit) in enumerate(active):
        r, c = divmod(i, ncol)
        cax = fig.add_subplot(gs[2 + r, c])
        for s in series:
            vals = s["data"].get(key)
            if not vals or all(v == 0 for v in vals):
                continue
            xs = list(range(1, len(vals) + 1))
            cax.plot(xs, vals, color=s["color"], linestyle=s["style"], lw=1.8)
        cax.set_title(ctitle, loc="left", fontsize=10, fontweight="bold", color=fg,
                      pad=20)
        cax.text(0, 1.012, "vllm:" + key, transform=cax.transAxes, fontsize=7.5,
                 va="bottom", ha="left", color=mut, family="monospace")
        cax.yaxis.set_major_formatter(FuncFormatter(fmt_bytes if unit == "bytes"
                                                    else fmt_compact))
        cax.set_xlabel("round", color=mut, fontsize=8)
        cax.margins(x=0.02)
        cax.set_ylim(bottom=0)
        for sp in ("top", "right"):
            cax.spines[sp].set_visible(False)
        cax.grid(axis="y", color=grid, lw=0.6)
        cax.tick_params(labelsize=8)

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
