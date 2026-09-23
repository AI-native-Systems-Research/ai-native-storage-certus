#!/usr/bin/env python3
"""
Generate an HTML report from sweep benchmark results (JSON).

Usage:
    python report.py results/sweep_results.json --output results/report.html
"""

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path


def load_results(path):
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(data, list):
        data = [data]
    return data


def extract_sweep_rows(results):
    """Extract per-page-size rows from sweep results."""
    rows = []
    for r in results:
        model = r.get("model", "unknown")
        backend = r.get("backend", "unknown")
        trace = r.get("trace_file", "")
        drives = r.get("drives", 0)
        storage = r.get("storage", {})
        read_stats = storage.get("read", {})
        write_stats = storage.get("write", {})

        # Page size: prefer explicit tag, fall back to model name parsing
        page_mib = r.get("page_size_mib", 0)
        if not page_mib:
            bpt = 0
            if model.startswith("custom-") and model.endswith("B"):
                try:
                    bpt = int(model[len("custom-"):-1])
                except ValueError:
                    pass
            page_bytes = bpt * 512 if bpt else 0
            page_mib = page_bytes / (1024 * 1024) if page_bytes else 0

        read_time = read_stats.get("time_s", 0)
        write_time = write_stats.get("time_s", 0)

        rows.append({
            "model": model,
            "backend": backend,
            "trace": trace,
            "drives": drives,
            "page_mib": round(page_mib, 1) if isinstance(page_mib, float) else page_mib,
            "qps": round(r.get("requests_per_second", 0), 2),
            "io_time_s": round(r.get("io_time_s", 0), 3),
            "total_requests": r.get("total_requests", 0),
            "hit_rate": round(r.get("page_hit_rate", 0) * 100, 2),
            "read_count": read_stats.get("count", 0),
            "read_mb": round(read_stats.get("mb", 0), 1),
            "read_avg_ms": round(read_stats.get("avg_ms", 0), 3),
            "read_p50_ms": round(read_stats.get("p50_ms", 0), 3),
            "read_p95_ms": round(read_stats.get("p95_ms", 0), 3),
            "read_p99_ms": round(read_stats.get("p99_ms", 0), 3),
            "read_bw_mbs": round(read_stats.get("mb", 0) / read_time, 1) if read_time > 0 else 0,
            "write_count": write_stats.get("count", 0),
            "write_mb": round(write_stats.get("mb", 0), 1),
            "write_avg_ms": round(write_stats.get("avg_ms", 0), 3),
            "write_p50_ms": round(write_stats.get("p50_ms", 0), 3),
            "write_p95_ms": round(write_stats.get("p95_ms", 0), 3),
            "write_p99_ms": round(write_stats.get("p99_ms", 0), 3),
            "write_bw_mbs": round(write_stats.get("mb", 0) / write_time, 1) if write_time > 0 else 0,
            "req_wall_avg_ms": round(r.get("request_wall_latency", {}).get("avg_ms", 0), 3),
            "req_wall_p99_ms": round(r.get("request_wall_latency", {}).get("p99_ms", 0), 3),
        })
    return rows


def generate_html(rows, json_path):
    """Generate a self-contained HTML report."""
    timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    rows_json = json.dumps(rows)

    # Group by trace for summary
    traces = sorted(set(r["trace"] for r in rows))

    html = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Certus Mooncake Benchmark</title>
<style>
:root {{
  --surface: #ffffff;
  --surface-alt: #f8f9fa;
  --border: #dee2e6;
  --text-primary: #212529;
  --text-secondary: #6c757d;
  --text-muted: #adb5bd;
  --accent: #2563eb;
  --accent-light: #dbeafe;
  --good: #16a34a;
  --warn: #d97706;
  --chart-1: #2563eb;
  --chart-2: #7c3aed;
  --chart-3: #db2777;
  --chart-4: #ea580c;
  --chart-read: #2563eb;
  --chart-write: #7c3aed;
}}
@media (prefers-color-scheme: dark) {{
  :root:not([data-theme="light"]) {{
    --surface: #1a1a2e;
    --surface-alt: #16213e;
    --border: #334155;
    --text-primary: #e2e8f0;
    --text-secondary: #94a3b8;
    --text-muted: #64748b;
    --accent: #60a5fa;
    --accent-light: #1e3a5f;
    --good: #4ade80;
    --warn: #fbbf24;
    --chart-1: #60a5fa;
    --chart-2: #a78bfa;
    --chart-3: #f472b6;
    --chart-4: #fb923c;
    --chart-read: #60a5fa;
    --chart-write: #a78bfa;
  }}
}}
:root[data-theme="dark"] {{
  --surface: #1a1a2e;
  --surface-alt: #16213e;
  --border: #334155;
  --text-primary: #e2e8f0;
  --text-secondary: #94a3b8;
  --text-muted: #64748b;
  --accent: #60a5fa;
  --accent-light: #1e3a5f;
  --good: #4ade80;
  --warn: #fbbf24;
  --chart-1: #60a5fa;
  --chart-2: #a78bfa;
  --chart-3: #f472b6;
  --chart-4: #fb923c;
  --chart-read: #60a5fa;
  --chart-write: #a78bfa;
}}
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  background: var(--surface);
  color: var(--text-primary);
  line-height: 1.5;
  padding: 16px;
  max-width: 1200px;
  margin: 0 auto;
}}
h1 {{ font-size: 1.5rem; font-weight: 700; margin-bottom: 4px; }}
.subtitle {{ color: var(--text-secondary); font-size: 0.875rem; margin-bottom: 24px; }}
h2 {{ font-size: 1.125rem; font-weight: 600; margin: 32px 0 12px; }}
.stat-row {{
  display: flex; gap: 12px; flex-wrap: wrap; margin-bottom: 24px;
}}
.stat-tile {{
  background: var(--surface-alt);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 12px 16px;
  min-width: 140px;
  flex: 1;
}}
.stat-tile .label {{ font-size: 0.75rem; color: var(--text-secondary); text-transform: uppercase; letter-spacing: 0.05em; }}
.stat-tile .value {{ font-size: 1.5rem; font-weight: 700; }}
.stat-tile .unit {{ font-size: 0.75rem; color: var(--text-muted); }}
table {{
  width: 100%;
  border-collapse: collapse;
  font-size: 0.8125rem;
  margin-bottom: 24px;
}}
th, td {{
  padding: 6px 10px;
  text-align: right;
  border-bottom: 1px solid var(--border);
  white-space: nowrap;
}}
th {{ font-weight: 600; color: var(--text-secondary); font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.04em; }}
th:first-child, td:first-child {{ text-align: left; }}
tr:hover td {{ background: var(--accent-light); }}
.chart-container {{
  background: var(--surface-alt);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 16px;
  margin-bottom: 24px;
  position: relative;
}}
.chart-title {{
  font-size: 0.875rem;
  font-weight: 600;
  margin-bottom: 12px;
}}
svg {{ display: block; }}
svg text {{ fill: var(--text-secondary); font-size: 11px; font-family: inherit; }}
svg .axis-line {{ stroke: var(--border); stroke-width: 1; }}
svg .grid-line {{ stroke: var(--border); stroke-width: 0.5; stroke-dasharray: 3,3; }}
svg .bar-read {{ fill: var(--chart-read); rx: 2; }}
svg .bar-write {{ fill: var(--chart-write); rx: 2; }}
svg .line-read {{ fill: none; stroke: var(--chart-read); stroke-width: 2; }}
svg .line-write {{ fill: none; stroke: var(--chart-write); stroke-width: 2; }}
svg .dot {{ r: 4; }}
svg .dot-read {{ fill: var(--chart-read); }}
svg .dot-write {{ fill: var(--chart-write); }}
.legend {{
  display: flex; gap: 16px; margin-bottom: 8px; font-size: 0.8125rem;
}}
.legend-item {{ display: flex; align-items: center; gap: 4px; }}
.legend-swatch {{ width: 12px; height: 12px; border-radius: 2px; }}
.legend-swatch.read {{ background: var(--chart-read); }}
.legend-swatch.write {{ background: var(--chart-write); }}
.tooltip {{
  position: absolute;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 8px 10px;
  font-size: 0.75rem;
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.15s;
  box-shadow: 0 2px 8px rgba(0,0,0,0.12);
  z-index: 10;
}}
.tooltip.visible {{ opacity: 1; }}
footer {{
  margin-top: 48px;
  padding-top: 16px;
  border-top: 1px solid var(--border);
  color: var(--text-muted);
  font-size: 0.75rem;
}}
</style>
</head>
<body>

<h1>Certus KVCache Storage Benchmark</h1>
<div class="subtitle">Mooncake FAST25 Trace Replay &mdash; Page-Size Sweep &mdash; {timestamp}</div>

<div id="stats-row" class="stat-row"></div>

<h2>Throughput vs Page Size</h2>
<div class="chart-container" id="chart-bw">
  <div class="legend">
    <div class="legend-item"><div class="legend-swatch read"></div>Read</div>
    <div class="legend-item"><div class="legend-swatch write"></div>Write</div>
  </div>
  <div class="chart-title">Bandwidth (MB/s)</div>
  <svg id="svg-bw"></svg>
  <div class="tooltip" id="tip-bw"></div>
</div>

<h2>Latency vs Page Size</h2>
<div class="chart-container" id="chart-lat">
  <div class="legend">
    <div class="legend-item"><div class="legend-swatch read"></div>Read avg</div>
    <div class="legend-item"><div class="legend-swatch write"></div>Write avg</div>
  </div>
  <div class="chart-title">Average Latency (ms)</div>
  <svg id="svg-lat"></svg>
  <div class="tooltip" id="tip-lat"></div>
</div>

<h2>QPS vs Page Size</h2>
<div class="chart-container" id="chart-qps">
  <div class="chart-title">Requests per Second</div>
  <svg id="svg-qps"></svg>
  <div class="tooltip" id="tip-qps"></div>
</div>

<h2>Detailed Results</h2>
<div style="overflow-x:auto">
<table id="results-table">
<thead>
<tr>
  <th>Drives</th>
  <th>Page Size</th>
  <th>QPS</th>
  <th>Hit Rate</th>
  <th>Read Avg</th>
  <th>Read p99</th>
  <th>Read BW</th>
  <th>Write Avg</th>
  <th>Write p99</th>
  <th>Write BW</th>
  <th>Req p99</th>
</tr>
</thead>
<tbody id="results-body"></tbody>
</table>
</div>

<footer>
  Generated from <code>{Path(json_path).name}</code> &mdash;
  Certus KVCache Storage Benchmark (Mooncake FAST25 Trace Replay)
</footer>

<script>
const DATA = {rows_json};

// ── stat tiles ──
const statsRow = document.getElementById('stats-row');
if (DATA.length > 0) {{
  const maxBw = Math.max(...DATA.map(r => Math.max(r.read_bw_mbs, r.write_bw_mbs)));
  const avgQps = (DATA.reduce((s,r) => s + r.qps, 0) / DATA.length).toFixed(1);
  const minLat = Math.min(...DATA.map(r => Math.min(r.read_avg_ms, r.write_avg_ms)));
  const sizes = DATA.map(r => r.page_mib).filter(x => x > 0);
  const sizeRange = sizes.length ? sizes[0] + '–' + sizes[sizes.length-1] + ' MiB' : 'N/A';
  const tiles = [
    ['Page Sizes', sizeRange, ''],
    ['Runs', DATA.length, ''],
    ['Peak BW', maxBw.toFixed(0), 'MB/s'],
    ['Avg QPS', avgQps, 'req/s'],
    ['Min Latency', minLat.toFixed(3), 'ms'],
  ];
  tiles.forEach(([label, value, unit]) => {{
    const d = document.createElement('div');
    d.className = 'stat-tile';
    d.innerHTML = `<div class="label">${{label}}</div><div class="value">${{value}}<span class="unit"> ${{unit}}</span></div>`;
    statsRow.appendChild(d);
  }});
}}

// ── helpers ──
const driveGroups = [...new Set(DATA.map(r => r.drives))].sort((a,b) => a-b);
const hasDrives = driveGroups.length > 1 || (driveGroups.length === 1 && driveGroups[0] > 0);
// Colors per drive count (1-indexed-ish)
const DRIVE_COLORS_LIGHT = ['#2563eb', '#7c3aed', '#db2777', '#ea580c', '#059669', '#d97706'];
const DRIVE_COLORS_DARK  = ['#60a5fa', '#a78bfa', '#f472b6', '#fb923c', '#34d399', '#fbbf24'];
function driveColor(idx) {{
  const dark = window.matchMedia('(prefers-color-scheme:dark)').matches;
  const pal = dark ? DRIVE_COLORS_DARK : DRIVE_COLORS_LIGHT;
  return pal[idx % pal.length];
}}

// ── table ──
const tbody = document.getElementById('results-body');
DATA.forEach(r => {{
  const tr = document.createElement('tr');
  tr.innerHTML = `
    <td>${{r.drives || '—'}}</td>
    <td>${{r.page_mib}} MiB</td>
    <td>${{r.qps.toFixed(1)}}</td>
    <td>${{r.hit_rate.toFixed(1)}}%</td>
    <td>${{r.read_avg_ms.toFixed(3)}} ms</td>
    <td>${{r.read_p99_ms.toFixed(3)}} ms</td>
    <td>${{r.read_bw_mbs.toFixed(0)}} MB/s</td>
    <td>${{r.write_avg_ms.toFixed(3)}} ms</td>
    <td>${{r.write_p99_ms.toFixed(3)}} ms</td>
    <td>${{r.write_bw_mbs.toFixed(0)}} MB/s</td>
    <td>${{r.req_wall_p99_ms.toFixed(3)}} ms</td>
  `;
  tbody.appendChild(tr);
}});

// ── charts ──
function drawBarChart(svgId, tipId, getData, yLabel) {{
  const svg = document.getElementById(svgId);
  const tip = document.getElementById(tipId);
  const container = svg.parentElement;
  const W = Math.min(container.clientWidth - 32, 900);
  const H = 280;
  const M = {{ top: 10, right: 20, bottom: 40, left: 60 }};
  const cW = W - M.left - M.right;
  const cH = H - M.top - M.bottom;
  svg.setAttribute('width', W);
  svg.setAttribute('height', H);
  svg.setAttribute('viewBox', `0 0 ${{W}} ${{H}}`);

  const labels = DATA.map(r => r.page_mib + ' MiB');
  const vals = DATA.map(getData);
  const maxVal = Math.max(...vals.map(v => Math.max(v[0], v[1]))) * 1.15;

  const barGroupW = cW / labels.length;
  const barW = Math.max(4, barGroupW * 0.35);
  const gap = 2;

  let html = '';
  // grid
  for (let i = 0; i <= 4; i++) {{
    const y = M.top + cH - (i / 4) * cH;
    const val = (maxVal * i / 4).toFixed(maxVal > 100 ? 0 : 2);
    html += `<line class="grid-line" x1="${{M.left}}" x2="${{W - M.right}}" y1="${{y}}" y2="${{y}}"/>`;
    html += `<text x="${{M.left - 6}}" y="${{y + 4}}" text-anchor="end">${{val}}</text>`;
  }}
  // axis
  html += `<line class="axis-line" x1="${{M.left}}" x2="${{W - M.right}}" y1="${{M.top + cH}}" y2="${{M.top + cH}}"/>`;

  vals.forEach((v, i) => {{
    const cx = M.left + barGroupW * i + barGroupW / 2;
    const h0 = (v[0] / maxVal) * cH;
    const h1 = (v[1] / maxVal) * cH;
    const x0 = cx - barW - gap / 2;
    const x1 = cx + gap / 2;
    html += `<rect class="bar-read" x="${{x0}}" y="${{M.top + cH - h0}}" width="${{barW}}" height="${{h0}}"
      data-i="${{i}}" data-type="read"/>`;
    html += `<rect class="bar-write" x="${{x1}}" y="${{M.top + cH - h1}}" width="${{barW}}" height="${{h1}}"
      data-i="${{i}}" data-type="write"/>`;
    html += `<text x="${{cx}}" y="${{M.top + cH + 16}}" text-anchor="middle">${{labels[i]}}</text>`;
  }});

  svg.innerHTML = html;

  // tooltip
  svg.querySelectorAll('rect').forEach(rect => {{
    rect.addEventListener('mouseenter', e => {{
      const i = +e.target.dataset.i;
      const t = e.target.dataset.type;
      const v = t === 'read' ? vals[i][0] : vals[i][1];
      tip.innerHTML = `<b>${{labels[i]}}</b><br>${{t}}: ${{v.toFixed(2)}}`;
      tip.classList.add('visible');
    }});
    rect.addEventListener('mousemove', e => {{
      const r = container.getBoundingClientRect();
      tip.style.left = (e.clientX - r.left + 12) + 'px';
      tip.style.top = (e.clientY - r.top - 10) + 'px';
    }});
    rect.addEventListener('mouseleave', () => tip.classList.remove('visible'));
  }});
}}

function drawLineChart(svgId, tipId, getY, yLabel) {{
  const svg = document.getElementById(svgId);
  const tip = document.getElementById(tipId);
  const container = svg.parentElement;
  const W = Math.min(container.clientWidth - 32, 900);
  const H = 260;
  const M = {{ top: 10, right: 20, bottom: 40, left: 60 }};
  const cW = W - M.left - M.right;
  const cH = H - M.top - M.bottom;
  svg.setAttribute('width', W);
  svg.setAttribute('height', H);
  svg.setAttribute('viewBox', `0 0 ${{W}} ${{H}}`);

  const labels = DATA.map(r => r.page_mib + ' MiB');
  const vals = DATA.map(getY);
  const maxVal = Math.max(...vals) * 1.15;
  const step = cW / Math.max(1, labels.length - 1);

  let html = '';
  for (let i = 0; i <= 4; i++) {{
    const y = M.top + cH - (i / 4) * cH;
    const val = (maxVal * i / 4).toFixed(maxVal > 100 ? 0 : 1);
    html += `<line class="grid-line" x1="${{M.left}}" x2="${{W - M.right}}" y1="${{y}}" y2="${{y}}"/>`;
    html += `<text x="${{M.left - 6}}" y="${{y + 4}}" text-anchor="end">${{val}}</text>`;
  }}
  html += `<line class="axis-line" x1="${{M.left}}" x2="${{W - M.right}}" y1="${{M.top + cH}}" y2="${{M.top + cH}}"/>`;

  let path = '';
  vals.forEach((v, i) => {{
    const x = M.left + step * i;
    const y = M.top + cH - (v / maxVal) * cH;
    path += (i === 0 ? 'M' : 'L') + x + ',' + y;
    html += `<circle class="dot dot-read" cx="${{x}}" cy="${{y}}" data-i="${{i}}"/>`;
    html += `<text x="${{x}}" y="${{M.top + cH + 16}}" text-anchor="middle">${{labels[i]}}</text>`;
  }});
  html = `<path class="line-read" d="${{path}}"/>` + html;
  svg.innerHTML = html;

  svg.querySelectorAll('circle').forEach(c => {{
    c.addEventListener('mouseenter', e => {{
      const i = +e.target.dataset.i;
      tip.innerHTML = `<b>${{labels[i]}}</b><br>${{vals[i].toFixed(2)}} ${{yLabel}}`;
      tip.classList.add('visible');
    }});
    c.addEventListener('mousemove', e => {{
      const r = container.getBoundingClientRect();
      tip.style.left = (e.clientX - r.left + 12) + 'px';
      tip.style.top = (e.clientY - r.top - 10) + 'px';
    }});
    c.addEventListener('mouseleave', () => tip.classList.remove('visible'));
  }});
}}

// If multiple drive groups, draw multi-series line charts grouped by drives.
// Otherwise fall back to the bar charts.
if (hasDrives && driveGroups.length > 1) {{
  // Build legend for drive counts
  function driveLegend(containerId) {{
    const el = document.querySelector('#' + containerId + ' .legend');
    if (!el) return;
    el.innerHTML = driveGroups.map((d, i) =>
      `<div class="legend-item"><div class="legend-swatch" style="background:${{driveColor(i)}}"></div>${{d}} drive${{d>1?'s':''}}</div>`
    ).join('');
  }}
  driveLegend('chart-bw');
  driveLegend('chart-lat');
  driveLegend('chart-qps');

  function drawMultiLineChart(svgId, tipId, getY, yLabel) {{
    const svg = document.getElementById(svgId);
    const tip = document.getElementById(tipId);
    const container = svg.parentElement;
    const W = Math.min(container.clientWidth - 32, 900);
    const H = 280;
    const M = {{ top: 10, right: 20, bottom: 40, left: 70 }};
    const cW = W - M.left - M.right;
    const cH = H - M.top - M.bottom;
    svg.setAttribute('width', W);
    svg.setAttribute('height', H);
    svg.setAttribute('viewBox', `0 0 ${{W}} ${{H}}`);

    const pageSizes = [...new Set(DATA.map(r => r.page_mib))].sort((a,b) => a-b);
    const allVals = DATA.map(getY);
    const maxVal = Math.max(...allVals) * 1.15;
    const step = cW / Math.max(1, pageSizes.length - 1);

    let html = '';
    for (let i = 0; i <= 4; i++) {{
      const y = M.top + cH - (i/4)*cH;
      const val = (maxVal*i/4).toFixed(maxVal>100?0:2);
      html += `<line class="grid-line" x1="${{M.left}}" x2="${{W-M.right}}" y1="${{y}}" y2="${{y}}"/>`;
      html += `<text x="${{M.left-6}}" y="${{y+4}}" text-anchor="end">${{val}}</text>`;
    }}
    html += `<line class="axis-line" x1="${{M.left}}" x2="${{W-M.right}}" y1="${{M.top+cH}}" y2="${{M.top+cH}}"/>`;
    pageSizes.forEach((ps, i) => {{
      html += `<text x="${{M.left+step*i}}" y="${{M.top+cH+16}}" text-anchor="middle">${{ps}} MiB</text>`;
    }});

    driveGroups.forEach((d, di) => {{
      const series = DATA.filter(r => r.drives === d).sort((a,b) => a.page_mib - b.page_mib);
      const color = driveColor(di);
      let path = '';
      series.forEach((r, i) => {{
        const xi = pageSizes.indexOf(r.page_mib);
        const x = M.left + step * xi;
        const v = getY(r);
        const y = M.top + cH - (v/maxVal)*cH;
        path += (i===0?'M':'L') + x + ',' + y;
        html += `<circle cx="${{x}}" cy="${{y}}" r="4" fill="${{color}}" data-d="${{d}}" data-ps="${{r.page_mib}}" data-v="${{v.toFixed(2)}}"/>`;
      }});
      html = `<path d="${{path}}" fill="none" stroke="${{color}}" stroke-width="2"/>` + html;
    }});
    svg.innerHTML = html;

    svg.querySelectorAll('circle').forEach(c => {{
      c.addEventListener('mouseenter', e => {{
        tip.innerHTML = `<b>${{e.target.dataset.d}} drive(s), ${{e.target.dataset.ps}} MiB</b><br>${{e.target.dataset.v}} ${{yLabel}}`;
        tip.classList.add('visible');
      }});
      c.addEventListener('mousemove', e => {{
        const r = container.getBoundingClientRect();
        tip.style.left = (e.clientX-r.left+12)+'px';
        tip.style.top = (e.clientY-r.top-10)+'px';
      }});
      c.addEventListener('mouseleave', () => tip.classList.remove('visible'));
    }});
  }}

  drawMultiLineChart('svg-bw', 'tip-bw', r => r.write_bw_mbs, 'MB/s');
  document.querySelector('#chart-bw .chart-title').textContent = 'Write Bandwidth vs Page Size (MB/s)';
  drawMultiLineChart('svg-lat', 'tip-lat', r => r.write_avg_ms, 'ms');
  document.querySelector('#chart-lat .chart-title').textContent = 'Write Avg Latency vs Page Size (ms)';
  drawMultiLineChart('svg-qps', 'tip-qps', r => r.qps, 'req/s');
}} else {{
  drawBarChart('svg-bw', 'tip-bw', r => [r.read_bw_mbs, r.write_bw_mbs], 'MB/s');
  drawBarChart('svg-lat', 'tip-lat', r => [r.read_avg_ms, r.write_avg_ms], 'ms');
  drawLineChart('svg-qps', 'tip-qps', r => r.qps, 'req/s');
}}
</script>
</body>
</html>"""
    return html


def main():
    parser = argparse.ArgumentParser(description="Generate HTML report from sweep results")
    parser.add_argument("json_file", help="Path to sweep_results.json")
    parser.add_argument("--output", "-o", default=None, help="Output HTML path (default: alongside JSON)")
    args = parser.parse_args()

    results = load_results(args.json_file)
    rows = extract_sweep_rows(results)

    if not rows:
        print("No results found in", args.json_file, file=sys.stderr)
        sys.exit(1)

    html = generate_html(rows, args.json_file)

    out_path = args.output or str(Path(args.json_file).with_suffix(".html"))
    Path(out_path).write_text(html, encoding="utf-8")
    print(f"Report written to {out_path} ({len(rows)} data points)")


if __name__ == "__main__":
    main()
