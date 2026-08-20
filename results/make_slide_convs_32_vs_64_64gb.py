#!/usr/bin/env python3
"""Render the 32-conv vs 64-conv Certus slide at 64 GiB host RAM.

Same build, same Llama-3-8B x 12-round replay, same 45 GiB DRAM tier (host
booted with mem=64G -> --total-mem 64 -> 46080 MiB spdk_zmalloc pool). The only
difference is the number of concurrent conversations (working-set size):
  32 convs : kvprofile-vllm0.26.0-075350_14226  (wall  89.5 s, 384 gens, 644 tok/s)
  64 convs : kvprofile-vllm0.26.0-080128_17076   (wall 188.2 s, 768 gens, 612 tok/s)
Punchline: the 45 GiB DRAM tier holds the whole working set at BOTH scales, so
ssd_read = 0.00 GiB every round — all KV reuse is served from DRAM and the run
is write-only. Throughput holds within 5% as the conv set doubles.
"""
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch

# ── palette (dataviz light mode) ──
PAGE = "#eef1f5"; SURFACE = "#ffffff"
INK = "#182231"; INK2 = "#56637a"; MUTED = "#8a95a8"
GRID = "#e6eaf1"
ACCENT = "#2f6df0"   # 32 convs
ALT = "#d9821a"      # 64 convs
GOOD = "#17915f"

R = list(range(1, 13))
# per-round SSD write (GiB) — the demote/offload traffic
write = {
    "c32": [0.10, 0.57, 0.52, 0.60, 0.63, 0.68, 0.64, 0.68, 0.66, 0.67, 0.72, 0.70],
    "c64": [0.24, 1.06, 1.24, 1.31, 1.40, 1.38, 1.39, 1.41, 1.38, 1.39, 1.35, 1.42],
}
# per-round SSD read (GiB) — flat zero in both: all reuse served from DRAM tier
read = {"c32": [0.0]*12, "c64": [0.0]*12}
# per-round write ops
wops = {
    "c32": [848, 4688, 4256, 4896, 5184, 5568, 5336, 5552, 5408, 5488, 5928, 5768],
    "c64": [1953, 8656, 10176, 10736, 11512, 11328, 11464, 11536, 11368, 11496, 11128, 11736],
}
# per-round write latency (us)
wlat = {
    "c32": [39.1, 35.8, 33.8, 33.9, 33.7, 33.6, 35.2, 33.5, 32.4, 32.8, 32.0, 31.9],
    "c64": [33.1, 34.0, 33.5, 34.3, 32.2, 33.6, 32.3, 32.0, 31.9, 33.7, 33.0, 33.7],
}

# scaling, indexed so the 32-conv run = 100
idx_labels = ["conversations", "generations", "wall\ntime", "throughput\n(tok/s)"]
idx_32 = [100.0, 100.0, 100.0, 100.0]
idx_64 = [200.0, 768/384*100, 188.2/89.5*100, 612/644*100]  # 200, 200, 210.3, 95.0

fig = plt.figure(figsize=(12.8, 7.2), dpi=150)
fig.patch.set_facecolor(PAGE)

# ── header ──
fig.text(0.045, 0.955, "CERTUS KV-OFFLOAD  ·  vLLM 0.26.0  ·  64 GiB HOST RAM  ·  45 GiB DRAM TIER",
         fontsize=9.5, color=MUTED, weight="bold", family="monospace")
fig.text(0.045, 0.915, "The DRAM tier absorbs all reuse at both scales",
         fontsize=18.5, color=INK, weight="bold")
fig.text(0.045, 0.882, "0 SSD reads at 32 and 64 convs — throughput holds within 5%",
         fontsize=12.5, color=INK2, weight="bold")
fig.text(0.045, 0.851,
         "Llama-3-8B · 12 rounds · 32-region multi-region offload · mem=64G booted → --total-mem 64 → 46080 MiB spdk_zmalloc pool",
         fontsize=8.8, color=MUTED)

# legend (top right)
fig.lines.append(plt.Line2D([0.775, 0.805], [0.952, 0.952], color=ACCENT, lw=2.6,
                            transform=fig.transFigure, solid_capstyle="round"))
fig.text(0.813, 0.949, "32 convs", fontsize=9.5, color=INK, weight="bold")
fig.text(0.813, 0.931, "384 gens · 89.5 s", fontsize=7, color=MUTED)
fig.lines.append(plt.Line2D([0.775, 0.805], [0.910, 0.910], color=ALT, lw=2.6,
                            transform=fig.transFigure, ls=(0, (5, 3)), solid_capstyle="round"))
fig.text(0.813, 0.907, "64 convs", fontsize=9.5, color=INK, weight="bold")
fig.text(0.813, 0.889, "768 gens · 188.2 s", fontsize=7, color=MUTED)

# ── summary strip ──
stats = [
    ("CONVERSATIONS", "32 → 64", "2× working set", "same tier fits", MUTED),
    ("WALL", "89.5 → 188.2 s", "for 2× the gens", "+5% vs linear", MUTED),
    ("THROUGHPUT", "644 → 612 tok/s", "4.3 → 4.1 gen/s", "−5%", MUTED),
    ("SSD READ", "0.00 GiB", "both runs", "all reuse from DRAM", GOOD),
    ("SSD WRITE", "7.2 → 15.0 GiB", "write-only profile", "2× · scales w/ load", MUTED),
]
x0, w, gap = 0.045, 0.1756, 0.0055
ty, th = 0.735, 0.108
for i, (k, v, cmp, d, dc) in enumerate(stats):
    x = x0 + i * (w + gap)
    box = FancyBboxPatch((x, ty), w, th, boxstyle="round,pad=0.003,rounding_size=0.008",
                         transform=fig.transFigure, facecolor=SURFACE, edgecolor=GRID, lw=1)
    fig.patches.append(box)
    fig.text(x + 0.011, ty + 0.083, k, fontsize=7.3, color=MUTED, weight="bold", family="monospace")
    fig.text(x + 0.011, ty + 0.046, v, fontsize=12.5, color=INK, weight="bold")
    fig.text(x + 0.011, ty + 0.024, cmp, fontsize=8, color=INK2)
    fig.text(x + 0.011, ty + 0.006, d, fontsize=7.8, color=dc, weight="bold")

# ── 2x2 chart grid ──
plt.rcParams["font.size"] = 9
rects = {
    0: [0.045, 0.405, 0.415, 0.175],
    1: [0.550, 0.405, 0.415, 0.175],
    2: [0.045, 0.100, 0.415, 0.175],
    3: [0.550, 0.100, 0.415, 0.175],
}

def style(ax):
    ax.set_facecolor(SURFACE)
    for s in ("top", "right"):
        ax.spines[s].set_visible(False)
    for s in ("left", "bottom"):
        ax.spines[s].set_color(GRID)
    ax.tick_params(colors=MUTED, labelsize=7.5, length=0)
    ax.grid(axis="y", color=GRID, lw=0.9)
    ax.set_axisbelow(True)
    ax.set_xlim(0.5, 13.2)
    ax.set_xticks([1, 3, 5, 7, 9, 11, 12])

def plot_pair(ax, c32, c64):
    ax.plot(R, c64, color=ALT, lw=2.2, ls=(0, (5, 3)), solid_capstyle="round", zorder=3)
    ax.plot(R, c64, "o", color=ALT, ms=4.2, mec=SURFACE, mew=1.2, zorder=4)
    ax.plot(R, c32, color=ACCENT, lw=2.4, solid_capstyle="round", zorder=5)
    ax.plot(R, c32, "o", color=ACCENT, ms=4.2, mec=SURFACE, mew=1.2, zorder=6)
    ax.annotate("64", (R[-1], c64[-1]), xytext=(7, 0), textcoords="offset points",
                color=ALT, fontsize=8, weight="bold", va="center")
    ax.annotate("32", (R[-1], c32[-1]), xytext=(7, 0), textcoords="offset points",
                color=ACCENT, fontsize=8, weight="bold", va="center")

titles = [
    ("SSD write per round  (GiB)",
     "Demoted/offloaded KV — ~0.65 GiB/round at 32 convs, ~1.4 GiB at 64 (2× the load)"),
    ("SSD read per round  (GiB)",
     "Flat zero at both scales: the 45 GiB tier holds the working set, reuse never touches SSD"),
    ("Offload write ops per round",
     "Steady-state ~5.5k ops (32) vs ~11.4k ops (64) — write count tracks conversation count"),
    ("Conv set doubles — where does the cost go?",
     "Indexed to 32-conv run = 100. Work 2×, wall +5% over linear, throughput −5%: near-ideal scaling"),
]
for idx in range(4):
    ax = fig.add_axes(rects[idx])
    style(ax)
    t, cap = titles[idx]
    ax.set_title(t, loc="left", color=INK, fontsize=11.5, weight="bold", pad=17)
    ax.text(0, 1.055, cap, transform=ax.transAxes, color=INK2, fontsize=7.7)
    if idx == 0:
        plot_pair(ax, write["c32"], write["c64"]); ax.set_ylim(0, 1.7)
    elif idx == 1:
        # flat zero for both — draw the two lines but skip the stacked end labels
        ax.plot(R, read["c64"], color=ALT, lw=2.2, ls=(0, (5, 3)),
                solid_capstyle="round", zorder=3)
        ax.plot(R, read["c32"], color=ACCENT, lw=2.4, solid_capstyle="round", zorder=5)
        ax.plot(R, read["c32"], "o", color=ACCENT, ms=4.2, mec=SURFACE, mew=1.2, zorder=6)
        ax.set_ylim(-0.5, 1.7)
        ax.annotate("0 GiB — served from DRAM", (6.5, 0), xytext=(0, 14),
                    textcoords="offset points", ha="center", color=GOOD,
                    fontsize=8.5, weight="bold")
    elif idx == 2:
        plot_pair(ax, wops["c32"], wops["c64"]); ax.set_ylim(0, 13000)
    else:
        xs = np.arange(len(idx_labels)); bw = 0.36
        ax.set_xlim(-0.5, len(idx_labels) - 0.5)
        ax.set_xticks(xs); ax.set_xticklabels(idx_labels, fontsize=7.6, color=INK2)
        ax.set_ylim(0, 235)
        ax.axhline(100, color=MUTED, lw=0.9, ls=(0, (2, 2)), zorder=1)
        ax.bar(xs - bw/2, idx_32, bw, color=ACCENT, zorder=3, edgecolor=SURFACE, lw=1.2)
        ax.bar(xs + bw/2, idx_64, bw, color=ALT, zorder=3, edgecolor=SURFACE, lw=1.2)
        for xi, (a, b) in enumerate(zip(idx_32, idx_64)):
            ax.text(xi - bw/2, a + 3, "100", ha="center", fontsize=7.2, color=ACCENT, weight="bold")
            ax.text(xi + bw/2, b + 3, f"{b:.0f}", ha="center", fontsize=7.2, color=ALT, weight="bold")

# ── footer verdict ──
fig.text(0.045, 0.050,
         "Booting mem=64G and passing --total-mem 64 reserves a 45 GiB (46080 MiB) SPDK hugepage pool for the DRAM tier — big enough that both the 32- and 64-conversation working sets stay fully resident.",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.032,
         "So ssd_read = 0.00 GiB every round in both runs: all KV reuse is served from DRAM and the only SSD traffic is the write-back/demote path (~34 µs/op).",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.014,
         "Doubling the load doubles the write volume and op count while wall grows only +5% over linear and throughput drops 5% — the tier scales cleanly with no re-read thrash.",
         fontsize=7.7, color=MUTED)

out = "results/slide-convs-32-vs-64-64gb.png"
fig.savefig(out, facecolor=PAGE)
print("wrote", out)
