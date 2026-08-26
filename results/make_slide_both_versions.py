#!/usr/bin/env python3
"""Render a KV-offload comparison slide across vLLM 0.23.0 and 0.26.0.

Two runs, same 450-conv x 12-turn ShareGPT replay, Llama-3-8B, single A30,
SPDK NVMe (NUMA 0):
  vLLM 0.23.0  /mnt/fs-backend-bench/kvprofile-vllm0.23.0-*  (4-way)
  vLLM 0.26.0  /mnt/fs-backend-bench/kvprofile-vllm0.26.0-094631_52651

Backends per version:
  0.23 — NoOffload, CPUOffload, SharedStorage (llmd_fs_backend), Certus-SPDK
  0.26 — NoOffload, CPUOffload, Certus-SPDK, Tiered-CPU-FS (native tiering:
         CPU primary in /dev/shm + fs secondary on RAID0/XFS)
SharedStorage is the <=0.23 path; Tiered-CPU-FS is the >=0.23 native path.
"""
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import Patch

# ── colors ──────────────────────────────────────────────────────────────────
colors = {
    "NoOffload":     "#9aa0a6",
    "CPUOffload":    "#4c8bf5",
    "Certus-SPDK":   "#34a853",
    "SharedStorage": "#f4a83d",
    "Tiered-CPU-FS": "#a142f4",
}

# ── measured wall time (s) / throughput (tok/s) ──────────────────────────────
wall = {
    "0.23": {"NoOffload": 1433.7, "CPUOffload": 1389.4, "Certus-SPDK": 1447.8, "SharedStorage": 879.6},
    "0.26": {"NoOffload": 1469.4, "CPUOffload": 1406.5, "Certus-SPDK": 1515.5, "Tiered-CPU-FS": 1117.0},
}
tok = {
    "0.23": {"NoOffload": 565, "CPUOffload": 583, "Certus-SPDK": 559, "SharedStorage": 921},
    "0.26": {"NoOffload": 551, "CPUOffload": 576, "Certus-SPDK": 534, "Tiered-CPU-FS": 725},
}

# ── per-round latency (s), 12 turns ──────────────────────────────────────────
rounds = list(range(1, 13))
pr = {
    "0.23": {
        "NoOffload":     [25.1, 36.5, 50.3, 66.3, 87.2, 107.0, 124.1, 144.2, 164.9, 184.8, 206.2, 223.7],
        "CPUOffload":    [23.5, 33.6, 41.0, 64.2, 81.5, 101.1, 120.6, 140.9, 161.5, 182.1, 204.2, 221.8],
        "SharedStorage": [24.7, 36.4, 48.3, 51.8, 60.3, 66.9, 73.8, 83.2, 93.0, 99.8, 109.6, 118.2],
        "Certus-SPDK":   [26, 36, 44, 50, 73, 95, 115, 137, 167, 185, 241, 246],
    },
    "0.26": {
        "NoOffload":     [25.4, 38.7, 52.1, 67.9, 89.0, 109.8, 126.4, 147.9, 168.8, 189.1, 211.2, 228.4],
        "CPUOffload":    [22.9, 33.2, 40.3, 64.9, 86.5, 105.7, 124.7, 144.6, 160.1, 180.9, 204.5, 224.3],
        "Certus-SPDK":   [27, 38, 46, 54, 79, 100, 123, 142, 168, 206, 232, 261],
        "Tiered-CPU-FS": [24.1, 33.8, 38.2, 51.8, 74.1, 91.4, 115.8, 113.2, 126.6, 138.5, 151.4, 144.7],
    },
}

# ── total SSD I/O over the whole run (GiB), disk-backed backends only ────────
# v0.23: summed from the 4-way run per-round series (make_slide_vllm023_4way.py)
ss_read_23  = [0.00, 0.00, 1.24, 6.17, 4.55, 23.35, 28.62, 36.95, 45.02, 55.88, 65.85, 73.69]
ss_write_23 = [0.03, 0.75, 10.72, 8.66, 8.47, 9.72, 7.94, 9.32, 10.72, 9.20, 10.42, 9.79]
cs_read_23  = [0.00, 0.13, 7.48, 18.29, 27.21, 36.41, 45.94, 55.66, 65.50, 75.29, 85.06, 94.89]
cs_write_23 = [1.90, 7.63, 8.72, 8.89, 9.16, 9.43, 9.66, 9.81, 9.80, 9.72, 9.80, 9.96]
# v0.26: Certus-SPDK server rw-telemetry, summed over the run
cs_read_26  = [0.00, 0.17, 8.23, 18.41, 27.40, 36.55, 45.80, 55.41, 65.14, 74.82, 84.48, 94.42]
cs_write_26 = [1.90, 7.84, 8.62, 8.95, 9.09, 9.24, 9.54, 9.70, 9.67, 9.61, 9.89, 10.04]

# Total read / write per (backend, version). Tiered-CPU-FS's native manager
# wasn't disk-instrumented this run; its fs secondary tier left 104 GiB resident
# on the RAID0 (= bytes written), and per-tier reads weren't recorded (None).
#   (label, total_read_GiB or None, total_write_GiB, color)
io_totals = [
    ("SharedStorage\nv0.23", sum(ss_read_23), sum(ss_write_23), colors["SharedStorage"]),
    ("Certus-SPDK\nv0.23",   sum(cs_read_23), sum(cs_write_23), colors["Certus-SPDK"]),
    ("Certus-SPDK\nv0.26",   sum(cs_read_26), sum(cs_write_26), colors["Certus-SPDK"]),
    ("Tiered-CPU-FS\nv0.26", None,            104.0,           colors["Tiered-CPU-FS"]),
]

# backend order for the bar panel (shared first, then version-exclusive)
order = ["NoOffload", "CPUOffload", "Certus-SPDK", "SharedStorage", "Tiered-CPU-FS"]

# ── figure ────────────────────────────────────────────────────────────────
fig = plt.figure(figsize=(20, 10.5), dpi=120)
fig.patch.set_facecolor("white")
gs = fig.add_gridspec(2, 3, hspace=0.40, wspace=0.24,
                      left=0.045, right=0.985, top=0.86, bottom=0.09)

fig.suptitle("KV-offload replay — vLLM 0.23.0 vs 0.26.0",
             fontsize=22, fontweight="bold", x=0.045, ha="left", y=0.965)
fig.text(0.045, 0.912,
         "Llama-3-8B · 450 convs × 12 turns · 5400 generations · single A30 · SPDK/RAID0 NVMe (NUMA 0) · 16 GiB CPU tier",
         fontsize=12, color="#444", ha="left")

# Panel 1: grouped wall-time bars (v0.23 hatched/lighter, v0.26 solid) ---------
ax1 = fig.add_subplot(gs[0, 0])
gw = 0.4
for i, b in enumerate(order):
    v23 = wall["0.23"].get(b)
    v26 = wall["0.26"].get(b)
    if v23 is not None:
        ax1.bar(i - gw/2, v23, width=gw, color=colors[b], alpha=0.5,
                hatch="//", edgecolor="white")
        sp = wall["0.23"]["NoOffload"] / v23
        ax1.text(i - gw/2, v23 + 12, f"{v23:.0f}\n{sp:.2f}×", ha="center",
                 va="bottom", fontsize=8.2, color="#555")
    if v26 is not None:
        ax1.bar(i + gw/2, v26, width=gw, color=colors[b], edgecolor="white")
        sp = wall["0.26"]["NoOffload"] / v26
        ax1.text(i + gw/2, v26 + 12, f"{v26:.0f}\n{sp:.2f}×", ha="center",
                 va="bottom", fontsize=8.2, fontweight="bold")
ax1.set_xticks(range(len(order)))
ax1.set_xticklabels(order, fontsize=9.5, rotation=12, ha="right")
ax1.set_ylabel("wall time (s), lower is better", fontsize=11)
ax1.set_title("Total run time (speedup vs same-version NoOffload)",
              fontsize=13, fontweight="bold", loc="left")
ax1.set_ylim(0, 1750)
vh = [Patch(facecolor="#888", alpha=0.5, hatch="//", edgecolor="white", label="vLLM 0.23.0"),
      Patch(facecolor="#888", edgecolor="white", label="vLLM 0.26.0")]
ax1.legend(handles=vh, fontsize=9.5, loc="upper right", frameon=False)
ax1.grid(axis="y", alpha=0.25); ax1.set_axisbelow(True)

# Panel 2: per-round latency @ 0.26 ------------------------------------------
ax2 = fig.add_subplot(gs[0, 1])
for b in ["NoOffload", "CPUOffload", "Certus-SPDK", "Tiered-CPU-FS"]:
    ax2.plot(rounds, pr["0.26"][b], marker="o", ms=4, lw=2.2, color=colors[b], label=b)
ax2.set_xlabel("conversation turn (round)", fontsize=11)
ax2.set_ylabel("round latency (s)", fontsize=11)
ax2.set_title("Per-round latency @ vLLM 0.26 — Tiered-CPU-FS flattens the tail",
              fontsize=13, fontweight="bold", loc="left")
ax2.set_xticks(rounds)
ax2.legend(fontsize=9.5, loc="upper left", frameon=False)
ax2.grid(alpha=0.25); ax2.set_axisbelow(True)
ax2.annotate("native tiering serves prior-turn KV\nfrom the CPU tier — late-round prefill cut",
             xy=(11, pr["0.26"]["Tiered-CPU-FS"][10]), xytext=(5.6, 235),
             fontsize=9.0, color="#6a1fb0",
             arrowprops=dict(arrowstyle="->", color="#a142f4", lw=1))

# Panel 3: per-round latency @ 0.23 ------------------------------------------
ax3 = fig.add_subplot(gs[0, 2])
for b in ["NoOffload", "CPUOffload", "Certus-SPDK", "SharedStorage"]:
    ax3.plot(rounds, pr["0.23"][b], marker="o", ms=4, lw=2.2, color=colors[b], label=b)
ax3.set_xlabel("conversation turn (round)", fontsize=11)
ax3.set_ylabel("round latency (s)", fontsize=11)
ax3.set_title("Per-round latency @ vLLM 0.23 — SharedStorage flattens the tail",
              fontsize=13, fontweight="bold", loc="left")
ax3.set_xticks(rounds)
ax3.legend(fontsize=9.5, loc="upper left", frameon=False)
ax3.grid(alpha=0.25); ax3.set_axisbelow(True)

# Panel 4: total SSD I/O over the whole run (read + write) -------------------
ax5 = fig.add_subplot(gs[1, 0:2])
w = 0.38
xs = list(range(len(io_totals)))
for i, (lab, rd, wr, col) in enumerate(io_totals):
    if rd is not None:
        ax5.bar(i - w/2, rd, width=w, color=col, edgecolor="white")
        ax5.text(i - w/2, rd + 6, f"{rd:.0f}", ha="center", va="bottom",
                 fontsize=9.5, fontweight="bold")
    else:
        # read not instrumented for this backend/run
        ax5.text(i - w/2, 12, "read\nn/a", ha="center", va="bottom",
                 fontsize=8.5, color="#888", style="italic")
    ax5.bar(i + w/2, wr, width=w, color=col, alpha=0.45, hatch="//", edgecolor="white")
    ax5.text(i + w/2, wr + 6, f"{wr:.0f}", ha="center", va="bottom", fontsize=9.5)
ax5.set_xticks(xs)
ax5.set_xticklabels([t[0] for t in io_totals], fontsize=10)
ax5.set_ylabel("total SSD I/O over run (GiB)", fontsize=11)
ax5.set_title("Total SSD read / write over the run",
              fontsize=13, fontweight="bold", loc="left")
ax5.set_ylim(0, 570)
ax5.legend(handles=[Patch(facecolor="#888", edgecolor="white", label="read"),
                    Patch(facecolor="#888", alpha=0.45, hatch="//", edgecolor="white", label="write")],
           fontsize=10, loc="upper right", frameon=False)
ax5.grid(axis="y", alpha=0.25); ax5.set_axisbelow(True)
ax5.text(0.995, 0.62,
         "Certus re-reads its whole store from SSD (~512 GiB read for ~104 GiB\n"
         "written) — disk on the critical path. Tiered-CPU-FS keeps the working set\n"
         "in a 16 GiB CPU-RAM tier, spilling only 104 GiB resident to the fs tier;\n"
         "its per-tier reads weren't instrumented this run (read n/a).",
         transform=ax5.transAxes, fontsize=8.6, color="#555", va="top", ha="right")

# Panel 5: takeaways ---------------------------------------------------------
ax4 = fig.add_subplot(gs[1, 2]); ax4.axis("off")
lines = [
    ("Takeaways", None, True),
    ("• Each vLLM version has a clear offload winner that beats", "#333", False),
    ("  its GPU-only baseline by cutting late-round prefill:", "#333", False),
    ("• v0.26 — Tiered-CPU-FS: 1117.0s / 725 tok/s / 1.32×.", "#a142f4", False),
    ("   vLLM's native tiering (CPU primary in /dev/shm + fs", "#666", False),
    ("   secondary on RAID0/XFS). Best offload path at 0.26.", "#666", False),
    ("• v0.23 — SharedStorage: 879.6s / 921 tok/s / 1.63×.", "#c77d20", False),
    ("   llmd_fs_backend on RAID0/XFS — the <=0.23 path.", "#666", False),
    ("• CPUOffload — marginal both versions (1.03–1.05×):", "#4c8bf5", False),
    ("   host-RAM tier alone doesn't shortcut prefill much.", "#666", False),
    ("• Certus-SPDK — ~1.0× (0.99× / 0.97×): stores KV but", "#34a853", False),
    ("   re-reads it from SSD every round (Σ 511 GiB read,", "#666", False),
    ("   growing to 94 GiB/round) — disk sits on the critical", "#666", False),
    ("   path, so the reloads don't shorten prefill yet.", "#666", False),
    ("• The winners cut critical-path SSD reads: SharedStorage", "#333", False),
    ("   Σ 341 GiB; Tiered-CPU-FS caches in 16 GiB CPU RAM,", "#333", False),
    ("   spills only 104 GiB resident to the fs tier.", "#333", False),
    ("", None, False),
    ("Baselines differ across versions (NoOffload 1433.7s", "#333", False),
    ("@0.23 vs 1469.4s @0.26) — compare speedups within a", "#333", False),
    ("version, not raw seconds across versions.", "#333", False),
]
y = 1.0
for txt, col, hdr in lines:
    if hdr:
        ax4.text(0.0, y, txt, fontsize=13, fontweight="bold", va="top"); y -= 0.058
    else:
        ax4.text(0.0, y, txt, fontsize=9.2, color=col or "#000", va="top"); y -= 0.044

out = "/home/bdh/ai-native-storage-certus/benchmarks/kv-offload-replay/slide-both-versions.png"
fig.savefig(out, facecolor="white", bbox_inches="tight")
print("wrote", out)
