#!/usr/bin/env python3
"""Render the v0.20 Certus container-network A/B slide: rootless-podman bridge
(host.containers.internal via slirp4netns/pasta) vs host networking (localhost
loopback, no userspace proxy).

Same build, same 450x12 Llama-3-8B workload, same 13 GiB DRAM tier, same SPDK
devices (61-64) — the ONLY difference is podman's --network mode. Data:
  bridge  : kvprofile-vllm0.20.0-145148_11662  (wall 1188.5 s, 682 tok/s)
  host net: kvprofile-vllm0.20.0-100520_91522  (wall 1062.7 s, 762 tok/s)
Reference: Jul-14 loopback run = 1061.5 s (host net reproduces it to ~1 s).
Punchline: I/O is identical (~102 GiB read, same latency curve); the bridge's
per-RPC userspace-proxy latency is the entire 125.8 s / 10.6% wall gap.
"""
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch
import numpy as np

# ── palette (dataviz light mode) ──
PAGE = "#eef1f5"; SURFACE = "#ffffff"
INK = "#182231"; INK2 = "#56637a"; MUTED = "#8a95a8"
GRID = "#e6eaf1"
ACCENT = "#2f6df0"   # host net (loopback)  — the finding
ALT = "#d9821a"      # bridge (proxy)
GOOD = "#17915f"; WARN = "#c26a12"

R = list(range(1, 13))
# per-round SSD read (GiB) = deltas of the cumulative counter
read = {
    "host":   [0.00, 0.43, 14.20, 10.37, 9.44, 9.20, 9.64, 9.42, 9.60, 9.17, 10.31, 9.90],
    "bridge": [0.00, 2.72, 12.09, 10.51, 9.25, 9.30, 9.39, 9.62, 9.88, 9.72, 9.65, 9.96],
}
write = {
    "host":   [7.82, 8.29, 9.13, 9.27, 9.09, 9.73, 9.41, 9.58, 9.82, 9.71, 9.87, 9.83],
    "bridge": [7.80, 8.04, 9.43, 9.25, 9.21, 9.46, 9.61, 9.72, 9.85, 9.61, 9.96, 9.91],
}
lat = {  # ms; round 1 has no reads
    "host":   [None, 0.890, 1.091, 1.408, 1.670, 1.831, 1.987, 2.151, 2.166, 2.246, 2.145, 2.152],
    "bridge": [None, 0.909, 1.102, 1.414, 1.644, 1.838, 2.119, 2.166, 2.117, 2.175, 2.155, 2.309],
}
WALL_HOST, WALL_BRIDGE, WALL_JUL14 = 1062.7, 1188.5, 1061.5

fig = plt.figure(figsize=(12.8, 7.2), dpi=150)
fig.patch.set_facecolor(PAGE)

# ── header ──
fig.text(0.045, 0.955, "CERTUS KV-OFFLOAD  ·  vLLM 0.20.0  ·  CONTAINER NETWORK A/B",
         fontsize=9.5, color=MUTED, weight="bold", family="monospace")
fig.text(0.045, 0.915, "The 2 minutes was the container network",
         fontsize=18.5, color=INK, weight="bold")
fig.text(0.045, 0.882, "Loopback 1062.7 s vs rootless-podman bridge 1188.5 s — on identical I/O",
         fontsize=12.5, color=INK2, weight="bold")
fig.text(0.045, 0.851,
         "450 conv × 12 rounds · Llama-3-8B · 5,400 gens · 13 GiB DRAM tier · SPDK on 61-64 · same build, only podman --network changed",
         fontsize=8.8, color=MUTED)

# legend (top right)
fig.lines.append(plt.Line2D([0.775, 0.805], [0.952, 0.952], color=ACCENT, lw=2.6,
                            transform=fig.transFigure, solid_capstyle="round"))
fig.text(0.813, 0.949, "host net", fontsize=9.5, color=INK, weight="bold")
fig.text(0.813, 0.931, "localhost loopback · no proxy", fontsize=7, color=MUTED)
fig.lines.append(plt.Line2D([0.775, 0.805], [0.910, 0.910], color=ALT, lw=2.6,
                            transform=fig.transFigure, ls=(0, (5, 3)), solid_capstyle="round"))
fig.text(0.813, 0.907, "bridge", fontsize=9.5, color=INK, weight="bold")
fig.text(0.813, 0.889, "host.containers.internal · slirp4netns", fontsize=7, color=MUTED)

# ── summary strip ──
stats = [
    ("WALL", "1188.5 → 1062.7 s", "bridge → host net", "−10.6%  ·  −125.8 s", GOOD),
    ("THROUGHPUT", "4.5 → 5.1 gen/s", "682 → 762 tok/s", "+13%", GOOD),
    ("SSD READ", "102.1 → 101.7 GiB", "identical work", "−0.4%", MUTED),
    ("READ LAT (r12)", "2.31 → 2.15 ms", "same SSD path", "−7%  ·  noise", MUTED),
    ("vs JUL-14 LOOPBACK", "1061.5 s", "host net = 1062.7", "Δ 1.2 s · reproduced", GOOD),
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

def plot_pair(ax, dhost, dbridge):
    rb = [r for r, v in zip(R, dbridge) if v is not None]
    vb = [v for v in dbridge if v is not None]
    ax.plot(rb, vb, color=ALT, lw=2.2, ls=(0, (5, 3)), solid_capstyle="round", zorder=3)
    ax.plot(rb, vb, "o", color=ALT, ms=4.2, mec=SURFACE, mew=1.2, zorder=4)
    rh = [r for r, v in zip(R, dhost) if v is not None]
    vh = [v for v in dhost if v is not None]
    ax.plot(rh, vh, color=ACCENT, lw=2.4, solid_capstyle="round", zorder=5)
    ax.plot(rh, vh, "o", color=ACCENT, ms=4.2, mec=SURFACE, mew=1.2, zorder=6)
    ax.annotate("host net", (rh[-1], vh[-1]), xytext=(7, 7), textcoords="offset points",
                color=ACCENT, fontsize=8, weight="bold", va="center")
    ax.annotate("bridge", (rb[-1], vb[-1]), xytext=(7, -9), textcoords="offset points",
                color=ALT, fontsize=8, weight="bold", va="center")

titles = [
    ("SSD read per round  (GiB)",
     "Bytes re-read from NVMe on the load path — curves overlap; read is workload-driven"),
    ("SSD write per round  (GiB)",
     "Demoted KV written back — effectively identical across the run"),
    ("SSD read latency per round  (ms)",
     "Mean read wait climbs ~0.9→2.2 ms; same SSD queue, transport-independent"),
    ("Wall time  (s, whole run)",
     "Host net reproduces the Jul-14 loopback run; the bridge adds 125.8 s of proxy latency"),
]
for idx in range(4):
    ax = fig.add_axes(rects[idx])
    style(ax)
    t, cap = titles[idx]
    ax.set_title(t, loc="left", color=INK, fontsize=11.5, weight="bold", pad=17)
    ax.text(0, 1.055, cap, transform=ax.transAxes, color=INK2, fontsize=7.7)
    if idx == 0:
        plot_pair(ax, read["host"], read["bridge"]); ax.set_ylim(0, 15.5)
    elif idx == 1:
        plot_pair(ax, write["host"], write["bridge"]); ax.set_ylim(0, 12.5)
    elif idx == 2:
        plot_pair(ax, lat["host"], lat["bridge"]); ax.set_ylim(0, 2.6)
    else:
        # wall-time bars: bridge vs host net, with the Jul-14 loopback reference
        labels = ["bridge\n(proxy)", "host net\n(loopback)"]
        xs = np.arange(len(labels)); bw = 0.5
        ax.set_xlim(-0.6, len(labels) - 0.4)
        ax.set_xticks(xs); ax.set_xticklabels(labels, fontsize=8, color=INK2)
        ax.set_ylim(0, 1320)
        ax.bar(xs[0], WALL_BRIDGE, bw, color=ALT, zorder=3, edgecolor=SURFACE, lw=1.2)
        ax.bar(xs[1], WALL_HOST, bw, color=ACCENT, zorder=3, edgecolor=SURFACE, lw=1.2)
        ax.text(xs[0], WALL_BRIDGE + 26, f"{WALL_BRIDGE:.1f}", ha="center",
                fontsize=9, color=ALT, weight="bold")
        ax.text(xs[1], WALL_HOST + 60, f"{WALL_HOST:.1f}", ha="center",
                fontsize=9, color=ACCENT, weight="bold")
        ax.axhline(WALL_JUL14, color=INK2, lw=1.4, ls=(0, (2, 3)), zorder=5)
        ax.annotate(f"Jul-14 loopback  {WALL_JUL14:.1f} s", (0.5, WALL_JUL14),
                    xytext=(0, 6), textcoords="offset points", ha="center",
                    color=INK2, fontsize=7.4, weight="bold")

# ── footer verdict ──
fig.text(0.045, 0.050,
         "Same v0.20 build, workload, 13 GiB tier and SPDK devices — the only change is podman's --network. SSD read (~102 GiB), write and latency curves overlap round-for-round.",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.032,
         "Yet wall drops 125.8 s (−10.6%). The rootless bridge routes every gRPC control RPC through the slirp4netns/pasta userspace proxy; host net dials localhost directly.",
         fontsize=7.7, color=INK2)
fig.text(0.045, 0.014,
         "Bulk KV moves by server-side DMA either way, so the gap is pure per-RPC latency. Host net reproduces Jul-14 loopback (1062.7 vs 1061.5 s): transport was the regression.",
         fontsize=7.7, color=MUTED)

out = "results/slide-net-bridge-vs-hostnet.png"
fig.savefig(out, facecolor=PAGE)
print("wrote", out)
