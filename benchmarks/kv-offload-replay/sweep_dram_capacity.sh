#!/usr/bin/env bash
# sweep_dram_capacity.sh — sweep DRAM tier sizes and produce a throughput + TTFT
# vs capacity HTML report.
#
# Outer loop = variant (one backend at a time, avoids host reconfiguration churn),
# inner loop = DRAM size. Each (variant, size) point calls profile_all.sh.
#
# Usage:
#   ./sweep_dram_capacity.sh --dram-sizes "32 64 128 512" \
#       --only certus-spdk,tiered-cpu-fs --workload long-doc-qa \
#       --device-pci 0000:05:00.0 --device-pci 0000:41:00.0 --gpu 4
#
#   ./sweep_dram_capacity.sh --dram-sizes "32 64 128" --only certus-spdk \
#       --workload long-doc-qa --logdir /tmp/dram-sweep
#
# All flags not consumed here are passed through to profile_all.sh.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROFILE_ALL="$SCRIPT_DIR/profile_all.sh"

DRAM_SIZES="${DRAM_SIZES:-32 64 128 512}"
VARIANTS=""
SWEEP_DIR=""
PASSTHROUGH=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dram-sizes)   DRAM_SIZES="$2"; shift 2 ;;
        --only)         VARIANTS="$2"; PASSTHROUGH+=(--only "$2"); shift 2 ;;
        --logdir)       SWEEP_DIR="$2"; shift 2 ;;
        *)              PASSTHROUGH+=("$1"); shift ;;
    esac
done

STAMP="$(date +%Y%m%d_%H%M%S)"
[[ -z "$SWEEP_DIR" ]] && SWEEP_DIR="$(pwd)/dram-sweep-${STAMP}"
mkdir -p "$SWEEP_DIR"

# Default variants if --only not given
[[ -z "$VARIANTS" ]] && VARIANTS="certus-spdk"

IFS=',' read -ra VARIANT_LIST <<< "$VARIANTS"

log() { echo "[sweep] $(date +%H:%M:%S) $*"; }

log "DRAM capacity sweep"
log "  sizes:    ${DRAM_SIZES}"
log "  variants: ${VARIANT_LIST[*]}"
log "  output:   ${SWEEP_DIR}"
log ""

FAILED=0

for variant in "${VARIANT_LIST[@]}"; do
    log "━━ variant: ${variant} ━━"
    for size in $DRAM_SIZES; do
        run_dir="${SWEEP_DIR}/${variant}/${size}G"
        log "── ${variant} @ ${size}G ──"

        # Strip --only from passthrough (we control it per-variant)
        ARGS=()
        skip_next=0
        for a in "${PASSTHROUGH[@]}"; do
            if [[ $skip_next -eq 1 ]]; then skip_next=0; continue; fi
            if [[ "$a" == "--only" ]]; then skip_next=1; continue; fi
            ARGS+=("$a")
        done

        "$PROFILE_ALL" \
            --only "$variant" \
            --memory-tier-size "${size}G" \
            --logdir "$run_dir" \
            "${ARGS[@]}" || {
                log "WARN: ${variant} @ ${size}G exited $? — continuing"
                FAILED=$((FAILED + 1))
            }

        log "  → ${run_dir}/"
        echo ""
    done
done

# ── Generate HTML report ─────────────────────────────────────────────────────
REPORT="${SWEEP_DIR}/dram-sweep-report.html"
log "generating report → ${REPORT}"

python3 - "$SWEEP_DIR" "$DRAM_SIZES" "$VARIANTS" "$REPORT" << 'PYEOF'
import json, os, sys, math

sweep_dir = sys.argv[1]
dram_sizes = [int(s) for s in sys.argv[2].split()]
variants_str = sys.argv[3]
report_path = sys.argv[4]
variants = [v.strip() for v in variants_str.split(",")]

# ── Collect data ──────────────────────────────────────────────────────────────
# rows[variant] = [{dram_gib, req_per_s, tok_per_s, wall_s, gens, rounds,
#                   ttft_p50_ms, ttft_p95_ms}, ...]
rows = {}
for variant in variants:
    rows[variant] = []
    for size in dram_sizes:
        run_dir = os.path.join(sweep_dir, variant, f"{size}G")
        point = {"dram_gib": size, "req_per_s": None, "tok_per_s": None,
                 "wall_s": None, "gens": None, "rounds": None,
                 "ttft_p50_ms": None, "ttft_p95_ms": None}

        # Read result JSON
        slug = variant.lower()
        rfile = os.path.join(run_dir, f"result-{slug}.json")
        if os.path.isfile(rfile):
            try:
                d = json.load(open(rfile))
                wall = d.get("wall_s")
                gens = d.get("generations")
                tps = d.get("tokens_per_sec")
                if wall and gens and wall > 0:
                    point["req_per_s"] = gens / wall
                    point["wall_s"] = wall
                    point["gens"] = gens
                    point["rounds"] = d.get("rounds")
                if tps:
                    point["tok_per_s"] = tps
            except (json.JSONDecodeError, KeyError):
                pass

        # Read TTFT from histograms JSON
        hfile = os.path.join(run_dir, f"{slug}.histograms.json")
        # Also check Certus-SPDK which may capitalize
        if not os.path.isfile(hfile):
            for f in os.listdir(run_dir) if os.path.isdir(run_dir) else []:
                if f.endswith(".histograms.json"):
                    hfile = os.path.join(run_dir, f)
                    break
        if os.path.isfile(hfile):
            try:
                hists = json.load(open(hfile))
                ttft = hists.get("vllm:time_to_first_token_seconds")
                if ttft:
                    buckets = ttft.get("buckets", {})
                    count = ttft.get("count", 0)
                    if count > 0:
                        def hist_pct(bkts, cnt, p):
                            target = p * cnt
                            def _le(k):
                                return float("inf") if k in ("+Inf","inf","Inf") else float(k)
                            for le in sorted(bkts, key=_le):
                                if bkts[le] >= target:
                                    v = _le(le)
                                    return v if v != float("inf") else None
                            return None
                        p50 = hist_pct(buckets, count, 0.50)
                        p95 = hist_pct(buckets, count, 0.95)
                        if p50 is not None:
                            point["ttft_p50_ms"] = p50 * 1000
                        if p95 is not None:
                            point["ttft_p95_ms"] = p95 * 1000
            except (json.JSONDecodeError, KeyError):
                pass

        rows[variant].append(point)

# ── SVG chart helpers ─────────────────────────────────────────────────────────
W, H = 680, 300
PAD_L, PAD_R, PAD_T, PAD_B = 64, 130, 20, 48

def fmt_num(v):
    if v is None: return "—"
    if v >= 1000: return f"{v:,.0f}"
    if v >= 10: return f"{v:.1f}"
    return f"{v:.2f}"

COLORS = ["var(--s1)", "var(--s2)", "var(--s3)", "var(--s4)"]
DASHES = ["", "6,3", "3,3", "8,4,2,4"]

def line_chart(x_labels, series, x_title, y_title, y_max=None):
    pw = W - PAD_L - PAD_R
    ph = H - PAD_T - PAD_B

    all_vals = [v for _, _, vals, _ in series for v in vals if v is not None]
    if not all_vals:
        return '<p style="color:var(--muted)">No data</p>'
    if y_max is None:
        y_max = max(all_vals) * 1.15
    y_min = 0

    def sx(i): return PAD_L + i * pw / max(len(x_labels) - 1, 1)
    def sy(v): return PAD_T + ph - (v - y_min) / max(y_max - y_min, 1e-9) * ph

    lines = [f'<svg viewBox="0 0 {W} {H}" xmlns="http://www.w3.org/2000/svg" '
             f'style="width:100%;max-width:{W}px;font:12px system-ui">']

    # grid
    n_grid = 5
    for i in range(n_grid + 1):
        y = PAD_T + ph * i / n_grid
        v = y_max - (y_max - y_min) * i / n_grid
        lines.append(f'<line x1="{PAD_L}" y1="{y:.1f}" x2="{PAD_L+pw}" y2="{y:.1f}" '
                     f'stroke="var(--grid)" stroke-width="1"/>')
        lines.append(f'<text x="{PAD_L-8}" y="{y:.1f}" text-anchor="end" '
                     f'dominant-baseline="middle" fill="var(--muted)" font-size="11">'
                     f'{fmt_num(v)}</text>')

    # x labels
    for i, lbl in enumerate(x_labels):
        x = sx(i)
        lines.append(f'<text x="{x:.1f}" y="{H-6}" text-anchor="middle" '
                     f'fill="var(--muted)" font-size="11">{lbl}</text>')
        lines.append(f'<line x1="{x:.1f}" y1="{PAD_T}" x2="{x:.1f}" y2="{PAD_T+ph}" '
                     f'stroke="var(--grid)" stroke-width="0.5" stroke-dasharray="3,3"/>')

    # x title
    lines.append(f'<text x="{PAD_L + pw/2}" y="{H+2}" text-anchor="middle" '
                 f'fill="var(--muted)" font-size="12">{x_title}</text>')

    # series
    end_labels = []
    for si, (name, color, vals, dash) in enumerate(series):
        pts = [(i, v) for i, v in enumerate(vals) if v is not None]
        if not pts:
            continue
        d = " ".join(f"{'M' if j==0 else 'L'}{sx(i):.1f},{sy(v):.1f}" for j,(i,v) in enumerate(pts))
        sd = f' stroke-dasharray="{dash}"' if dash else ""
        lines.append(f'<path d="{d}" fill="none" stroke="{color}" stroke-width="2.5"{sd}/>')
        for i, v in pts:
            lines.append(f'<circle cx="{sx(i):.1f}" cy="{sy(v):.1f}" r="3.5" '
                         f'fill="{color}" stroke="var(--bg)" stroke-width="1.5"/>')
            tip = f"{name}: {fmt_num(v)}"
            lines.append(f'<circle cx="{sx(i):.1f}" cy="{sy(v):.1f}" r="12" '
                         f'fill="transparent" data-tip="{tip}"/>')
        # end label
        last_v = pts[-1][1]
        end_labels.append((sy(last_v), name, color))

    # nudge end labels apart
    end_labels.sort(key=lambda t: t[0])
    for i in range(1, len(end_labels)):
        if end_labels[i][0] - end_labels[i-1][0] < 14:
            end_labels[i] = (end_labels[i-1][0] + 14, end_labels[i][1], end_labels[i][2])
    for y, name, color in end_labels:
        lines.append(f'<text x="{PAD_L+pw+8}" y="{y:.1f}" dominant-baseline="middle" '
                     f'fill="{color}" font-size="11">{name}</text>')

    lines.append('</svg>')
    return "\n".join(lines)

# ── Build report ──────────────────────────────────────────────────────────────
x_labels = [f"{s}" for s in dram_sizes]

# Chart 1: req/s
series1 = []
for vi, variant in enumerate(variants):
    vals = [p["req_per_s"] for p in rows[variant]]
    series1.append((variant, COLORS[vi % len(COLORS)], vals, ""))

chart1 = line_chart(x_labels, series1, "DRAM tier (GiB)", "Requests / sec")

# Chart 2: TTFT p50 + p95
series2 = []
for vi, variant in enumerate(variants):
    p50s = [p["ttft_p50_ms"] for p in rows[variant]]
    p95s = [p["ttft_p95_ms"] for p in rows[variant]]
    c = COLORS[vi % len(COLORS)]
    series2.append((f"{variant} p50", c, p50s, ""))
    series2.append((f"{variant} p95", c, p95s, "6,3"))

chart2 = line_chart(x_labels, series2, "DRAM tier (GiB)", "TTFT (ms)")

# Data table
table_rows = []
for variant in variants:
    for p in rows[variant]:
        table_rows.append(
            f'<tr><td>{variant}</td><td>{p["dram_gib"]}</td>'
            f'<td>{fmt_num(p["req_per_s"])}</td>'
            f'<td>{fmt_num(p["tok_per_s"])}</td>'
            f'<td>{fmt_num(p["ttft_p50_ms"])}</td>'
            f'<td>{fmt_num(p["ttft_p95_ms"])}</td>'
            f'<td>{fmt_num(p["wall_s"])}</td>'
            f'<td>{fmt_num(p["gens"])}</td></tr>')

html = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>DRAM Capacity Sweep</title>
<style>
:root {{
  --bg: #fcfcfb; --fg: #0b0b0b; --muted: #6b6b6b; --surface: #f4f4f2;
  --grid: #e0e0de; --border: #d8d8d4;
  --s1: #2a78d6; --s2: #eb6834; --s3: #1baf7a; --s4: #eda100;
}}
@media (prefers-color-scheme: dark) {{
  :root:not([data-theme="light"]) {{
    --bg: #0d0d0d; --fg: #e8e8e8; --muted: #8a8a8a; --surface: #1a1a19;
    --grid: #2a2a28; --border: #333330;
    --s1: #3987e5; --s2: #d95926; --s3: #199e70; --s4: #c98500;
  }}
}}
:root[data-theme="dark"] {{
  --bg: #0d0d0d; --fg: #e8e8e8; --muted: #8a8a8a; --surface: #1a1a19;
  --grid: #2a2a28; --border: #333330;
  --s1: #3987e5; --s2: #d95926; --s3: #199e70; --s4: #c98500;
}}
body {{ background: var(--bg); color: var(--fg); font: 15px/1.5 system-ui, sans-serif;
       max-width: 820px; margin: 2rem auto; padding: 0 16px; }}
h1 {{ font-size: 1.4rem; margin: 0 0 0.3rem; }}
.sub {{ color: var(--muted); font-size: 0.9rem; margin-bottom: 1.5rem; }}
section {{ background: var(--surface); border: 1px solid var(--border);
           border-radius: 10px; padding: 1.2rem; margin-bottom: 1.2rem; }}
section h2 {{ font-size: 1rem; margin: 0 0 0.8rem; }}
table {{ width: 100%; border-collapse: collapse; font-size: 0.85rem;
         font-variant-numeric: tabular-nums; }}
th {{ text-align: left; padding: 6px 8px; border-bottom: 2px solid var(--border);
      color: var(--muted); font-weight: 600; }}
td {{ padding: 6px 8px; border-bottom: 1px solid var(--border); }}
#tip {{ display: none; position: fixed; background: var(--surface);
        border: 1px solid var(--border); border-radius: 6px; padding: 4px 10px;
        font: 12px/1.4 system-ui; white-space: pre; pointer-events: none;
        box-shadow: 0 2px 8px rgba(0,0,0,.15); z-index: 9; }}
</style>
</head>
<body>
<h1>DRAM Capacity Sweep</h1>
<p class="sub">Throughput and TTFT vs DRAM tier size &mdash; long-doc-qa workload via profile_all.sh</p>

<section>
<h2>Requests / sec vs DRAM tier</h2>
{chart1}
</section>

<section>
<h2>TTFT vs DRAM tier</h2>
{chart2}
</section>

<section>
<h2>Data</h2>
<table>
<tr><th>Variant</th><th>DRAM (GiB)</th><th>req/s</th><th>tok/s</th>
    <th>TTFT p50 (ms)</th><th>TTFT p95 (ms)</th><th>Wall (s)</th><th>Gens</th></tr>
{"".join(table_rows)}
</table>
</section>

<div id="tip"></div>
<script>
const tip=document.getElementById("tip");
document.addEventListener("mousemove",e=>{{
  const t=e.target.closest("[data-tip]");
  if(!t){{tip.style.display="none";return}}
  tip.textContent=t.dataset.tip;tip.style.display="block";
  let x=e.clientX+14,y=e.clientY+14;
  if(x+tip.offsetWidth>innerWidth)x=e.clientX-tip.offsetWidth-8;
  if(y+tip.offsetHeight>innerHeight)y=e.clientY-tip.offsetHeight-8;
  tip.style.left=x+"px";tip.style.top=y+"px";
}});
document.addEventListener("mouseleave",()=>tip.style.display="none",true);
</script>
</body>
</html>"""

with open(report_path, "w") as f:
    f.write(html)
print(f"[sweep] report written to {report_path}")
PYEOF

log ""
log "=== sweep complete ==="
log "  results: ${SWEEP_DIR}/"
log "  report:  ${REPORT}"
[[ $FAILED -gt 0 ]] && log "  WARNING: ${FAILED} run(s) failed"
