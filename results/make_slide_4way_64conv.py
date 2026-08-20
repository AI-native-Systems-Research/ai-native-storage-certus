#!/usr/bin/env python3
"""Render the 4-backend comparison slide on the 64-conv x 12-round workload.

Same Llama-3-8B replay, 64 concurrent conversations, 12 rounds, 768 generations,
64 GiB host RAM. Four KV backends:
  NoOffload     : kvprofile-vllm0.26.0-083920_21163  (wall 207.6 s, 555 tok/s)
  CPUOffload    : kvprofile-vllm0.26.0-083920_21163  (wall 124.2 s, 928 tok/s)
  Tiered-CPU-FS : kvprofile-vllm0.26.0-083920_21163  (wall 127.6 s, 903 tok/s)
  Certus-SPDK   : kvprofile-vllm0.26.0-080128_17076   (wall 188.2 s, 612 tok/s)
Punchline: on a working set that fits in host RAM, the pure-RAM CPU offload wins
(1.67x over the GPU-only baseline). Certus-SPDK's gRPC + CUDA-IPC + SPDK-NVMe
path gains only 1.10x here — its DRAM tier serves all reuse (0 SSD reads, see the
32-vs-64 slide), so the extra transport overhead isn't repaid without SSD-read
pressure. This workload does not exercise Certus's SSD tier.
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
GOOD = "#17915f"

# one color per backend (color follows the entity, used across every panel)
BK = ["NoOffload", "CPUOffload", "Tiered-CPU-FS", "Certus-SPDK"]
COL = {"NoOffload": "#8a95a8", "CPUOffload": "#d9821a",
       "Tiered-CPU-FS": "#17915f", "Certus-SPDK": "#2f6df0"}
wall = {"NoOffload": 207.6, "CPUOffload": 124.2, "Tiered-CPU-FS": 127.6, "Certus-SPDK": 188.2}
toks = {"NoOffload": 555, "CPUOffload": 928, "Tiered-CPU-FS": 903, "Certus-SPDK": 612}
speedup = {b: wall["NoOffload"] / wall[b] for b in BK}  # 1.00, 1.67, 1.63, 1.10
# mean GPU busy-% over each backend's active window (nvidia-smi, gpu-summary.txt)
gpu = {"NoOffload": 59.9, "CPUOffload": 49.3, "Tiered-CPU-FS": 51.6, "Certus-SPDK": 45.1}

R = list(range(1, 13))
# per-round round latency (s) — logged by 3 backends; Certus logs total only
rlat = {
    "NoOffload":     [4.9, 4.4, 4.9, 5.0, 8.8, 17.5, 19.3, 21.0, 26.4, 29.4, 30.8, 33.1],
    "CPUOffload":    [4.0, 4.5, 4.5, 4.8, 9.2, 10.3, 10.5, 10.9, 14.4, 14.9, 15.4, 18.8],
    "Tiered-CPU-FS": [3.9, 4.4, 4.6, 4.8, 8.8, 10.7, 11.1, 12.9, 13.8, 15.8, 17.2, 17.4],
}

fig = plt.figure(figsize=(12.8, 7.2), dpi=150)
fig.patch.set_facecolor(PAGE)

# ── header ──
fig.text(0.045, 0.955, "CERTUS KV-OFFLOAD  ·  vLLM 0.26.0  ·  64 CONVS × 12 ROUNDS  ·  768 GENERATIONS  ·  64 GiB RAM",
         fontsize=9.5, color=MUTED, weight="bold", family="monospace")
fig.text(0.045, 0.915, "Four KV backends, one workload",
         fontsize=18.5, color=INK, weight="bold")
fig.text(0.045, 0.882, "RAM-resident working set → pure-RAM CPU offload wins (1.67×); Certus gains 1.10×",
         fontsize=12.5, color=INK2, weight="bold")
fig.text(0.045, 0.851,
         "Llama-3-8B · identical replay · speedup measured against the GPU-only NoOffload baseline (207.6 s)",
         fontsize=8.8, color=MUTED)

# ── summary strip: one card per backend ──
cards = [
    ("NoOffload", "GPU-only baseline", "1.00×"),
    ("CPUOffload", "vLLM → host RAM", f"{speedup['CPUOffload']:.2f}×"),
    ("Tiered-CPU-FS", "RAM + local FS", f"{speedup['Tiered-CPU-FS']:.2f}×"),
    ("Certus-SPDK", "gRPC + SPDK NVMe", f"{speedup['Certus-SPDK']:.2f}×"),
]
x0, w, gap = 0.045, 0.2213, 0.0057
ty, th = 0.735, 0.108
for i, (b, desc, sp) in enumerate(cards):
    x = x0 + i * (w + gap)
    box = FancyBboxPatch((x, ty), w, th, boxstyle="round,pad=0.003,rounding_size=0.008",
                         transform=fig.transFigure, facecolor=SURFACE, edgecolor=GRID, lw=1)
    fig.patches.append(box)
    # color chip + name
    fig.patches.append(FancyBboxPatch((x + 0.011, ty + 0.086), 0.014, 0.014,
                       boxstyle="round,pad=0.001,rounding_size=0.004", transform=fig.transFigure,
                       facecolor=COL[b], edgecolor="none"))
    fig.text(x + 0.032, ty + 0.085, b, fontsize=9.5, color=INK, weight="bold", family="monospace")
    fig.text(x + 0.011, ty + 0.050, f"{wall[b]:.0f} s", fontsize=15.5, color=INK, weight="bold")
    fig.text(x + 0.088, ty + 0.052, f"{toks[b]} tok/s · GPU {gpu[b]:.0f}%", fontsize=9, color=INK2)
    fig.text(x + 0.011, ty + 0.024, desc, fontsize=8, color=INK2)
    dc = GOOD if speedup[b] >= 1.5 else (MUTED if speedup[b] < 1.15 else INK2)
    fig.text(x + 0.011, ty + 0.006, f"{sp} vs baseline", fontsize=7.8, color=dc, weight="bold")

# ── 2x2 chart grid ──
plt.rcParams["font.size"] = 9
rects = {
    0: [0.045, 0.405, 0.415, 0.175],
    1: [0.550, 0.405, 0.415, 0.175],
    2: [0.045, 0.100, 0.415, 0.175],
    3: [0.550, 0.100, 0.415, 0.175],
}

def frame(ax):
    ax.set_facecolor(SURFACE)
    for s in ("top", "right"):
        ax.spines[s].set_visible(False)
    for s in ("left", "bottom"):
        ax.spines[s].set_color(GRID)
    ax.tick_params(colors=MUTED, labelsize=7.5, length=0)
    ax.set_axisbelow(True)

def bars(ax, vals, fmt, ymax):
    xs = np.arange(len(BK))
    ax.grid(axis="y", color=GRID, lw=0.9)
    ax.bar(xs, [vals[b] for b in BK], 0.62, color=[COL[b] for b in BK],
           zorder=3, edgecolor=SURFACE, lw=1.2)
    ax.set_xticks(xs)
    ax.set_xticklabels(["NoOffload", "CPU", "Tiered", "Certus"], fontsize=7.8, color=INK2)
    ax.set_xlim(-0.6, len(BK) - 0.4)
    ax.set_ylim(0, ymax)
    for xi, b in enumerate(BK):
        ax.text(xi, vals[b] + ymax * 0.02, fmt(vals[b]), ha="center",
                fontsize=8.2, color=INK, weight="bold")

titles = [
    ("Wall-clock time  (s) — lower is better",
     "Total time for all 12 rounds. CPUOffload 124 s vs GPU-only 208 s; Certus 188 s"),
    ("Per-round latency  (s) — how each round grows",
     "NoOffload balloons to 33 s/round (full-context recompute); offload reuses KV and stays flatter"),
    ("Mean GPU utilization  (%) — over each backend's active window",
     "NoOffload busiest (60%): full-context recompute keeps the GPU working; Certus lowest — transport stalls"),
    ("Speedup over NoOffload baseline",
     "RAM-tier backends ~1.65×; Certus 1.10× — its DRAM tier serves all reuse, SSD tier idle here"),
]
for idx in range(4):
    ax = fig.add_axes(rects[idx])
    frame(ax)
    t, cap = titles[idx]
    ax.set_title(t, loc="left", color=INK, fontsize=11.5, weight="bold", pad=17)
    ax.text(0, 1.055, cap, transform=ax.transAxes, color=INK2, fontsize=7.7)
    if idx == 0:
        bars(ax, wall, lambda v: f"{v:.0f}", 240)
    elif idx == 1:
        ax.grid(axis="y", color=GRID, lw=0.9)
        ax.set_xlim(0.5, 13.4); ax.set_xticks([1, 3, 5, 7, 9, 11, 12]); ax.set_ylim(0, 37)
        for b in ("NoOffload", "CPUOffload", "Tiered-CPU-FS"):
            ax.plot(R, rlat[b], color=COL[b], lw=2.3, solid_capstyle="round", zorder=4)
            ax.plot(R, rlat[b], "o", color=COL[b], ms=3.6, mec=SURFACE, mew=1.0, zorder=5)
        ax.annotate("NoOffload", (12, rlat["NoOffload"][-1]), xytext=(6, 0),
                    textcoords="offset points", color=COL["NoOffload"], fontsize=7.6, weight="bold", va="center")
        ax.annotate("CPU", (12, rlat["CPUOffload"][-1]), xytext=(6, 4),
                    textcoords="offset points", color=COL["CPUOffload"], fontsize=7.6, weight="bold", va="center")
        ax.annotate("Tiered", (12, rlat["Tiered-CPU-FS"][-1]), xytext=(6, -6),
                    textcoords="offset points", color=COL["Tiered-CPU-FS"], fontsize=7.6, weight="bold", va="center")
    elif idx == 2:
        bars(ax, gpu, lambda v: f"{v:.0f}%", 72)
    else:
        bars(ax, speedup, lambda v: f"{v:.2f}×", 2.0)
        ax.axhline(1.0, color=MUTED, lw=0.9, ls=(0, (2, 2)), zorder=2)

# ── footer verdict ──
fig.text(0.045, 0.050,
         "All four run the same 64-conversation replay. The two RAM-tier backends (CPUOffload, Tiered-CPU-FS) reach ~1.65× by keeping reused KV in host memory and skipping full-context recompute.",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.032,
         "Certus-SPDK reaches 1.10×: correct and stable (32-region multi-region offload, 0 dropped blocks), but its GPU→gRPC→CUDA-IPC→SPDK path adds transport cost the workload doesn't repay.",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.014,
         "Why: the working set is DRAM-resident (0 SSD reads, per the 32-vs-64 slide), so Certus's SSD tier never engages. Its advantage — capacity beyond host RAM — needs a working set that overflows DRAM.",
         fontsize=7.7, color=MUTED)

out = "results/slide-4way-64conv.png"
fig.savefig(out, facecolor=PAGE)
print("wrote", out)
