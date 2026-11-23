#!/usr/bin/env python3
"""Plot the patched CC-weka low-concurrency sweep (certus vs cputier).

Reads the combined markdown table written after the 2026-11-20/21 patched runs and
draws three small multiples against the in-flight cap: request throughput, TTFT p50
(log scale), and external KV-tier hit rate. Separate panels, not a dual axis, because
the three measures have unrelated units.

    ./plot-lowconc-patched.py [combined.md] [out.png]
"""
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.ticker import FixedLocator, FuncFormatter, NullLocator  # noqa: E402

SRC = sys.argv[1] if len(sys.argv) > 1 else "/mnt/certus1/vllm-serve-matrix/ccweka-lowconc-patched-combined.md"
OUT = sys.argv[2] if len(sys.argv) > 2 else SRC.rsplit(".", 1)[0] + ".png"

# Reference palette (validated: CVD dE 24.7, normal dE 33.6, >= 3:1 on surface).
SURFACE, INK, INK2, GRID = "#fcfcfb", "#0b0b0b", "#52514e", "#e4e3de"
SERIES = {"certus": ("#2a78d6", "Certus (SPDK NVMe tier)"), "cputier": ("#eb6834", "cputier (vLLM CPU+fs tier)")}


def load(path):
    lines = [l for l in open(path) if l.startswith("|")]
    head = [c.strip() for c in lines[0].strip("|\n").split("|")]
    data = {}
    for l in lines[2:]:
        cells = dict(zip(head, (c.strip() for c in l.strip("|\n").split("|"))))
        key = "certus" if cells["arm"].lower().startswith("certus") else "cputier"
        data.setdefault(key, []).append(cells)
    for rows in data.values():
        rows.sort(key=lambda r: int(r["streams"]))
    return data


def main():
    data = load(SRC)
    panels = [
        ("req/s", "Request throughput", "requests / s", False, "{:.2f}"),
        ("TTFT p50 (s)", "Time to first token, p50", "seconds (log scale)", True, "{:.0f} s"),
        ("ext hit %", "External KV-tier hit rate", "% of looked-up tokens", False, "{:.0f}%"),
    ]
    plt.rcParams.update({"font.family": "DejaVu Sans", "font.size": 10, "text.color": INK,
                         "axes.labelcolor": INK2, "xtick.color": INK2, "ytick.color": INK2})
    fig, axes = plt.subplots(1, 3, figsize=(15, 4.9), facecolor=SURFACE)
    for ax, (col, title, ylabel, logy, fmt) in zip(axes, panels):
        ax.set_facecolor(SURFACE)
        for key, (color, _) in SERIES.items():
            rows = data[key]
            x = [int(r["streams"]) for r in rows]
            y = [float(r[col]) for r in rows]
            ax.plot(x, y, color=color, lw=2, marker="o", ms=8, mec=SURFACE, mew=2, zorder=3, solid_capstyle="round")
            # selective direct label: last point only, in ink (identity comes from the adjacent mark)
            ax.annotate(fmt.format(y[-1]), (x[-1], y[-1]), xytext=(8, 0), textcoords="offset points",
                        va="center", fontsize=9, color=INK2)
        ax.set_xscale("log", base=2)
        ax.xaxis.set_major_locator(FixedLocator([4, 8, 16, 32, 64]))
        ax.xaxis.set_major_formatter(FuncFormatter(lambda v, _: f"{int(v)}"))
        ax.xaxis.set_minor_locator(NullLocator())
        ax.set_xlim(3.4, 90)
        if logy:
            ax.set_yscale("log")
            ax.yaxis.set_major_locator(FixedLocator([2, 5, 10, 20, 50, 100, 200]))
            ax.yaxis.set_major_formatter(FuncFormatter(lambda v, _: f"{v:g}"))
            ax.yaxis.set_minor_locator(NullLocator())
            ax.set_ylim(2.5, 260)
        elif col == "req/s":
            ax.set_ylim(0, 0.75)
        else:
            ax.set_ylim(75, 100)
        ax.set_title(title, loc="left", fontsize=12, fontweight="bold", color=INK, pad=10)
        ax.set_xlabel("in-flight cap (streams)")
        ax.set_ylabel(ylabel)
        ax.grid(axis="y", color=GRID, lw=0.8, zorder=0)
        for side in ("top", "right", "left"):
            ax.spines[side].set_visible(False)
        ax.spines["bottom"].set_color(GRID)
        ax.tick_params(length=0)

    # certus advantage, annotated once on the throughput panel (where the claim lives)
    ax = axes[0]
    c = {int(r["streams"]): float(r["req/s"]) for r in data["certus"]}
    p = {int(r["streams"]): float(r["req/s"]) for r in data["cputier"]}
    for n in (4, 16, 64):
        # just right of the cap, centred in the gap between the two lines
        ax.annotate(f"+{(c[n] / p[n] - 1) * 100:.0f}%", (n, (c[n] + p[n]) / 2), xytext=(7, 0),
                    textcoords="offset points", ha="left", va="center", fontsize=8.5, color=INK2)
        ax.plot([n, n], [p[n] + 0.012, c[n] - 0.012], color=GRID, lw=1.2, zorder=2)

    handles = [plt.Line2D([], [], color=col, lw=2, marker="o", ms=8, mec=SURFACE, mew=2) for col, _ in SERIES.values()]
    fig.legend(handles, [lab for _, lab in SERIES.values()], loc="upper right", ncol=2, frameon=False,
               bbox_to_anchor=(0.99, 1.0), fontsize=10)
    fig.text(0.01, 0.975, "CC-weka think-time replay, low concurrency — Certus vs cputier",
             fontsize=14, fontweight="bold", color=INK, va="top")
    fig.text(0.01, 0.025,
             "Qwen2.5-7B-1M dense @262k, 1 GPU, 64 GB DRAM tier both backends, 3600 s per arm, patched vLLM 0.30 "
             "scheduler (no arm stalled). Percentages: Certus request-rate advantage.",
             fontsize=8.5, color=INK2)
    fig.subplots_adjust(left=0.05, right=0.97, top=0.80, bottom=0.17, wspace=0.28)
    fig.savefig(OUT, dpi=160, facecolor=SURFACE)
    print(OUT)


if __name__ == "__main__":
    main()
