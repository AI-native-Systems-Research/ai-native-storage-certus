#!/usr/bin/env python3
"""Sweep the DRAM tier size and report how much of the KV lookup stream SSD serves.

This module is also the shared sweep engine: ``sweep_turns_ssd.py`` calls
``main("turns")`` and ``sweep_concurrency_ssd.py`` calls ``main("concurrency")``
to sweep turns per conversation or concurrent sessions with the same runs and
report.

Runs ``kv_cache_dynamics.py`` through ``run-burstgpt-example.sh`` (BurstGPT
think-time intervals + the cc131k model / cache / token configuration) once per
DRAM size, and — unless ``--no-baseline`` — once more per size with the SSD tier
disabled. The no-SSD run shows what SSD tiering buys: lookups that SSD serves in
the tiered run are recomputed (prefilled) without it.

Writes a self-contained HTML report: workload summary, the interval and turn
distributions driving the run, % of lookups served by HBM / DRAM / SSD vs DRAM
size, SSD hits vs no-SSD recompute, and a data table.

Defaults below can be overridden by flag or by the environment variable named in
each help string; any other ``run-cc131k-example.sh`` variable (``CONCURRENT``,
``SSD_GB``, ``PREFIX_TOKENS``, ``ADMISSION``, ...) passes through from the
environment untouched.

Examples
--------
    ./sweep_dram_ssd.py                                  # writes reports/dram-sweep-report.html
    HBM_GB=20 CONCURRENT=64 ./sweep_dram_ssd.py -o c64.html
    ./sweep_dram_ssd.py --turn-dist cc-trace-weka-062126-turns.yaml
    ./sweep_dram_ssd.py --dram-sizes 0,8,32,128,512
"""

from __future__ import annotations

import argparse
import concurrent.futures
import html
import json
import math
import os
import subprocess
import sys
import tempfile

import yaml

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RUNNER = os.path.join(SCRIPT_DIR, "run-burstgpt-example.sh")
DEFAULT_DIST = os.path.join(SCRIPT_DIR, "burstgpt-3-intervals.yaml")
REPORT_DIR = os.path.join(SCRIPT_DIR, "reports")

TIERS = ("HBM", "DRAM", "SSD")

# What a sweep varies. Each axis names the run-cc131k-example.sh variable it sets
# per point, how to label points, and its report wording. The other knob is held
# knobs are held fixed (DRAM via --dram-gb, turns via --num-turns) unless swept.
AXES = {
    "dram": {
        "env": "DRAM_GB", "values_env": "DRAM_SIZES", "flag": "--dram-sizes",
        "default": "8,16,32,64,128,256", "unit": "GiB",
        "label": lambda v: fmt_gb(v), "x_title": "DRAM tier size",
        "noun": "DRAM tier", "by": "DRAM size",
        "output": "dram-sweep-report.html", "script": "sweep_dram_ssd.py",
    },
    "turns": {
        "env": "NUM_TURNS", "values_env": "TURN_COUNTS", "flag": "--turn-counts",
        "default": "8,16,32,64,128,256", "unit": "turns",
        "label": lambda v: f"{v:g}", "x_title": "turns per conversation",
        "noun": "turns per conversation", "by": "turn count",
        "output": "turns-sweep-report.html", "script": "sweep_turns_ssd.py",
    },
    "concurrency": {
        "env": "CONCURRENT", "values_env": "CONCURRENCY_LEVELS", "flag": "--concurrency",
        "default": "1,2,4,8,16,32,64,128", "unit": "sessions",
        "label": lambda v: f"{v:g}", "x_title": "concurrent sessions",
        "noun": "concurrent sessions", "by": "concurrency",
        "output": "concurrency-sweep-report.html", "script": "sweep_concurrency_ssd.py",
        # The first concurrency wave is warmup, so the total must exceed the
        # largest level; 4x 128 keeps >= 75% of conversations measured.
        "sessions_default": "512",
    },
}


def run_once(env_overrides, json_path):
    """Run the simulator with ``env_overrides``; return its parsed JSON output."""
    env = dict(os.environ)
    env.update(env_overrides)
    proc = subprocess.run([RUNNER, "--json", json_path], env=env,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"run failed ({env_overrides}):\n{proc.stderr.strip()}")
    with open(json_path) as f:
        return json.load(f)


def decode_shares(result):
    """Per-tier % of decode-step context reads, plus % recomputed; None if off."""
    st = result["stats"]
    reads = st.get("decode_lookup_tokens", 0)
    if not reads:
        return None
    names = [t["name"] for t in result["tiers"]]
    shares = {n: 0.0 for n in TIERS}
    for name, hits in zip(names, st["decode_tier_hit_tokens"]):
        shares[name] = hits / reads * 100
    shares["miss"] = st["decode_recompute_tokens"] / reads * 100
    return shares


def tier_shares(result):
    """Per-tier % of looked-up prior-context tokens, plus % missed (recomputed)."""
    st = result["stats"]
    lookup = st["lookup_tokens"]
    names = [t["name"] for t in result["tiers"]]
    shares = {n: 0.0 for n in TIERS}
    for name, hits in zip(names, st["tier_hit_tokens"]):
        shares[name] = hits / lookup * 100 if lookup else 0.0
    missed = lookup - sum(st["tier_hit_tokens"])
    shares["miss"] = missed / lookup * 100 if lookup else 0.0
    return shares


# --- formatting -----------------------------------------------------------------

def fmt_seconds(s):
    """Compact human duration for axis ticks: 1s, 30s, 5m, 2h, 3d."""
    if s < 1:
        return f"{s * 1000:.0f}ms" if s >= 0.001 else f"{s:.0e}s"
    for unit, size in (("d", 86400), ("h", 3600), ("m", 60)):
        if s >= size:
            v = s / size
            return f"{v:.0f}{unit}" if v >= 10 or v == int(v) else f"{v:.1f}{unit}"
    return f"{s:.0f}s"


def fmt_gb(gb):
    if gb is None:
        return "unbounded"
    return f"{gb / 1024:.3g} TiB" if gb >= 1024 else f"{gb:g} GiB"


def fmt_tokens(n):
    for unit, size in (("B", 1e9), ("M", 1e6), ("K", 1e3)):
        if abs(n) >= size:
            return f"{n / size:.1f}{unit}"
    return f"{n:.0f}"


def esc(s):
    return html.escape(str(s), quote=True)


# --- SVG charts -----------------------------------------------------------------
# Hand-rolled inline SVG so the report is one dependency-free file that themes
# with CSS variables (light/dark) and carries hover tooltips via data-tip.

W, H = 720, 320
PAD_L, PAD_R, PAD_T, PAD_B = 56, 120, 16, 44


def bar_chart(labels, values, tips, *, y_label, aria):
    """Single-series vertical bars (fraction 0..1 values shown as %)."""
    n = len(values)
    pw, ph = W - PAD_L - 24, H - PAD_T - PAD_B
    vmax = max(values) if values and max(values) > 0 else 1.0
    top = math.ceil(vmax * 100 / 5) * 5 / 100 or 0.05
    slot = pw / max(n, 1)
    bw = max(2.0, slot - 2)                        # 2px surface gap between bars
    out = [f'<svg viewBox="0 0 {W} {H}" role="img" aria-label="{esc(aria)}">']
    for k in range(5):                             # recessive gridlines + y ticks
        v = top * k / 4
        y = PAD_T + ph - v / top * ph
        out.append(f'<line class="grid" x1="{PAD_L}" x2="{PAD_L + pw}" y1="{y:.1f}" y2="{y:.1f}"/>')
        out.append(f'<text class="tick" x="{PAD_L - 8}" y="{y + 4:.1f}" text-anchor="end">'
                   f'{v * 100:.0f}%</text>')
    for i, (v, tip) in enumerate(zip(values, tips)):
        x = PAD_L + i * slot + 1
        h = v / top * ph
        y = PAD_T + ph - h
        r = min(4, bw / 2, h)                      # 4px rounded data-end, flat baseline
        if h > 0:
            out.append(
                f'<path class="bar s1" d="M{x:.1f},{PAD_T + ph:.1f} V{y + r:.1f} '
                f'Q{x:.1f},{y:.1f} {x + r:.1f},{y:.1f} H{x + bw - r:.1f} '
                f'Q{x + bw:.1f},{y:.1f} {x + bw:.1f},{y + r:.1f} V{PAD_T + ph:.1f} Z"/>')
        # Hit target: the full column, bigger than the mark.
        out.append(f'<rect class="hit" x="{x - 1:.1f}" y="{PAD_T}" width="{slot:.1f}" '
                   f'height="{ph}" data-tip="{esc(tip)}"/>')
    step = max(1, n // 8)
    for i in range(0, n, step):
        x = PAD_L + i * slot + slot / 2
        out.append(f'<text class="tick" x="{x:.1f}" y="{PAD_T + ph + 18}" '
                   f'text-anchor="middle">{esc(labels[i])}</text>')
    out.append(f'<line class="axis" x1="{PAD_L}" x2="{PAD_L + pw}" '
               f'y1="{PAD_T + ph}" y2="{PAD_T + ph}"/>')
    out.append(f'<text class="axis-title" x="{PAD_L + pw / 2}" y="{H - 6}" '
               f'text-anchor="middle">{esc(y_label)}</text>')
    out.append("</svg>")
    return "\n".join(out)


def line_chart(x_labels, series, *, x_title, aria, y_max=100):
    """Multi-series % line chart over categorical x; series = [(name, cls, vals, dashed)]."""
    n = len(x_labels)
    pw, ph = W - PAD_L - PAD_R, H - PAD_T - PAD_B
    xs = [PAD_L + (pw * i / (n - 1) if n > 1 else pw / 2) for i in range(n)]

    def y_of(v):
        return PAD_T + ph - v / y_max * ph

    out = [f'<svg viewBox="0 0 {W} {H}" role="img" aria-label="{esc(aria)}">']
    for k in range(5):
        v = y_max * k / 4
        y = y_of(v)
        out.append(f'<line class="grid" x1="{PAD_L}" x2="{PAD_L + pw}" y1="{y:.1f}" y2="{y:.1f}"/>')
        out.append(f'<text class="tick" x="{PAD_L - 8}" y="{y + 4:.1f}" text-anchor="end">'
                   f'{v:g}%</text>')
    for x, lab in zip(xs, x_labels):
        out.append(f'<text class="tick" x="{x:.1f}" y="{PAD_T + ph + 18}" '
                   f'text-anchor="middle">{esc(lab)}</text>')
    out.append(f'<line class="axis" x1="{PAD_L}" x2="{PAD_L + pw}" '
               f'y1="{PAD_T + ph}" y2="{PAD_T + ph}"/>')
    out.append(f'<text class="axis-title" x="{PAD_L + pw / 2}" y="{H - 6}" '
               f'text-anchor="middle">{esc(x_title)}</text>')

    # Direct end labels, nudged apart so close series don't collide.
    ends = sorted(((y_of(vals[-1]), name, cls) for name, cls, vals, _ in series),
                  key=lambda t: t[0])
    placed = []
    for y, name, cls in ends:
        if placed and y - placed[-1][0] < 14:
            y = placed[-1][0] + 14
        placed.append((y, name, cls))
    # Keep labels clear of the x-axis tick row: shift the stack up if needed.
    floor = PAD_T + ph - 6
    if placed and placed[-1][0] > floor:
        shift = placed[-1][0] - floor
        placed = [(y - shift, name, cls) for y, name, cls in placed]

    for name, cls, vals, dashed in series:
        pts = " ".join(f"{x:.1f},{y_of(v):.1f}" for x, v in zip(xs, vals))
        dash = ' stroke-dasharray="6 4"' if dashed else ""
        out.append(f'<polyline class="line {cls}" points="{pts}"{dash}/>')
        for x, v in zip(xs, vals):
            out.append(f'<circle class="dot {cls}" cx="{x:.1f}" cy="{y_of(v):.1f}" r="4"/>')
    for y, name, cls in placed:
        out.append(f'<text class="end-label" x="{PAD_L + pw + 10}" y="{y + 4:.1f}">'
                   f'<tspan class="swatch-text {cls}">&#9644;</tspan> {esc(name)}</text>')
    # Hover columns: one per x, tooltip lists every series at that x.
    col = pw / max(n - 1, 1)
    for i, (x, lab) in enumerate(zip(xs, x_labels)):
        tip = f"{x_title}: {lab}\n" + "\n".join(
            f"{name}: {vals[i]:.1f}%" for name, _, vals, _ in series)
        out.append(f'<rect class="hit" x="{x - col / 2:.1f}" y="{PAD_T}" width="{col:.1f}" '
                   f'height="{ph}" data-tip="{esc(tip)}" data-x="{x:.1f}"/>')
    out.append("</svg>")
    return "\n".join(out)


def legend(items):
    return '<div class="legend">' + "".join(
        f'<span><i class="sw {cls}{" dashed" if dashed else ""}"></i>{esc(name)}</span>'
        for name, cls, dashed in items) + "</div>"


# --- report ---------------------------------------------------------------------

CSS = """
:root {
  color-scheme: light;
  --page: #f9f9f7; --surface: #fcfcfb; --ink: #0b0b0b; --ink-2: #52514e;
  --muted: #898781; --grid: #e1e0d9; --axis: #c3c2b7;
  --border: rgba(11,11,11,0.10);
  --s1: #2a78d6; --s2: #eb6834; --s3: #1baf7a; --s4: #eda100;
}
@media (prefers-color-scheme: dark) {
  :root:where(:not([data-theme="light"])) {
    color-scheme: dark;
    --page: #0d0d0d; --surface: #1a1a19; --ink: #ffffff; --ink-2: #c3c2b7;
    --muted: #898781; --grid: #2c2c2a; --axis: #383835;
    --border: rgba(255,255,255,0.10);
    --s1: #3987e5; --s2: #d95926; --s3: #199e70; --s4: #c98500;
  }
}
:root[data-theme="dark"] {
  color-scheme: dark;
  --page: #0d0d0d; --surface: #1a1a19; --ink: #ffffff; --ink-2: #c3c2b7;
  --muted: #898781; --grid: #2c2c2a; --axis: #383835;
  --border: rgba(255,255,255,0.10);
  --s1: #3987e5; --s2: #d95926; --s3: #199e70; --s4: #c98500;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--page); color: var(--ink);
  font: 15px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
main { max-width: 820px; margin: 0 auto; padding: 32px 16px 64px; }
h1 { font-size: 26px; margin: 0 0 4px; }
h2 { font-size: 18px; margin: 40px 0 4px; }
p.sub, p.note { color: var(--ink-2); margin: 0 0 12px; }
p.note { font-size: 13px; }
.card { background: var(--surface); border: 1px solid var(--border);
  border-radius: 10px; padding: 16px; position: relative; }
.tiles { display: grid; grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
  gap: 10px; margin-top: 16px; }
.tile { background: var(--surface); border: 1px solid var(--border);
  border-radius: 10px; padding: 12px 14px; }
.tile .k { color: var(--ink-2); font-size: 12px; }
.tile .v { font-size: 20px; font-weight: 600; }
.tile .d { color: var(--muted); font-size: 12px; }
.hero { display: flex; gap: 24px; flex-wrap: wrap; align-items: baseline; }
.hero .big { font-size: 44px; font-weight: 650; line-height: 1.1; }
.hero .lbl { color: var(--ink-2); max-width: 520px; }
svg { width: 100%; height: auto; display: block; overflow: visible; }
.grid { stroke: var(--grid); stroke-width: 1; }
.axis { stroke: var(--axis); stroke-width: 1; }
.tick { fill: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; }
.axis-title { fill: var(--ink-2); font-size: 12px; }
.end-label { fill: var(--ink); font-size: 12px; }
.bar.s1 { fill: var(--s1); }
.line { fill: none; stroke-width: 2; stroke-linejoin: round; }
.dot { stroke: var(--surface); stroke-width: 2; }
.line.s1 { stroke: var(--s1); } .dot.s1, .swatch-text.s1 { fill: var(--s1); }
.line.s2 { stroke: var(--s2); } .dot.s2, .swatch-text.s2 { fill: var(--s2); }
.line.s3 { stroke: var(--s3); } .dot.s3, .swatch-text.s3 { fill: var(--s3); }
.line.s4 { stroke: var(--s4); } .dot.s4, .swatch-text.s4 { fill: var(--s4); }
.hit { fill: transparent; cursor: crosshair; }
.hit:hover { fill: var(--grid); fill-opacity: 0.35; }
.legend { display: flex; gap: 16px; flex-wrap: wrap; color: var(--ink-2);
  font-size: 13px; margin-bottom: 8px; }
.legend .sw { display: inline-block; width: 18px; height: 0; border-top: 2px solid;
  vertical-align: middle; margin-right: 6px; }
.legend .sw.dashed { border-top-style: dashed; }
.legend .sw.s1 { border-color: var(--s1); } .legend .sw.s2 { border-color: var(--s2); }
.legend .sw.s3 { border-color: var(--s3); } .legend .sw.s4 { border-color: var(--s4); }
#tip { position: fixed; pointer-events: none; background: var(--surface); color: var(--ink);
  border: 1px solid var(--border); border-radius: 8px; padding: 8px 10px;
  font-size: 12px; white-space: pre; box-shadow: 0 4px 16px rgba(0,0,0,0.12);
  display: none; z-index: 10; font-variant-numeric: tabular-nums; }
.table-wrap { overflow-x: auto; }
table { border-collapse: collapse; width: 100%; font-size: 12px;
  font-variant-numeric: tabular-nums; }
th, td { text-align: right; padding: 6px 8px; border-bottom: 1px solid var(--grid);
  white-space: nowrap; }
th:first-child, td:first-child { text-align: left; }
th { color: var(--ink-2); font-weight: 600; }
code { font-size: 13px; }
"""

JS = """
const tip = document.getElementById('tip');
document.querySelectorAll('[data-tip]').forEach(el => {
  el.addEventListener('mousemove', e => {
    tip.textContent = el.dataset.tip;
    tip.style.display = 'block';
    const r = tip.getBoundingClientRect();
    let x = e.clientX + 14, y = e.clientY + 14;
    if (x + r.width > innerWidth - 8) x = e.clientX - r.width - 14;
    if (y + r.height > innerHeight - 8) y = e.clientY - r.height - 14;
    tip.style.left = x + 'px'; tip.style.top = y + 'px';
  });
  el.addEventListener('mouseleave', () => { tip.style.display = 'none'; });
});
"""


def tile(k, v, d=""):
    return (f'<div class="tile"><div class="k">{esc(k)}</div><div class="v">{esc(v)}</div>'
            + (f'<div class="d">{esc(d)}</div>' if d else "") + "</div>")


def interval_section(dist_doc):
    buckets = [b for b in dist_doc["buckets"]]
    # Trim empty leading/trailing buckets so the axis spans the real data.
    nz = [i for i, b in enumerate(buckets) if b.get("count", 0) > 0]
    buckets = buckets[nz[0]:nz[-1] + 1] if nz else buckets
    labels = [fmt_seconds(b["upper_s"]) for b in buckets]
    vals = [b["fraction"] for b in buckets]
    tips = [f"{fmt_seconds(b['lower_s'])} – {fmt_seconds(b['upper_s'])}\n"
            f"{b['fraction'] * 100:.1f}% of gaps ({b['count']:,})" for b in buckets]
    s = dist_doc.get("summary", {})
    chart = bar_chart(labels, vals, tips, y_label="think-time gap between turns "
                      "(bucket upper edge, log-spaced)",
                      aria="Histogram of inter-turn think-time gaps")
    return f"""
<h2>Think-time between turns</h2>
<p class="sub">{esc(dist_doc.get('source', ''))} — {dist_doc.get('num_intervals', 0):,} gaps.
Median {fmt_seconds(s.get('median_s', 0))}, p90 {fmt_seconds(s.get('p90_s', 0))},
p99 {fmt_seconds(s.get('p99_s', 0))}. Longer pauses let other sessions' traffic push a
session's KV down the tiers before it returns.</p>
<div class="card">{chart}</div>"""


def turn_section(turn_doc, num_turns, turn_path):
    if turn_doc is None:
        return f"""
<h2>Turns per conversation</h2>
<div class="card hero"><div class="big">{num_turns}</div>
<div class="lbl">turns in every conversation (fixed). Pass <code>--turn-dist</code> to
sample turn counts from a distribution instead.</div></div>"""
    buckets = turn_doc["buckets"]
    nz = [i for i, b in enumerate(buckets) if b.get("count", 0) > 0]
    buckets = buckets[nz[0]:nz[-1] + 1] if nz else buckets
    labels = [f"{b['upper']:.0f}" for b in buckets]
    vals = [b["fraction"] for b in buckets]
    tips = [f"{b['lower']:.0f} – {b['upper']:.0f} turns\n"
            f"{b['fraction'] * 100:.1f}% of conversations ({b['count']:,})" for b in buckets]
    s = turn_doc.get("summary", {})
    chart = bar_chart(labels, vals, tips, y_label="turns per conversation "
                      "(bucket upper edge, log-spaced)",
                      aria="Histogram of turns per conversation")
    return f"""
<h2>Turns per conversation</h2>
<p class="sub">Sampled from {esc(os.path.basename(turn_path))} — median {s.get('median')},
mean {float(s.get('mean', 0)):.0f}, max {s.get('max')}.</p>
<div class="card">{chart}</div>"""


def working_set_gb(result):
    """Concurrent private windows + the shared prefix, in GB."""
    gb_per_block = result["bytes_per_block"] / 2 ** 30
    return (result["args"]["concurrent_sessions"] * result["conv_blocks_est"]
            + result["prefix_blocks"]) * gb_per_block


def build_report(rows, base, last, dist_doc, turn_doc, args, axis):
    a = base["args"]
    gb_per_block = base["bytes_per_block"] / 2 ** 30
    window_gb = base["window_blocks"] * gb_per_block if base["window_blocks"] else None
    ssd_gb = next((t["gb"] for t in base["tiers"] if t["name"] == "SSD"), 0)
    dram_gb = next((t["gb"] for t in base["tiers"] if t["name"] == "DRAM"), 0)
    ws_lo, ws_hi = working_set_gb(base), working_set_gb(last)
    turns_swept = axis is AXES["turns"]
    dram_swept = axis is AXES["dram"]
    conc_swept = axis is AXES["concurrency"]

    x_labels = [axis["label"](r["x"]) for r in rows]
    hbm = [r["with"]["HBM"] for r in rows]
    dram = [r["with"]["DRAM"] for r in rows]
    ssd = [r["with"]["SSD"] for r in rows]
    has_base = all(r.get("without") for r in rows)

    tiles = "".join([
        tile("Interval source", "BurstGPT" if "urst" in dist_doc.get("source", "")
             else os.path.basename(a["distribution"]),
             f"median {fmt_seconds(dist_doc['summary']['median_s'])}"),
        tile("Concurrent sessions",
             f"{x_labels[0]}–{x_labels[-1]}" if conc_swept else a["concurrent_sessions"],
             ("swept, " if conc_swept else "") + f"{a['sessions']} conversations total"),
        (tile("Turns / conversation", f"{x_labels[0]}–{x_labels[-1]}", "swept, fixed per run")
         if turns_swept else
         tile("Turns / conversation",
              f"{a['num_turns']} fixed" if turn_doc is None else "sampled",
              "" if turn_doc is None else os.path.basename(a["turn_distribution"]))),
        tile("Tokens added / turn", f"{base['turn_tokens']:,}",
             f"{a['avg_prompt_tokens']} prompt + {a['avg_gen_tokens']} gen"),
        tile("KV per token", f"{base['bytes_per_token'] / 1024:.0f} KB",
             f"{a['num_layers']} layers, {a['num_kv_heads']} KV heads, "
             f"{a['bytes_per_element'] * 8}-bit"),
        tile("Context window", f"{a['max_model_len']:,}" if a["max_model_len"] else "unbounded",
             f"{window_gb:.1f} GB KV cap" if window_gb else ""),
        tile("Shared prefix", f"{a['prefix_tokens']:,} tok",
             f"{base['prefix_blocks'] * gb_per_block:.2f} GB, held once"),
        tile("HBM tier", fmt_gb(a["hbm_gb"]), "top tier"),
        tile("DRAM tier", f"{x_labels[0]}–{x_labels[-1]}" if dram_swept
             else fmt_gb(dram_gb), "swept" if dram_swept else "middle tier"),
        tile("SSD tier", fmt_gb(ssd_gb), "bottom tier"),
        tile("Working set",
             f"{ws_hi:.0f} GB" if abs(ws_hi - ws_lo) < 0.5 else f"{ws_lo:.0f}–{ws_hi:.0f} GB",
             f"up to {ws_hi / a['hbm_gb']:.1f}× HBM"),
        tile("Cross-session sharing",
             f"{a['shared_fraction'] * 100:g}%" if a["shared_fraction"] else "none",
             (f"of live context, {a['share_groups']} group(s)" if a["shared_fraction"]
              else "private contexts")),
        tile("Admission", a["admission"], f"seed {a['seed']}"),
    ])

    tier_series = [("HBM", "s1", hbm, False), ("DRAM", "s2", dram, False),
                   ("SSD", "s3", ssd, False)]
    tier_chart = line_chart(x_labels, tier_series, x_title=axis["x_title"],
                            aria="Percent of KV lookups served by HBM, DRAM and SSD "
                                 f"versus {axis['by']}")
    impact = ""
    if has_base:
        miss = [r["without"]["miss"] for r in rows]
        imp_series = [("SSD hits (with SSD)", "s3", ssd, False),
                      ("Recompute (no SSD)", "s4", miss, True)]
        ymax = max(max(ssd), max(miss))
        ymax = max(10, math.ceil(ymax / 10) * 10)
        imp_chart = line_chart(x_labels, imp_series, x_title=axis["x_title"],
                               y_max=ymax,
                               aria="SSD hit share with SSD versus recompute share "
                                    f"without SSD, by {axis['by']}")
        impact = f"""
<h2>What the SSD tier buys</h2>
<p class="sub">The same sweep with the SSD tier disabled (<code>SSD_GB=0</code>): blocks
pushed out of DRAM are lost, and the next turn that needs them recomputes them. The
dashed line is that recompute share; the solid line is the share SSD serves instead.</p>
<div class="card">{legend([(n, c, d) for n, c, _, d in imp_series])}{imp_chart}</div>
<p class="note">The two lines need not match exactly: without SSD a lost block also
breaks the hash chain behind it, so recompute can exceed what SSD served.</p>"""

    decode = ""
    if all(r.get("decode") for r in rows):
        d_series = [("HBM", "s1", [r["decode"]["HBM"] for r in rows], False),
                    ("DRAM", "s2", [r["decode"]["DRAM"] for r in rows], False),
                    ("SSD", "s3", [r["decode"]["SSD"] for r in rows], False),
                    ("Recompute", "s4", [r["decode"]["miss"] for r in rows], True)]
        d_chart = line_chart(x_labels, d_series, x_title=axis["x_title"],
                             aria=f"Percent of decode-step context reads served by "
                                  f"each tier versus {axis['by']}")
        step_ms = float(a.get("decode_step_s") or 0) * 1000
        decode = f"""
<h2>Decode-stage reads, by {esc(axis['by'])}</h2>
<p class="sub">Every generated token is a decode step that re-reads the session's whole
live context ({a['avg_gen_tokens']:,} steps per turn at {step_ms:g} ms/step). Shares
are of those context reads. A context larger than HBM spills its tail every step, so
the lower tiers are read again on each token.</p>
<div class="card">{legend([(n, c, d) for n, c, _, d in d_series])}{d_chart}</div>
<p class="note">Decode simulated {a.get('decode_batch', 1)} step(s) per event; the
prefill charts above are unaffected by decode batching. Real engines keep a running
sequence's KV resident in HBM, so decode reads from DRAM/SSD here indicate a context
that would need KV offload during attention (or preemption) to run at all.</p>"""

    trs = []
    for r in rows:
        w, wo = r["with"], r.get("without")
        cells = [axis["label"](r["x"]), f"{w['HBM']:.1f}%", f"{w['DRAM']:.1f}%",
                 f"{w['SSD']:.1f}%", f"{w['miss']:.1f}%",
                 fmt_tokens(r["with_stats"]["recompute_tokens"])]
        if wo:
            cells += [f"{wo['miss']:.1f}%", fmt_tokens(r["without_stats"]["recompute_tokens"])]
        trs.append("<tr>" + "".join(f"<td>{esc(c)}</td>" for c in cells) + "</tr>")
    head = [axis["by"].capitalize(), "HBM hit", "DRAM hit", "SSD hit", "Miss", "Prefill tok"]
    if has_base:
        head += ["Miss (no SSD)", "Prefill tok (no SSD)"]
    table = ("<table><thead><tr>" + "".join(f"<th>{esc(h)}</th>" for h in head)
             + "</tr></thead><tbody>" + "".join(trs) + "</tbody></table>")

    cmd = " ".join(f"{k}={v}" for k, v in sorted(args.env_used.items()))
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SSD Tiering Sweep</title><style>{CSS}</style></head>
<body><main>
<h1>SSD tiering: {esc(axis['by'])} sweep</h1>
<p class="sub">Simulated three-tier KV cache (HBM → DRAM → SSD, LRU, demote-on-evict)
under BurstGPT think-times, sweeping {esc(axis['noun'])} from {esc(x_labels[0])} to
{esc(x_labels[-1])}{" (every conversation runs exactly that many turns)" if turns_swept else ""}{" (conversations in flight at once, held constant per run)" if conc_swept else ""}.</p>

<h2>Workload</h2>
<div class="tiles">{tiles}</div>
{interval_section(dist_doc)}
{"" if turns_swept else turn_section(turn_doc, a['num_turns'], a['turn_distribution'])}

<h2>Where lookups are served, by {esc(axis['by'])}</h2>
<p class="sub">Share of in-window prior-context tokens (including the shared prefix)
found in each tier, over all measured turns. The three lines sum to 100% minus misses.</p>
<div class="card">{legend([(n, c, d) for n, c, _, d in tier_series])}{tier_chart}</div>
{impact}
{decode}

<h2>Data</h2>
<div class="card table-wrap">{table}</div>
<p class="note">Hit columns are % of looked-up prior-context tokens. Prefill tokens are
everything computed on measured return turns: missed prior context plus each turn's
new prompt and generated tokens (the constant floor).</p>
<p class="note">Generated by <code>{axis['script']}</code> via
<code>run-burstgpt-example.sh</code> with {esc(cmd)}. Tiers model capacity and placement
only (no transfer bandwidth or latency); a warmup of one concurrency wave is excluded
from the hit statistics.</p>
</main><div id="tip" role="tooltip"></div><script>{JS}</script></body></html>
"""


def main(axis_key="dram", doc=None):
    axis = AXES[axis_key]
    turns_swept = axis_key == "turns"
    dram_swept = axis_key == "dram"
    env = os.environ.get
    ap = argparse.ArgumentParser(
        description=doc or __doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument(axis["flag"], dest="values",
                    default=env(axis["values_env"], axis["default"]),
                    help=f"comma-separated {axis['noun']} values ({axis['unit']}) "
                         f"(env {axis['values_env']}; default: {axis['default']})")
    ap.add_argument("--hbm-gb", default=env("HBM_GB", "10"),
                    help="HBM tier GiB (env HBM_GB; default: 10)")
    if not dram_swept:
        ap.add_argument("--dram-gb", default=env("DRAM_GB", "32"),
                        help="DRAM tier GiB, held fixed (env DRAM_GB; default: 32)")
    if not turns_swept:
        ap.add_argument("--num-turns", default=env("NUM_TURNS", "100"),
                        help="fixed turns per conversation (env NUM_TURNS; default: 100)")
        ap.add_argument("--turn-dist", default=env("TURN_DIST", ""),
                        help="turn-count YAML to sample instead of fixed --num-turns "
                             "(env TURN_DIST; default: fixed turns)")
    sessions_default = axis.get("sessions_default", "100")
    ap.add_argument("--sessions", default=env("SESSIONS", sessions_default),
                    help="total conversations per run "
                         f"(env SESSIONS; default: {sessions_default})")
    ap.add_argument("--dist", default=env("DIST", DEFAULT_DIST),
                    help="interval YAML (env DIST; default: burstgpt-3-intervals.yaml)")
    ap.add_argument("--no-baseline", action="store_true",
                    help="skip the SSD-disabled comparison runs")
    ap.add_argument("-o", "--output", default=os.path.join(REPORT_DIR, axis["output"]),
                    help=f"HTML report path (default: reports/{axis['output']} next "
                         f"to this script)")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4,
                    help="parallel simulator runs (default: CPU count)")
    args = ap.parse_args()

    try:
        values = [float(x) for x in args.values.split(",") if x.strip()]
    except ValueError:
        ap.error(f"{axis['flag']} must be comma-separated numbers")
    if not values:
        ap.error(f"{axis['flag']} is empty")
    if not dram_swept and any(v < 1 or v != int(v) for v in values):
        ap.error(f"{axis['flag']} values must be whole numbers >= 1")
    if axis_key == "concurrency" and max(values) >= int(args.sessions):
        ap.error(f"--sessions ({args.sessions}) must exceed the largest concurrency "
                 f"({max(values):g}): the first concurrency wave is warmup and is not "
                 f"measured")
    turn_dist = ("" if turns_swept or not args.turn_dist
                 else os.path.abspath(args.turn_dist))

    common = {"HBM_GB": args.hbm_gb, "SESSIONS": args.sessions,
              "TURN_DIST": turn_dist, "DIST": os.path.abspath(args.dist)}
    if not dram_swept:
        common["DRAM_GB"] = args.dram_gb
    if not turns_swept:
        common["NUM_TURNS"] = args.num_turns
    args.env_used = {k: v for k, v in common.items() if k != "DIST"}
    args.env_used["DIST"] = os.path.basename(common["DIST"])
    for k in ("CONCURRENT", "SSD_GB", "PREFIX_TOKENS", "ADMISSION", "SEED",
              "SHARED_FRACTION", "SHARE_GROUPS", "DECODE", "DECODE_STEP_S",
              "DECODE_BATCH", "AVG_PROMPT_TOKENS", "AVG_GEN_TOKENS"):
        if k in os.environ and k != axis["env"]:
            args.env_used[k] = os.environ[k]

    jobs = []
    for v in values:
        point = {**common, axis["env"]: f"{v:g}"}
        jobs.append((v, "with", point))
        if not args.no_baseline:
            jobs.append((v, "without", {**point, "SSD_GB": "0"}))

    results = {}
    with tempfile.TemporaryDirectory(prefix=f"{axis_key}-sweep-") as tmp, \
            concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futs = {pool.submit(run_once, ov, os.path.join(tmp, f"{i}.json")): (v, kind)
                for i, (v, kind, ov) in enumerate(jobs)}
        for fut in concurrent.futures.as_completed(futs):
            v, kind = futs[fut]
            try:
                results[(v, kind)] = fut.result()
            except RuntimeError as e:
                print(f"error: {e}", file=sys.stderr)
                return 1
            print(f"  done {axis['env']}={v:g} {kind} SSD", file=sys.stderr)

    rows = []
    for v in values:
        r = {"x": v, "with": tier_shares(results[(v, "with")]),
             "with_stats": results[(v, "with")]["stats"],
             "decode": decode_shares(results[(v, "with")])}
        if not args.no_baseline:
            r["without"] = tier_shares(results[(v, "without")])
            r["without_stats"] = results[(v, "without")]["stats"]
        rows.append(r)

    base = results[(values[0], "with")]
    last = results[(values[-1], "with")]
    with open(common["DIST"]) as f:
        dist_doc = yaml.safe_load(f)
    turn_doc = None
    if turn_dist:
        with open(turn_dist) as f:
            turn_doc = yaml.safe_load(f)

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "w") as f:
        f.write(build_report(rows, base, last, dist_doc, turn_doc, args, axis))

    w0 = max(9, len(axis["env"]))
    print(f"{axis['env']:>{w0}s} {'HBM%':>6s} {'DRAM%':>6s} {'SSD%':>6s}"
          + ("" if args.no_baseline else f" {'noSSD miss%':>12s}"))
    for r in rows:
        w = r["with"]
        line = (f"{axis['label'](r['x']):>{w0}s} {w['HBM']:6.1f} {w['DRAM']:6.1f} "
                f"{w['SSD']:6.1f}")
        if "without" in r:
            line += f" {r['without']['miss']:12.1f}"
        print(line)
    print(f"wrote {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
