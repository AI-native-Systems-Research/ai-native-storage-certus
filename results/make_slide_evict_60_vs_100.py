#!/usr/bin/env python3
"""Render the v0.20 Certus eviction-threshold 60% vs 100% comparison slide.

Same build, same 450x12 Llama-3-8B workload, same 13 GiB DRAM tier — the ONLY
difference is the mt-evictor threshold. Data from the two run logs:
  60%  : kvprofile-vllm0.20.0-145148_11662  (wall 1188.5 s, 5791 demote events)
  100% : kvprofile-vllm0.20.0-190113_21722  (wall 1179.0 s,  228 demote events)
Punchline: the threshold reshapes HOW KV is demoted, not net work — wall/I/O
are within 1%.  So the evictor is NOT the 1188-vs-1062 lever.
"""
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch

# ── palette (dataviz light mode) ──
PAGE = "#eef1f5"; SURFACE = "#ffffff"
INK = "#182231"; INK2 = "#56637a"; MUTED = "#8a95a8"
GRID = "#e6eaf1"
ACCENT = "#2f6df0"   # 60% threshold  (drip)
ALT = "#d9821a"      # 100% threshold (burst)
GOOD = "#17915f"; WARN = "#c26a12"

R = list(range(1, 13))
# per-round SSD read (GiB) = deltas of the cumulative counter
read = {
    "t60":  [0.00, 2.72, 12.09, 10.51, 9.25, 9.30, 9.39, 9.62, 9.88, 9.72, 9.65, 9.96],
    "t100": [0.00, 0.25, 14.23, 10.83, 9.41, 9.44, 9.30, 9.65, 9.74, 9.81, 9.72, 10.00],
}
write = {
    "t60":  [7.80, 8.04, 9.43, 9.25, 9.21, 9.46, 9.61, 9.72, 9.85, 9.61, 9.96, 9.91],
    "t100": [7.81, 8.47, 9.20, 9.34, 9.40, 9.35, 9.64, 9.65, 9.94, 9.68, 9.99, 9.91],
}
lat = {  # ms; round 1 has no reads
    "t60":  [None, 0.909, 1.102, 1.414, 1.644, 1.838, 2.119, 2.166, 2.117, 2.175, 2.155, 2.309],
    "t100": [None, 0.949, 1.093, 1.404, 1.676, 1.878, 2.090, 2.207, 2.177, 2.147, 2.305, 2.317],
}
# evictor work + wall, indexed so the 60% run = 100
idx_labels = ["wall\ntime", "demote\nevents", "entries\ndemoted"]
idx_60  = [100.0, 100.0, 100.0]
idx_100 = [1179.0/1188.5*100, 228/5791*100, 116736/353123*100]  # 99.2, 3.9, 33.1

fig = plt.figure(figsize=(12.8, 7.2), dpi=150)
fig.patch.set_facecolor(PAGE)

# ── header ──
fig.text(0.045, 0.955, "CERTUS KV-OFFLOAD  ·  vLLM 0.20.0  ·  mt-EVICTOR THRESHOLD SWEEP",
         fontsize=9.5, color=MUTED, weight="bold", family="monospace")
fig.text(0.045, 0.915, "Eviction threshold isn't the wall lever",
         fontsize=18.5, color=INK, weight="bold")
fig.text(0.045, 0.882, "60% vs 100% differ 9.5 s on identical I/O",
         fontsize=12.5, color=INK2, weight="bold")
fig.text(0.045, 0.851,
         "450 conv × 12 rounds · Llama-3-8B · 5,400 gens · 13 GiB DRAM tier · same build, only the evict knob changed",
         fontsize=8.8, color=MUTED)

# legend (top right)
fig.lines.append(plt.Line2D([0.775, 0.805], [0.952, 0.952], color=ACCENT, lw=2.6,
                            transform=fig.transFigure, solid_capstyle="round"))
fig.text(0.813, 0.949, "60% threshold", fontsize=9.5, color=INK, weight="bold")
fig.text(0.813, 0.931, "drip · batch 64 · 5,791 evts", fontsize=7, color=MUTED)
fig.lines.append(plt.Line2D([0.775, 0.805], [0.910, 0.910], color=ALT, lw=2.6,
                            transform=fig.transFigure, ls=(0, (5, 3)), solid_capstyle="round"))
fig.text(0.813, 0.907, "100% threshold", fontsize=9.5, color=INK, weight="bold")
fig.text(0.813, 0.889, "burst · batch 512 · 228 evts", fontsize=7, color=MUTED)

# ── summary strip ──
stats = [
    ("WALL", "1188.5 → 1179.0 s", "60% → 100%", "−0.8%  ·  noise", MUTED),
    ("THROUGHPUT", "4.5 → 4.6 gen/s", "682 → 687 tok/s", "+0.8%", MUTED),
    ("DEMOTE EVENTS", "5,791 → 228", "60% → 100%", "−96%  ·  25× fewer", GOOD),
    ("ENTRIES DEMOTED", "353k → 117k", "60% → 100%", "−67%", GOOD),
    ("SSD READ", "102.1 → 102.4 GiB", "identical work", "+0.3%", MUTED),
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

def plot_pair(ax, d60, d100):
    r1 = [r for r, v in zip(R, d100) if v is not None]
    v1 = [v for v in d100 if v is not None]
    ax.plot(r1, v1, color=ALT, lw=2.2, ls=(0, (5, 3)), solid_capstyle="round", zorder=3)
    ax.plot(r1, v1, "o", color=ALT, ms=4.2, mec=SURFACE, mew=1.2, zorder=4)
    r0 = [r for r, v in zip(R, d60) if v is not None]
    v0 = [v for v in d60 if v is not None]
    ax.plot(r0, v0, color=ACCENT, lw=2.4, solid_capstyle="round", zorder=5)
    ax.plot(r0, v0, "o", color=ACCENT, ms=4.2, mec=SURFACE, mew=1.2, zorder=6)
    ax.annotate("60%", (r0[-1], v0[-1]), xytext=(7, 7), textcoords="offset points",
                color=ACCENT, fontsize=8, weight="bold", va="center")
    ax.annotate("100%", (r1[-1], v1[-1]), xytext=(7, -9), textcoords="offset points",
                color=ALT, fontsize=8, weight="bold", va="center")

titles = [
    ("SSD read per round  (GiB)",
     "100% delays demotion one round (r2 ~0), then converges — total re-read identical"),
    ("SSD write per round  (GiB)",
     "Demoted KV written back — effectively identical across the run"),
    ("SSD read latency per round  (ms)",
     "Mean read wait climbs ~0.9→2.3 ms; threshold has no measurable effect"),
    ("Evictor work vs wall   (indexed, 60% run = 100)",
     "100% does ~4% of the demote events and a third of the entries — wall unchanged"),
]
for idx in range(4):
    ax = fig.add_axes(rects[idx])
    style(ax)
    t, cap = titles[idx]
    ax.set_title(t, loc="left", color=INK, fontsize=11.5, weight="bold", pad=17)
    ax.text(0, 1.055, cap, transform=ax.transAxes, color=INK2, fontsize=7.7)
    if idx == 0:
        plot_pair(ax, read["t60"], read["t100"]); ax.set_ylim(0, 15.5)
    elif idx == 1:
        plot_pair(ax, write["t60"], write["t100"]); ax.set_ylim(0, 12.5)
    elif idx == 2:
        plot_pair(ax, lat["t60"], lat["t100"]); ax.set_ylim(0, 2.6)
    else:
        # grouped bar chart, indexed to the 60% run
        import numpy as np
        xs = np.arange(len(idx_labels)); bw = 0.36
        ax.set_xlim(-0.5, len(idx_labels) - 0.5)
        ax.set_xticks(xs); ax.set_xticklabels(idx_labels, fontsize=7.8, color=INK2)
        ax.set_ylim(0, 118)
        ax.bar(xs - bw/2, idx_60, bw, color=ACCENT, zorder=3,
               edgecolor=SURFACE, lw=1.2)
        ax.bar(xs + bw/2, idx_100, bw, color=ALT, zorder=3,
               edgecolor=SURFACE, lw=1.2)
        for xi, (a, b) in enumerate(zip(idx_60, idx_100)):
            ax.text(xi - bw/2, a + 2, "100", ha="center", fontsize=7.2, color=ACCENT, weight="bold")
            ax.text(xi + bw/2, b + 2, f"{b:.0f}", ha="center", fontsize=7.2, color=ALT, weight="bold")

# ── footer verdict ──
fig.text(0.045, 0.050,
         "At 60% the evictor drips continuously (5,791 events, batch 64, pinned on the boundary); at 100% it bursts only when full (228 events, batch 512, 8× aggressiveness) — internals differ ~25×.",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.032,
         "Yet wall (1188.5 vs 1179.0 s), throughput, SSD read (~102 GiB) and latency all land within 1%. The threshold reshapes HOW KV is demoted, not HOW MUCH net work the run does.",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.014,
         "Why: the ~104 GiB working set ≫ 13 GiB tier, so cold blocks are re-read from SSD regardless of evict policy. The threshold is NOT the 1188-vs-1062 lever.",
         fontsize=7.7, color=MUTED)

out = "results/slide-evict-60-vs-100.png"
fig.savefig(out, facecolor=PAGE)
print("wrote", out)
