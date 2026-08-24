#!/usr/bin/env python3
"""Render the 4-backend KV-offload comparison slide for the vLLM 0.23.0 run.

Data is baked in from the per-variant results.json + run logs:
  NoOffload      /mnt/fs-backend-bench/kvprofile-vllm0.23.0-093940_629485
  CPUOffload     (same dir)
  SharedStorage  /mnt/fs-backend-bench/kvprofile-vllm0.23.0-090835_626054
  Certus-SPDK    /mnt/fs-backend-bench/kvprofile-vllm0.23.0-150913_29008  (rw-telemetry ON)

Workload: NousResearch/Meta-Llama-3-8B, 450 convs x 12 turns, 5400 generations,
150 output tok/gen. Single A30, dual-NUMA host, SPDK userspace NVMe (node 0).
"""
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import Patch

# ── measured results ──────────────────────────────────────────────────────
backends = ["NoOffload", "CPUOffload", "SharedStorage", "Certus-SPDK"]
colors   = {"NoOffload": "#9aa0a6", "CPUOffload": "#4c8bf5",
            "SharedStorage": "#f4a83d", "Certus-SPDK": "#34a853"}
wall_s   = {"NoOffload": 1433.7, "CPUOffload": 1389.4,
            "SharedStorage": 879.6, "Certus-SPDK": 1447.8}
tok_s    = {"NoOffload": 565, "CPUOffload": 583,
            "SharedStorage": 921, "Certus-SPDK": 559}
base = wall_s["NoOffload"]

rounds = list(range(1, 13))
per_round = {
    "NoOffload":     [25.1, 36.5, 50.3, 66.3, 87.2, 107.0, 124.1, 144.2, 164.9, 184.8, 206.2, 223.7],
    "CPUOffload":    [23.5, 33.6, 41.0, 64.2, 81.5, 101.1, 120.6, 140.9, 161.5, 182.1, 204.2, 221.8],
    "SharedStorage": [24.7, 36.4, 48.3, 51.8, 60.3, 66.9, 73.8, 83.2, 93.0, 99.8, 109.6, 118.2],
    # Certus per-round wall from the tqdm "Processed prompts 100%" bars in
    # certus-spdk.log (Σ ~1415s ≈ 1447.8s total).
    "Certus-SPDK":   [26, 36, 44, 50, 73, 95, 115, 137, 167, 185, 241, 246],
}
ss_read  = [0.00, 0.00, 1.24, 6.17, 4.55, 23.35, 28.62, 36.95, 45.02, 55.88, 65.85, 73.69]
ss_write = [0.03, 0.75, 10.72, 8.66, 8.47, 9.72, 7.94, 9.32, 10.72, 9.20, 10.42, 9.79]

# Certus-SPDK per-round SSD I/O — real block-device counters (GetIoStats), the
# server was rebuilt with the `rw-telemetry` feature for this run. Per-round
# deltas from certus-spdk.log ("[run] round N: ... ssd_read=X ssd_write=Y").
cs_read  = [0.00, 0.13, 7.48, 18.29, 27.21, 36.41, 45.94, 55.66, 65.50, 75.29, 85.06, 94.89]
cs_write = [1.90, 7.63, 8.72, 8.89, 9.16, 9.43, 9.66, 9.81, 9.80, 9.72, 9.80, 9.96]
CS_READ_SUM  = sum(cs_read)    # ~511.9 GiB read back from NVMe
CS_WRITE_SUM = sum(cs_write)   # ~104.5 GiB written (= 104 GiB resident)

# ── figure ────────────────────────────────────────────────────────────────
fig = plt.figure(figsize=(16, 9), dpi=120)
fig.patch.set_facecolor("white")
gs = fig.add_gridspec(2, 2, height_ratios=[1.0, 1.0], hspace=0.34, wspace=0.22,
                      left=0.06, right=0.97, top=0.86, bottom=0.08)

fig.suptitle("KV-offload replay — 4 backends @ vLLM 0.23.0",
             fontsize=22, fontweight="bold", x=0.06, ha="left", y=0.965)
fig.text(0.06, 0.905,
         "Llama-3-8B · 450 convs × 12 turns · 5400 generations · single A30 · SPDK NVMe (NUMA 0), 13 GiB DRAM tier",
         fontsize=12, color="#444", ha="left")

# Panel 1: wall time bars ----------------------------------------------------
ax1 = fig.add_subplot(gs[0, 0])
xs = range(len(backends))
bars = ax1.bar(xs, [wall_s[b] for b in backends],
               color=[colors[b] for b in backends], width=0.62, edgecolor="white")
ax1.set_xticks(list(xs)); ax1.set_xticklabels(backends, fontsize=11)
ax1.set_ylabel("wall time (s), lower is better", fontsize=11)
ax1.set_title("Total run time & throughput", fontsize=13, fontweight="bold", loc="left")
ax1.set_ylim(0, max(wall_s.values()) * 1.18)
for i, b in enumerate(backends):
    sp = base / wall_s[b]
    ax1.text(i, wall_s[b] + 18,
             f"{wall_s[b]:.0f}s\n{tok_s[b]} tok/s\n{sp:.2f}×",
             ha="center", va="bottom", fontsize=10, fontweight="bold")
ax1.grid(axis="y", alpha=0.25); ax1.set_axisbelow(True)

# Panel 2: per-round latency -------------------------------------------------
ax2 = fig.add_subplot(gs[0, 1])
for b in ["NoOffload", "CPUOffload", "SharedStorage", "Certus-SPDK"]:
    ax2.plot(rounds, per_round[b], marker="o", ms=4, lw=2.2,
             color=colors[b], label=b)
ax2.set_xlabel("conversation turn (round)", fontsize=11)
ax2.set_ylabel("round latency (s)", fontsize=11)
ax2.set_title("Per-round latency — SharedStorage stays flat (serves KV from disk)",
              fontsize=13, fontweight="bold", loc="left")
ax2.set_xticks(rounds)
ax2.legend(fontsize=10, loc="upper left", frameon=False)
ax2.grid(alpha=0.25); ax2.set_axisbelow(True)
ax2.annotate("Certus-SPDK tracks NoOffload early,\nthen runs SLOWER in the tail\n(SSD read latency climbs to 3.2 ms)",
             xy=(12, per_round["Certus-SPDK"][11]), xytext=(5.4, 205),
             fontsize=9.0, color="#34682f",
             arrowprops=dict(arrowstyle="->", color="#34a853", lw=1))

# Panel 3: per-round SSD I/O — SharedStorage vs Certus (both disk-backed) -----
ax3 = fig.add_subplot(gs[1, 0])
w = 0.4
ax3.bar([r - w/2 for r in rounds], ss_read, width=w, color="#f4a83d", label="SharedStorage read")
ax3.bar([r + w/2 for r in rounds], cs_read, width=w, color="#34a853", label="Certus-SPDK read")
ax3.set_xlabel("round", fontsize=11); ax3.set_ylabel("SSD read per round (GiB)", fontsize=11)
ax3.set_title("Per-round SSD reads — Certus reloads MORE KV than SharedStorage",
              fontsize=13, fontweight="bold", loc="left")
ax3.set_xticks(rounds)
handles = [Patch(color="#f4a83d", label=f"SharedStorage read  (Σ 341.3 GiB)"),
           Patch(color="#34a853", label=f"Certus-SPDK read     (Σ {CS_READ_SUM:.0f} GiB)")]
ax3.legend(handles=handles, fontsize=9.5, loc="upper left", frameon=False)
ax3.grid(axis="y", alpha=0.25); ax3.set_axisbelow(True)
ax3.text(0.5, -0.30,
         f"Certus writes Σ {CS_WRITE_SUM:.0f} GiB (= 104 GiB resident on NVMe), reads Σ {CS_READ_SUM:.0f} GiB back.  "
         "rw-telemetry ON — real GetIoStats block-device counters.",
         transform=ax3.transAxes, fontsize=8.6, color="#34682f", ha="center", va="top",
         bbox=dict(boxstyle="round,pad=0.4", fc="#eaf5ea", ec="#34a853", lw=1))

# Panel 4: takeaways ---------------------------------------------------------
ax4 = fig.add_subplot(gs[1, 1]); ax4.axis("off")
lines = [
    ("Takeaways", None, True),
    ("• SharedStorage — fastest: 879.6s / 921 tok/s / 1.63×.", "#c77d20", False),
    ("   Serves prior-turn KV from disk (Σ 341 GiB read); late-round", "#666", False),
    ("   prefill is cut and per-round latency stays flat.", "#666", False),
    ("• Certus-SPDK — offload + load path both fire now: writes", "#34a853", False),
    ("   Σ 104 GiB (= resident), reads Σ 512 GiB back from NVMe —", "#666", False),
    ("   MORE disk I/O than SharedStorage. Yet 1447.8s ≈ NoOffload", "#666", False),
    ("   (1.00×): the reloaded KV isn't cutting prefill on the", "#666", False),
    ("   critical path (read latency climbs 0.9→3.2 ms/round).", "#666", False),
    ("• CPUOffload — 1389.4s / 1.03×: marginal at this scale.", "#4c8bf5", False),
    ("• NoOffload — baseline 1433.7s / 565 tok/s.", "#9aa0a6", False),
    ("", None, False),
    ("Certus now reads heavily from SSD but sees no speedup, while", "#333", False),
    ("SharedStorage reads less and gets 1.63×. Next: find why the", "#333", False),
    ("Certus reloads aren't shortcutting prefill (placement / hit", "#333", False),
    ("attribution / sync vs async on the decode path).", "#333", False),
]
y = 0.98
for txt, col, hdr in lines:
    if hdr:
        ax4.text(0.0, y, txt, fontsize=14, fontweight="bold", va="top")
        y -= 0.085
    else:
        ax4.text(0.0, y, txt, fontsize=10.5, color=col or "#000", va="top")
        y -= 0.062

out = "/home/bdh/ai-native-storage-certus/benchmarks/kv-offload-replay/slide-vllm0.23-4way-fixed.png"
fig.savefig(out, facecolor="white", bbox_inches="tight")
print("wrote", out)
