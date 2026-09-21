#!/usr/bin/env python3
"""Plot an interval-distribution YAML produced by extract_interval_distribution.py.

Draws the bucketed histogram of intra-conversation request inter-arrival
intervals: one bar per bucket spanning its [lower, upper] range, height = the
fraction of intervals in that bucket. The x-axis uses the ``scale`` recorded in
the YAML (log or linear). Summary percentiles (median / p90 / p99) are overlaid
as vertical reference lines.

Usage
-----
    ./plot_interval_distribution.py cc-trace-weka-062126-intervals.yaml
    ./plot_interval_distribution.py dist.yaml -o dist.png --metric count
"""

from __future__ import annotations

import argparse
import os
import sys

import yaml

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.ticker import FuncFormatter


def fmt_seconds(x, _pos=None):
    """Human-friendly duration tick label."""
    if x <= 0:
        return "0"
    if x < 1e-3:
        return f"{x*1e6:.0f}µs"
    if x < 1:
        return f"{x*1e3:.0f}ms"
    if x < 60:
        return f"{x:.0f}s"
    if x < 3600:
        return f"{x/60:.0f}m"
    if x < 86400:
        return f"{x/3600:.0f}h"
    return f"{x/86400:.0f}d"


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("yaml_file", help="interval-distribution YAML to plot")
    ap.add_argument("-o", "--output",
                    help="output image (default: <yaml basename>.png)")
    ap.add_argument("--metric", choices=("fraction", "count"), default="fraction",
                    help="bar height metric (default: fraction)")
    ap.add_argument("--title", help="override plot title")
    args = ap.parse_args()

    with open(args.yaml_file) as f:
        doc = yaml.safe_load(f)

    buckets = doc.get("buckets") or []
    if not buckets:
        print("error: no buckets in YAML", file=sys.stderr)
        return 1

    scale = doc.get("scale", "log")
    summary = doc.get("summary", {}) or {}

    lowers = [b["lower_s"] for b in buckets]
    uppers = [b["upper_s"] for b in buckets]
    heights = [b[args.metric] for b in buckets]
    counts = [b["count"] for b in buckets]
    widths = [u - l for l, u in zip(lowers, uppers)]

    fig, ax = plt.subplots(figsize=(11, 6))
    ax.bar(lowers, heights, width=widths, align="edge",
           color="#D4A017", edgecolor="#5a4408", linewidth=0.8, alpha=0.9)

    if scale == "log":
        ax.set_xscale("log")
    ax.xaxis.set_major_formatter(FuncFormatter(fmt_seconds))

    # Annotate each bar with its raw count (skip empty buckets).
    for l, u, h, c in zip(lowers, uppers, heights, counts):
        if c == 0:
            continue
        xc = (l * u) ** 0.5 if scale == "log" else (l + u) / 2
        ax.annotate(f"{c:,}", (xc, h), textcoords="offset points",
                    xytext=(0, 3), ha="center", va="bottom", fontsize=8)

    # Overlay summary percentiles as vertical reference lines.
    ref_lines = [("median", summary.get("median_s"), "#3A6EA5"),
                 ("p90", summary.get("p90_s"), "#2E8B57"),
                 ("p99", summary.get("p99_s"), "#E07B39")]
    for label, val, color in ref_lines:
        if val is None or (scale == "log" and val <= 0):
            continue
        ax.axvline(val, color=color, linestyle="--", linewidth=1.3)
        ax.annotate(f"{label}={fmt_seconds(val)}", (val, 1.0),
                    xycoords=("data", "axes fraction"),
                    textcoords="offset points", xytext=(3, -12),
                    color=color, fontsize=8, rotation=90, va="top")

    ax.set_xlabel(f"interval ({doc.get('units', 'seconds')}, {scale} scale)")
    ax.set_ylabel("fraction of intervals" if args.metric == "fraction"
                  else "interval count")
    ax.grid(axis="y", linestyle=":", alpha=0.5)

    n = doc.get("num_intervals", sum(counts))
    default_title = (f"Request interval distribution — {n:,} intervals\n"
                     f"{doc.get('source', args.yaml_file)}")
    ax.set_title(args.title or default_title, fontsize=11)

    fig.tight_layout()

    out = args.output or (os.path.splitext(args.yaml_file)[0] + ".png")
    fig.savefig(out, dpi=130)
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
