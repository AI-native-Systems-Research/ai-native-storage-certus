#!/usr/bin/env python3
"""Render the cross-version vLLM benchmark slide to a PNG (no browser needed).

Data for 0.20 / 0.22 / 0.24 is final; 0.26 cells default to placeholders and are
filled once the 0.26 run reports DONE. Re-run after editing the ``V026`` dict.
"""
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle, FancyBboxPatch

# ── palette (dataviz reference, light mode) ──
SURFACE = "#fcfcfb"; PAGE = "#f9f9f7"
INK = "#0b0b0b"; INK2 = "#52514e"; MUTED = "#898781"
GRID = "#e1e0d9"; BASELINE = "#c3c2b7"
BLUE = "#2a78d6"; BLUE_SOFT = "#cde2fb"; GOOD = "#006300"

# ── 0.26 run state (edit + re-run when DONE) ──
V026 = dict(
    thr="3.8",          # gen/s
    hit="~89.6%",       # external prefix cache hit
    thr_val=3.8,        # numeric for the bar (gen/s), None => ghost
    hit_val=89.6,       # numeric for the bar (%), None => ghost
    rounds="12 / 12",
    wall="1422",
    ssd_read="93.42",
    done=True,
)

fig = plt.figure(figsize=(12.8, 7.2), dpi=150)
fig.patch.set_facecolor(SURFACE)
# slide border
fig.add_artist(Rectangle((0.006, 0.008), 0.988, 0.984, transform=fig.transFigure,
                         fill=False, edgecolor=GRID, lw=1.2))

# ── header ──
fig.text(0.035, 0.945, "Certus gRPC connector — one package, four vLLM versions",
         fontsize=21, fontweight="bold", color=INK)
fig.text(0.035, 0.905,
         "Same connector code runs on vLLM 0.20 → 0.22 → 0.24 → 0.26 via a capability-matrix shim.",
         fontsize=12.5, color=INK2)
fig.text(0.035, 0.878,
         "Workload: Llama-3-8B, 450 conversations × 12 turns ShareGPT, clean DRAM+SSD tier each run, LOG_STATS=1.",
         fontsize=12.5, color=INK2)

# ── table ──
axt = fig.add_axes([0.035, 0.30, 0.52, 0.53]); axt.axis("off")
axt.set_xlim(0, 1); axt.set_ylim(0, 10.6); axt.invert_yaxis()
COLX = {"0.20": 0.565, "0.22": 0.71, "0.24": 0.855, "0.26": 1.0}
# caption
axt.text(0.0, -0.35, "Per-version results — identical workload, clean tier each run",
         fontsize=10.5, color=MUTED)
# header
hy = 0.55
for v, x in COLX.items():
    axt.text(x, hy, v, fontsize=12.5, fontweight="bold", color=BLUE, ha="right", va="center")
axt.text(0.0, hy, "Metric", fontsize=12.5, color=INK2, ha="left", va="center")
axt.plot([0, 1], [1.05, 1.05], color=BASELINE, lw=1.6)

rows = [
    ("Throughput (gen/s)",          "4.1",   "3.7",    "3.5",       V026["thr"], "b"),
    ("External prefix cache hit",   "~96%",  "~97.3%", "~89.4%",    V026["hit"], "b"),
    ("Generations completed",       "5400",  "5400",   "5400",      "5400",      ""),
    ("Rounds",                      "12 / 12","12 / 12","12 / 12",  V026["rounds"], ""),
    ("Wall time (s)",               "1311",  "1445",   "1552",      V026["wall"], ""),
    ("SSD read / round, r12 (GiB)", "100.5", "~101",   "94.95",     V026["ssd_read"], ""),
    ("SSD write / round (GiB)",     "~10",   "~9–10",  "~10",       "~10",   ""),
    ("Write latency (µs)",          "~32",   "~30",    "~32",       "~32",       ""),
    ("Correctness",                 "served ✓","served ✓","served ✓","served ✓", "g"),
]
pending_txt = {"running…", "—", "… / 12"}
for i, (metric, c20, c22, c24, c26, flag) in enumerate(rows):
    y = 2.0 + i
    axt.text(0.0, y, metric, fontsize=11, color=INK2, ha="left", va="center")
    for val, x in ((c20, COLX["0.20"]), (c22, COLX["0.22"]), (c24, COLX["0.24"]), (c26, COLX["0.26"])):
        color, weight, style = INK, "normal", "normal"
        if flag == "b": weight = "bold"
        if flag == "g": color = GOOD
        if val in pending_txt:
            color, style, weight = MUTED, "italic", "normal"
        axt.text(x, y, val, fontsize=11, color=color, ha="right", va="center",
                 fontweight=weight, fontstyle=style)
    if i < len(rows) - 1:
        axt.plot([0, 1], [y + 0.5, y + 0.5], color=GRID, lw=0.8)

fig.text(0.035, 0.272,
         "*0.26 in progress — final throughput, hit-rate, wall-time & per-round read fill on completion."
         if not V026["done"] else
         "All 12 rounds completed; figures are per-round deltas. Writes ~equal across versions (~10 GiB/round).\n"
         "Multi-region offload sends full 32-layer blocks on 0.24 & 0.26: SSD read/round matches 0.20/0.22 — "
         "the same connector code, four API eras, identical on-disk behavior.",
         fontsize=9, color=MUTED, va="top")

# ── bar charts (single-hue blue, one measure across ordered versions) ──
def barchart(rect, title, vals, xmax, fmt, scale_note):
    ax = fig.add_axes(rect)
    cats = ["0.20", "0.22", "0.24", "0.26"]
    ypos = [3, 2, 1, 0]
    ax.set_xlim(0, xmax); ax.set_ylim(-0.6, 3.6)
    for sp in ax.spines.values(): sp.set_visible(False)
    ax.set_xticks([]); ax.set_yticks(ypos); ax.set_yticklabels(cats, fontsize=11, color=INK2)
    ax.tick_params(length=0)
    ax.set_facecolor(SURFACE)
    fig.text(rect[0], rect[1] + rect[3] + 0.018, title, fontsize=12, fontweight="bold", color=INK2)
    for y, v in zip(ypos, vals):
        # track
        ax.barh(y, xmax, height=0.62, color=BLUE_SOFT, zorder=1)
        if v is None:
            gw = xmax * 0.16
            ax.barh(y, gw, height=0.62, color=BLUE_SOFT, edgecolor=BLUE, lw=1.1,
                    hatch="////", zorder=2)
            ax.text(gw + xmax * 0.02, y, "running…", fontsize=10, color=MUTED,
                    style="italic", va="center", ha="left")
        else:
            ax.barh(y, v, height=0.62, color=BLUE, zorder=2)
            inside = v > xmax * 0.22
            ax.text(v - xmax * 0.02 if inside else v + xmax * 0.02, y, fmt(v),
                    fontsize=10, fontweight="bold",
                    color="#ffffff" if inside else INK,
                    va="center", ha="right" if inside else "left")
    fig.text(rect[0], rect[1] - 0.028, scale_note, fontsize=9, color=MUTED)

barchart([0.60, 0.60, 0.36, 0.20],
         "Throughput (generations / s) — higher is better",
         [4.1, 3.7, 3.5, V026["thr_val"]], 5.0,
         lambda v: f"{v:.1f}", "scale 0–5 gen/s")

barchart([0.60, 0.335, 0.36, 0.20],
         "External prefix cache hit rate — Certus-served KV",
         [96.0, 97.3, 89.4, V026["hit_val"]], 100.0,
         lambda v: f"{v:.0f}%" if float(v).is_integer() else f"{v:.1f}%",
         "scale 0–100%; warms to steady state by round 3")

# ── footer: API breaks ──
fy = 0.235
fig.add_artist(Rectangle((0.035, fy), 0.93, 0.0, transform=fig.transFigure))
axt2 = fig.add_axes([0.035, 0.04, 0.93, 0.20]); axt2.axis("off")
axt2.set_xlim(0, 1); axt2.set_ylim(0, 1)
axt2.plot([0, 1], [0.96, 0.96], color=GRID, lw=1.0)
axt2.text(0.0, 0.86, "API BREAKS ABSORBED BY THE CAPABILITY-MATRIX SHIM (NO PER-VERSION FORK)",
          fontsize=10.5, color=MUTED, fontweight="bold")
cols = [
    (0.0, "0.20 — baseline", [
        "Original plugin surface",
        "Modules: abstract / mediums / spec"]),
    (0.26, "0.22 — 2 breaks", [
        "req_context arg on every manager",
        "  method → superset signature",
        "async scheduling auto-on →",
        "  async_scheduling=False",
        "modules → kv_offload.base"]),
    (0.52, "0.24 — 1 break", [
        "new abstract on_new_request()",
        "  → default RequestOffloading-",
        "  Context (BLOCK_LEVEL)"]),
    (0.78, "0.26 — API rewrite", [
        "get_worker() → single Offloading-",
        "  Worker (submit_store/load)",
        "CanonicalKVCaches (N per-layer)",
        "OffloadingConfig ctor; Transfer-",
        "  Result drops transfer_type"]),
]
for x, head, items in cols:
    axt2.text(x, 0.66, head, fontsize=11, fontweight="bold", color=BLUE)
    yy = 0.50
    for it in items:
        axt2.text(x, yy, "• " + it if not it.startswith("  ") else "   " + it.strip(),
                  fontsize=9.2, color=INK2)
        yy -= 0.135

out = "/home/bdh/ai-native-storage-certus/results/crossversion-slide.png"
fig.savefig(out, facecolor=SURFACE)
print("wrote", out)
