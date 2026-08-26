#!/usr/bin/env python3
"""Render the v0.20 Certus shim-vs-non-shim per-round comparison slide to PNG.

No browser needed. Data is pulled from the two run logs:
  non-shim (pre-shim, current HEAD): kvprofile-vllm0.20.0-145148_11662/certus-spdk.log
  compat shim (multi-version):       /tmp/run-0.20.log  (2026-07-29)
Both: 450 conv x 12 rounds, Llama-3-8B, 150 out tok, 5400 gens, 13 GiB tier, evict@60%.
"""
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch

# ── palette (dataviz light mode; cool neutrals biased to the blue accent) ──
PAGE = "#eef1f5"; SURFACE = "#ffffff"
INK = "#182231"; INK2 = "#56637a"; MUTED = "#8a95a8"
GRID = "#e6eaf1"
ACCENT = "#2f6df0"   # non-shim / pre-shim (current)
ALT = "#d9821a"      # compat shim
GOOD = "#17915f"; WARN = "#c26a12"

R = list(range(1, 13))
# per-round SSD read (GiB) = deltas of the cumulative counter
read = {
    "non": [0.00, 2.72, 12.09, 10.51, 9.25, 9.30, 9.39, 9.62, 9.88, 9.72, 9.65, 9.96],
    "shim": [0.00, 2.38, 11.79, 10.70, 8.90, 9.60, 9.24, 9.81, 9.62, 9.56, 9.29, 9.63],
}
write = {
    "non": [7.80, 8.04, 9.43, 9.25, 9.21, 9.46, 9.61, 9.72, 9.85, 9.61, 9.96, 9.91],
    "shim": [7.60, 8.30, 9.27, 9.37, 10.20, 9.93, 10.54, 10.38, 10.47, 10.47, 10.66, 10.91],
}
lat = {  # ms; round 1 has no reads
    "non": [None, 0.909, 1.102, 1.414, 1.644, 1.838, 2.119, 2.166, 2.117, 2.175, 2.155, 2.309],
    "shim": [None, 0.916, 1.129, 1.429, 1.683, 2.010, 2.060, 2.294, 2.348, 2.349, 2.265, 2.186],
}
wall = {  # s; shim measured from log timestamps; non-shim only totals (per-round unlogged)
    "shim": [37, 30, 40, 51, 70, 81, 100, 120, 155, 185, 211, 231],
    "non_mean": 99.0,      # 1188.5 s instrumented run / 12       (slow end)
    "non_best_mean": 88.5,  # 1061.5 s (07-28 Llama x-model slide) / 12  (fast end)
}

fig = plt.figure(figsize=(12.8, 7.2), dpi=150)
fig.patch.set_facecolor(PAGE)

# ── header ──
fig.text(0.045, 0.955, "CERTUS KV-OFFLOAD  ·  vLLM 0.20.0  ·  gRPC CONNECTOR",
         fontsize=9.5, color=MUTED, weight="bold", family="monospace")
fig.text(0.045, 0.915, "Non-shim always faster — wall 1062–1188 s vs compat 1311 s",
         fontsize=18.5, color=INK, weight="bold")
fig.text(0.045, 0.879,
         "450 conv × 12 rounds · Llama-3-8B · 5,400 gens · 13 GiB DRAM tier @ 60% evict",
         fontsize=8.8, color=INK2)
fig.text(0.045, 0.858,
         "Non-shim best 1061.5 s = Llama Certus on the 07-28 cross-model slide; only the 1188.5 s run (Aug-7) kept per-round telemetry, so it drives the panels",
         fontsize=7.6, color=MUTED)

# legend (top right, clear of the headline)
fig.lines.append(plt.Line2D([0.795, 0.825], [0.952, 0.952], color=ACCENT, lw=2.6,
                            transform=fig.transFigure, solid_capstyle="round"))
fig.text(0.833, 0.949, "non-shim", fontsize=9.5, color=INK, weight="bold")
fig.text(0.833, 0.931, "pre-shim single-ver · 08-07", fontsize=7, color=MUTED)
fig.lines.append(plt.Line2D([0.795, 0.825], [0.910, 0.910], color=ALT, lw=2.6,
                            transform=fig.transFigure, ls=(0, (5, 3)), solid_capstyle="round"))
fig.text(0.833, 0.907, "compat shim", fontsize=9.5, color=INK, weight="bold")
fig.text(0.833, 0.889, "multi-version · 07-29", fontsize=7, color=MUTED)

# ── summary strip (vertically stacked lines, no horizontal collision) ──
stats = [
    ("WALL  (best–slow)", "1062–1188 s", "vs compat 1311 s", "−19%  to  −9.4%", GOOD),
    ("THROUGHPUT", "4.5–5.1 gen/s", "vs 4.1 gen/s", "+10%  to  +24%", GOOD),
    ("TOKENS / s", "682–763", "vs 618", "+10%  to  +24%", GOOD),
    ("SSD READ · 1188.5 run", "102.1 GiB", "vs 100.5 GiB", "+1.6%  ·  same work", MUTED),
    ("SSD WRITE · 1188.5 run", "111.9 GiB", "vs 118.1 GiB", "−5.3%  ·  smaller tier", WARN),
]
x0, w, gap = 0.045, 0.1756, 0.0055
ty, th = 0.735, 0.108
for i, (k, v, cmp, d, dc) in enumerate(stats):
    x = x0 + i * (w + gap)
    box = FancyBboxPatch((x, ty), w, th, boxstyle="round,pad=0.003,rounding_size=0.008",
                         transform=fig.transFigure, facecolor=SURFACE, edgecolor=GRID, lw=1)
    fig.patches.append(box)
    fig.text(x + 0.011, ty + 0.083, k, fontsize=7.3, color=MUTED, weight="bold", family="monospace")
    fig.text(x + 0.011, ty + 0.046, v, fontsize=13.5, color=(ACCENT if i < 3 else INK), weight="bold")
    fig.text(x + 0.011, ty + 0.024, cmp, fontsize=8, color=INK2)
    fig.text(x + 0.011, ty + 0.006, d, fontsize=7.8, color=dc, weight="bold")

# ── 2x2 chart grid ──
plt.rcParams["font.size"] = 9
# axes rects: [left, bottom, width, height]
rects = {
    0: [0.045, 0.405, 0.415, 0.175],  # top-left
    1: [0.550, 0.405, 0.415, 0.175],  # top-right
    2: [0.045, 0.100, 0.415, 0.175],  # bottom-left
    3: [0.550, 0.100, 0.415, 0.175],  # bottom-right
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

def plot_pair(ax, dnon, dshim):
    rs = [r for r, v in zip(R, dshim) if v is not None]
    vs = [v for v in dshim if v is not None]
    ax.plot(rs, vs, color=ALT, lw=2.2, ls=(0, (5, 3)), solid_capstyle="round", zorder=3)
    ax.plot(rs, vs, "o", color=ALT, ms=4.2, mec=SURFACE, mew=1.2, zorder=4)
    rn = [r for r, v in zip(R, dnon) if v is not None]
    vn = [v for v in dnon if v is not None]
    ax.plot(rn, vn, color=ACCENT, lw=2.4, solid_capstyle="round", zorder=5)
    ax.plot(rn, vn, "o", color=ACCENT, ms=4.2, mec=SURFACE, mew=1.2, zorder=6)
    ax.annotate("non-shim", (rn[-1], vn[-1]), xytext=(7, 7), textcoords="offset points",
                color=ACCENT, fontsize=8, weight="bold", va="center")
    ax.annotate("shim", (rs[-1], vs[-1]), xytext=(7, -9), textcoords="offset points",
                color=ALT, fontsize=8, weight="bold", va="center")

titles = [
    ("SSD read per round  (GiB)",
     "Bytes re-read from NVMe on the load path — curves overlap, read is workload-driven"),
    ("SSD write per round  (GiB)",
     "Demoted KV written back — compat a touch higher (smaller tier demotes more)"),
    ("SSD read latency per round  (ms)",
     "Mean read wait — both climb ~0.9→2.2 ms as the SSD queue saturates"),
    ("Wall time per round  (s)",
     "Grows with the re-read prefix · shim measured; non-shim per-round not logged"),
]
for idx in range(4):
    ax = fig.add_axes(rects[idx])
    style(ax)
    t, cap = titles[idx]
    ax.set_title(t, loc="left", color=INK, fontsize=11.5, weight="bold", pad=17)
    ax.text(0, 1.055, cap, transform=ax.transAxes, color=INK2, fontsize=7.7)
    if idx == 0:
        plot_pair(ax, read["non"], read["shim"]); ax.set_ylim(0, 13.5)
    elif idx == 1:
        plot_pair(ax, write["non"], write["shim"]); ax.set_ylim(0, 12.5)
    elif idx == 2:
        plot_pair(ax, lat["non"], lat["shim"]); ax.set_ylim(0, 2.6)
    else:
        ax.plot(R, wall["shim"], color=ALT, lw=2.2, ls=(0, (5, 3)), solid_capstyle="round", zorder=3)
        ax.plot(R, wall["shim"], "o", color=ALT, ms=4.2, mec=SURFACE, mew=1.2, zorder=4)
        ax.axhline(wall["non_mean"], color=ACCENT, lw=1.8, ls=(0, (2, 4)), zorder=2)
        ax.axhline(wall["non_best_mean"], color=ACCENT, lw=1.4, ls=(0, (1, 3)), alpha=0.6, zorder=2)
        ax.annotate("shim", (R[-1], wall["shim"][-1]), xytext=(7, 0), textcoords="offset points",
                    color=ALT, fontsize=8, weight="bold", va="center")
        ax.annotate("non-shim 99 s/rd  (1188.5 s run)", (1, wall["non_mean"]), xytext=(3, 6),
                    textcoords="offset points", color=ACCENT, fontsize=7.6, weight="bold")
        ax.annotate("best 88 s/rd  (1061.5 s, 07-28)", (7, wall["non_best_mean"]), xytext=(0, -13),
                    textcoords="offset points", color=ACCENT, fontsize=7.2, weight="bold", alpha=0.85)
        ax.set_ylim(0, 250)

# ── footer verdict ──
fig.text(0.045, 0.050,
         "Both completed non-shim v0.20 runs (1061.5 / 1188.5 s) beat the single compat run (1311 s); per-round read GiB + latency are identical (same workload).",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.032,
         "So the shim isn't doing less I/O, and a per-call Python shim isn't a plausible wall mover — but non-shim run-to-run spread (~12%) is comparable to the shim gap. Read it as a range, not a clean −9.4%.",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.014,
         "Caveats: not back-to-back; only the 1188.5 s run kept per-round telemetry; tier 13.0 vs ~12 GiB explains compat's +5.3% write.",
         fontsize=7.7, color=MUTED)

out = "results/slide-v020-shim-vs-noshim.png"
fig.savefig(out, facecolor=PAGE)
print("wrote", out)
