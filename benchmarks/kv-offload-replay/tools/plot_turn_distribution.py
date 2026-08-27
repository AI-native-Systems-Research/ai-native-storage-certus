#!/usr/bin/env python3
"""Plot the distribution of human-turn counts across a ShareGPT corpus.

A conversation's "turn count" here is its number of ``human`` turns — the same
quantity the multi-turn replay drives one generation round per (see
``run_multiturn_common.load_convs``). The replay keeps only conversations with
**>= 2** human turns (a single-turn conversation has nothing to replay across
rounds); this plot shows the whole distribution and shades the ``< 2`` region
that the workload drops, so the kept working set is visible against the corpus.

Usage:
    python3 tools/plot_turn_distribution.py                    # scans data/sharegpt/
    python3 tools/plot_turn_distribution.py --data DIR_OR_JSON --out FILE.png
    python3 tools/plot_turn_distribution.py --dark --max-turns 40

The input may be a directory of ``*.json`` chunks (read in sorted order) or a
single ShareGPT-format json file. Turns above ``--max-turns`` fold into one
``N+`` overflow bin so the long thin tail doesn't flatten the body of the
distribution. Summary stats are printed to stderr regardless.
"""
import argparse
import glob
import json
import os
import sys
from collections import Counter


def count_human_turns(data_path):
    """Return a Counter {human_turn_count: num_conversations} over the corpus."""
    if os.path.isdir(data_path):
        paths = sorted(glob.glob(os.path.join(data_path, "*.json")))
        if not paths:
            sys.exit(f"error: no *.json files in directory {data_path}")
    else:
        paths = [data_path]

    counts = Counter()
    for path in paths:
        with open(path) as f:
            data = json.load(f)
        for entry in data:
            turns = entry.get("conversations", [])
            human = sum(1 for t in turns if t.get("from") == "human")
            counts[human] += 1
    return counts


def percentile(sorted_vals, pct):
    """Nearest-rank percentile of an already-sorted list."""
    if not sorted_vals:
        return 0
    k = max(0, min(len(sorted_vals) - 1, int(round(pct / 100 * (len(sorted_vals) - 1)))))
    return sorted_vals[k]


def summarize(counts):
    """(total, kept>=2, dropped<2, mean, median, p95, p99, maxturns)."""
    total = sum(counts.values())
    kept = sum(n for k, n in counts.items() if k >= 2)
    vals = []
    for k, n in sorted(counts.items()):
        vals.extend([k] * n)
    mean = sum(vals) / len(vals) if vals else 0
    median = percentile(vals, 50)
    return {
        "total": total, "kept": kept, "dropped": total - kept,
        "mean": mean, "median": median,
        "p95": percentile(vals, 95), "p99": percentile(vals, 99),
        "maxturns": max(counts) if counts else 0,
    }


def render(counts, stats, out_path, max_turns, dark, dpi, subset_turns):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

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

    base = "#2a78d6"       # corpus bars (matches render_kvprofile "nooffload" blue)
    dropped_c = "#b6c7dc" if not dark else "#3f5878"  # <2 turns: excluded by replay
    subset_c = "#eb6834"   # the 12-turn subset bar (baked 450-conv DATASET_PATH)

    # Fold the long thin tail into one "max+" overflow bin.
    xs = list(range(0, max_turns + 1))
    heights = [counts.get(x, 0) for x in xs]
    overflow = sum(n for k, n in counts.items() if k > max_turns)
    if overflow:
        xs.append(max_turns + 1)
        heights.append(overflow)

    colors = []
    for x in xs:
        if x < 2:
            colors.append(dropped_c)      # dropped by the >=2 filter
        elif x == subset_turns:
            colors.append(subset_c)       # highlighted subset
        else:
            colors.append(base)

    fig, ax = plt.subplots(figsize=(11, 5.2), dpi=dpi)
    bars = ax.bar(xs, heights, width=0.86, color=colors,
                  edgecolor=bg, linewidth=0.5, zorder=3)

    # Label the highlighted subset bar and the overflow bin.
    for x, h, b in zip(xs, heights, bars):
        if x == subset_turns and h:
            ax.annotate(f"{h:,}\nconvs @ {subset_turns} turns",
                        xy=(x, h), xytext=(0, 8), textcoords="offset points",
                        ha="center", va="bottom", fontsize=8,
                        fontweight="bold", color=subset_c, zorder=5)
        if overflow and x == max_turns + 1 and h:
            ax.annotate(f"{h:,}", xy=(x, h), xytext=(0, 4),
                        textcoords="offset points", ha="center", va="bottom",
                        fontsize=8, color=mut, zorder=5)

    ax.set_xticks(xs)
    labels = [str(x) for x in xs]
    if overflow:
        labels[-1] = f"{max_turns + 1}+"
    ax.set_xticklabels(labels, fontsize=7)
    ax.set_xlim(-0.7, xs[-1] + 0.7)

    ax.set_xlabel("human turns per conversation", color=fg, fontsize=10)
    ax.set_ylabel("conversations", color=fg, fontsize=10)
    ax.grid(axis="y", color=grid, linewidth=0.6, zorder=0)
    ax.set_axisbelow(True)
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    ax.yaxis.set_major_formatter(
        plt.FuncFormatter(lambda v, _p: f"{v/1000:.0f}k" if v >= 1000 else f"{v:.0f}"))

    # Marker + legend making the >=2 replay threshold explicit.
    ax.axvline(1.5, color=mut, linewidth=1.0, linestyle=(0, (4, 3)), zorder=4)
    from matplotlib.patches import Patch
    from matplotlib.lines import Line2D
    legend = [
        Patch(facecolor=base, label="replayed (≥ 2 turns)"),
        Patch(facecolor=dropped_c, label=f"dropped (< 2 turns): {stats['dropped']:,}"),
        Patch(facecolor=subset_c, label=f"12-turn subset (baked 450)"),
        Line2D([0], [0], color=mut, linewidth=1.0, linestyle=(0, (4, 3)),
               label="replay threshold"),
    ]
    ax.legend(handles=legend, loc="upper right", frameon=False, fontsize=8)

    ax.set_title("ShareGPT corpus — human-turn distribution", loc="left",
                 fontsize=13, fontweight="bold", color=fg, pad=30)
    sub = (f"{stats['total']:,} conversations · "
           f"{stats['kept']:,} replayed (≥ 2 turns) · "
           f"mean {stats['mean']:.1f} · median {stats['median']} · "
           f"p95 {stats['p95']} · max {stats['maxturns']}")
    ax.text(0, 1.012, sub, transform=ax.transAxes, ha="left", va="bottom",
            fontsize=9, color=mut)

    fig.tight_layout()
    fig.savefig(out_path, dpi=dpi, bbox_inches="tight")
    print(f"wrote {out_path}", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--data", default="data/sharegpt",
                    help="directory of *.json chunks or a single json file "
                         "(default: data/sharegpt)")
    ap.add_argument("--out", default="slide-sharegpt-turn-distribution.png",
                    help="output PNG path")
    ap.add_argument("--max-turns", type=int, default=25,
                    help="fold turns above this into one N+ overflow bin (default 25)")
    ap.add_argument("--subset-turns", type=int, default=12,
                    help="turn count to highlight as the baked subset (default 12)")
    ap.add_argument("--dark", action="store_true", help="dark theme")
    ap.add_argument("--dpi", type=int, default=150)
    args = ap.parse_args()

    counts = count_human_turns(args.data)
    stats = summarize(counts)
    print(f"total={stats['total']:,}  kept(>=2)={stats['kept']:,} "
          f"({100*stats['kept']/stats['total']:.1f}%)  dropped(<2)={stats['dropped']:,}\n"
          f"mean={stats['mean']:.2f}  median={stats['median']}  "
          f"p95={stats['p95']}  p99={stats['p99']}  max={stats['maxturns']}",
          file=sys.stderr)
    render(counts, stats, args.out, args.max_turns, args.dark, args.dpi,
           args.subset_turns)


if __name__ == "__main__":
    main()
