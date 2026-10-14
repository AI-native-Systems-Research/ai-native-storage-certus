#!/usr/bin/env python3
"""Render the certus vs cputier connector-bench comparison as one HTML page.

Reads <artifacts>/params.json, certus.csv and cputier.csv (written by
run_compare.sh), joins result rows by label and writes a self-contained HTML
report: parameter parity, throughput/IOPS side by side, and per-phase latency.
"""
from __future__ import annotations

import argparse
import csv
import html
import json
import os
import re
from collections import Counter

# Config columns both benches write to their CSV; each must match across arms.
PARITY_COLUMNS = ["bs", "num_blocks", "block_bytes", "working_set", "tp", "lookup_order",
                  "pipeline_depth", "direction", "hit_ratio", "requests_per_step",
                  "num_sessions"]
METRIC_COLUMNS = {"label", "total_blocks", "total_bytes", "wall_seconds", "throughput_gbs", "iops"}


def parse_size(s: str) -> int:
    m = re.fullmatch(r"(\d+)([KMGT]?)", str(s).strip().upper())
    if not m:
        return int(s)
    return int(m.group(1)) << {"": 0, "K": 10, "M": 20, "G": 30, "T": 40}[m.group(2)]


def load_csv(path: str) -> list[dict]:
    if not os.path.exists(path):
        return []
    with open(path, newline="") as f:
        return list(csv.DictReader(f))


def row_key(rows: list[dict]) -> list[tuple[str, int]]:
    """Join key: label text before its parenthesised details, plus occurrence.

    Details such as the measured avg_prefix of a prefix-miss scan legitimately
    differ between backends, so they are not part of the key.
    """
    seen: Counter = Counter()
    keys = []
    for r in rows:
        base = r["label"].split(" (")[0].strip()
        keys.append((base, seen[base]))
        seen[base] += 1
    return keys


def phases(row: dict) -> dict[str, dict[str, float]]:
    out: dict[str, dict[str, float]] = {}
    for k, v in row.items():
        m = re.fullmatch(r"(.+)_(p50|p99|mean)_us", k or "")
        if m and v not in (None, ""):
            out.setdefault(m.group(1), {})[m.group(2)] = float(v)
    return out


def f(v, digits=2, empty="—"):
    if v is None or v == "":
        return empty
    return f"{float(v):,.{digits}f}"


def esc(s) -> str:
    return html.escape(str(s))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--artifacts", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    params = json.load(open(os.path.join(a.artifacts, "params.json")))
    certus = load_csv(os.path.join(a.artifacts, "certus.csv"))
    cputier = load_csv(os.path.join(a.artifacts, "cputier.csv"))

    # ── parameter parity, checked from what each bench actually recorded ──
    parity = []
    if certus and cputier:
        for col in PARITY_COLUMNS:
            cvs = _distinct(r.get(col) for r in certus)
            tvs = _distinct(r.get(col) for r in cputier)
            if not cvs and not tvs:
                continue
            same = len(cvs) == 1 and len(tvs) == 1 and (
                cvs[0] == tvs[0] or _num_eq(cvs[0], tvs[0]))
            parity.append((col, " / ".join(cvs) or "—", " / ".join(tvs) or "—", same))
        dram = parse_size(params["shared"]["dram"])
        tvs = _distinct(r.get("cpu_bytes") for r in cputier)
        parity.append(("DRAM tier bytes (certus --memory-tier-size / cputier cpu_bytes)",
                       dram, " / ".join(tvs) or "—", tvs == [str(dram)]))
    mismatches = [p for p in parity if not p[3]]

    # ── join result rows ──
    ck, tk = row_key(certus), row_key(cputier)
    by_c = dict(zip(ck, certus))
    by_t = dict(zip(tk, cputier))
    order = list(ck) + [k for k in tk if k not in by_c]
    joined = [(k, by_c.get(k), by_t.get(k)) for k in order]

    max_gbs = max([float(r["throughput_gbs"]) for r in certus + cputier] + [1e-9])

    sh = params["shared"]
    git = params["git"]

    def bar(v, cls):
        if v is None:
            return '<div class="bar empty">no result</div>'
        w = max(0.5, 100 * float(v) / max_gbs)
        return (f'<div class="bar"><span class="fill {cls}" style="width:{w:.1f}%"></span>'
                f'<span class="val">{f(v)} GB/s</span></div>')

    def ratio(c, t):
        if not c or not t:
            return "—"
        cg, tg = float(c["throughput_gbs"]), float(t["throughput_gbs"])
        if cg == 0 or tg == 0:
            return "—"
        r = cg / tg
        return f"certus ×{r:.2f}" if r >= 1 else f"cputier ×{1 / r:.2f}"

    rows_html = []
    details_html = []
    for (base, idx), c, t in joined:
        name = (c or t)["label"]
        cg = c["throughput_gbs"] if c else None
        tg = t["throughput_gbs"] if t else None
        rows_html.append(
            f"<tr><td class='lbl'>{esc(name)}</td>"
            f"<td class='bars'>{bar(cg, 'c')}{bar(tg, 't')}</td>"
            f"<td class='num'>{f(c and c['iops'], 0)}</td><td class='num'>{f(t and t['iops'], 0)}</td>"
            f"<td class='num'>{esc(ratio(c, t))}</td></tr>")
        pc, pt = phases(c) if c else {}, phases(t) if t else {}
        names = list(pc) + [p for p in pt if p not in pc]
        prow = []
        for p in names:
            x, y = pc.get(p, {}), pt.get(p, {})
            prow.append(
                f"<tr><td>{esc(p)}</td>"
                f"<td class='num'>{f(x.get('p50'), 1)}</td><td class='num'>{f(x.get('p99'), 1)}</td>"
                f"<td class='num'>{f(x.get('mean'), 1)}</td>"
                f"<td class='num sep'>{f(y.get('p50'), 1)}</td><td class='num'>{f(y.get('p99'), 1)}</td>"
                f"<td class='num'>{f(y.get('mean'), 1)}</td></tr>")
        labels_differ = c and t and c["label"] != t["label"]
        details_html.append(
            f"<details><summary>{esc(name)}</summary>"
            + (f"<p class='note'>cputier label: {esc(t['label'])}</p>" if labels_differ else "")
            + "<div class='scroll'><table class='lat'><thead>"
            "<tr><th rowspan=2>phase</th><th colspan=3>certus (µs)</th><th colspan=3 class='sep'>cputier (µs)</th></tr>"
            "<tr><th>p50</th><th>p99</th><th>mean</th><th class='sep'>p50</th><th>p99</th><th>mean</th></tr>"
            "</thead><tbody>" + "".join(prow) + "</tbody></table></div>"
            f"<p class='note'>blocks: certus {f(c and c['total_blocks'], 0)}, cputier {f(t and t['total_blocks'], 0)} · "
            f"wall s: certus {f(c and c['wall_seconds'], 3)}, cputier {f(t and t['wall_seconds'], 3)}</p>"
            "</details>")

    parity_html = "".join(
        f"<tr class='{'' if ok else 'bad'}'><td>{esc(col)}</td><td class='num'>{esc(cv)}</td>"
        f"<td class='num'>{esc(tv)}</td><td>{'✓ same' if ok else '✗ differs'}</td></tr>"
        for col, cv, tv, ok in parity)

    status = []
    if not certus:
        status.append("certus arm produced no results (see certus-bench.log / server.log).")
    if not cputier:
        status.append("cputier arm produced no results (see cputier-bench.log).")
    if mismatches:
        status.append(f"{len(mismatches)} parameter(s) differ between arms — results are not comparable.")
    banner = "".join(f"<div class='warn'>{esc(s)}</div>" for s in status)

    shared_html = "".join(f"<tr><td>{esc(k)}</td><td class='num'>{esc(v)}</td></tr>" for k, v in sh.items())
    cp, tp = params["certus"], params["cputier"]

    page = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Connector Bench Comparison</title>
<style>
:root {{ --bg:#fbfaf8; --fg:#1d1c1a; --muted:#6b6862; --line:#e4e1db; --card:#ffffff;
  --c:#2f6fb5; --t:#c9792b; --bad:#b3261e; --badbg:#fdecea; --warnbg:#fff4e0; }}
@media (prefers-color-scheme: dark) {{ :root:not([data-theme="light"]) {{
  --bg:#171614; --fg:#ecebe8; --muted:#a19d96; --line:#34322e; --card:#201f1c;
  --c:#6aa5e6; --t:#e7a35f; --bad:#f28b82; --badbg:#3a1f1d; --warnbg:#3a2f1a; }} }}
:root[data-theme="dark"] {{ --bg:#171614; --fg:#ecebe8; --muted:#a19d96; --line:#34322e; --card:#201f1c;
  --c:#6aa5e6; --t:#e7a35f; --bad:#f28b82; --badbg:#3a1f1d; --warnbg:#3a2f1a; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; background:var(--bg); color:var(--fg);
  font:14px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif; }}
main {{ max-width:1100px; margin:0 auto; padding:24px 16px 64px; }}
h1 {{ font-size:22px; margin:0 0 4px; }} h2 {{ font-size:16px; margin:32px 0 8px; }}
.sub, .note {{ color:var(--muted); font-size:13px; }}
table {{ border-collapse:collapse; width:100%; }}
th, td {{ text-align:left; padding:6px 8px; border-bottom:1px solid var(--line); vertical-align:middle; }}
th {{ font-weight:600; color:var(--muted); font-size:12px; }}
.num {{ text-align:right; font-variant-numeric:tabular-nums; white-space:nowrap; }}
.sep {{ border-left:1px solid var(--line); }}
.scroll {{ overflow-x:auto; }}
.card {{ background:var(--card); border:1px solid var(--line); border-radius:8px; padding:12px; }}
.legend span {{ display:inline-flex; align-items:center; gap:6px; margin-right:16px; }}
.sw {{ width:12px; height:12px; border-radius:2px; display:inline-block; }}
.bars {{ width:45%; min-width:180px; }}
.bar {{ position:relative; height:18px; margin:2px 0; }}
.bar.empty {{ color:var(--muted); font-size:12px; }}
.fill {{ position:absolute; left:0; top:0; bottom:0; border-radius:2px; }}
.fill.c {{ background:var(--c); }} .fill.t {{ background:var(--t); }}
.val {{ position:relative; padding-left:6px; font-size:12px; font-variant-numeric:tabular-nums;
  text-shadow:0 0 3px var(--card); }}
.lbl {{ max-width:320px; }}
tr.bad td {{ background:var(--badbg); color:var(--bad); }}
.warn {{ background:var(--warnbg); border:1px solid var(--line); border-radius:6px; padding:8px 12px; margin:8px 0; }}
details {{ border:1px solid var(--line); border-radius:6px; margin:6px 0; background:var(--card); }}
summary {{ cursor:pointer; padding:8px 12px; font-weight:500; }}
details > .scroll, details > .note {{ padding:0 12px 8px; }}
.grid {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(300px,1fr)); gap:12px; }}
code {{ font-size:12px; word-break:break-all; }}
</style></head><body><main>
<h1>Connector bench: certus vs cputier</h1>
<div class="sub">{esc(params.get('host', ''))} · GPU {esc(sh['gpus'])} {esc(params.get('gpu_name', ''))} ·
branch {esc(git['branch'])} @ {esc(git['commit'])}{' (dirty)' if git['dirty'] else ''} ·
mode {esc(sh['mode'])} · lookup order {esc(sh['lookup_order'])}</div>
{banner}

<h2>Throughput</h2>
<div class="legend sub"><span><i class="sw" style="background:var(--c)"></i>certus (certus-server, SPDK NVMe)</span>
<span><i class="sw" style="background:var(--t)"></i>cputier (vLLM TieringOffloadingSpec, fs tier)</span></div>
<div class="card scroll"><table>
<thead><tr><th>result</th><th>GB/s</th><th class="num">certus IOPS</th><th class="num">cputier IOPS</th><th class="num">faster</th></tr></thead>
<tbody>{''.join(rows_html)}</tbody></table></div>
<p class="note">IOPS counts blocks (cputier: offload chunks). cputier store throughput excludes the per-pass fs write-through drain
(see its log for the sustained figure); certus stores write through to SSD inside the server.</p>

<h2>Per-phase latency</h2>
{''.join(details_html)}

<h2>Parameters</h2>
<div class="grid">
<div class="card"><b>Shared (passed identically to both)</b><table>{shared_html}</table></div>
<div class="card"><b>Recorded by each bench</b><div class="scroll"><table>
<thead><tr><th>column</th><th class="num">certus</th><th class="num">cputier</th><th></th></tr></thead>
<tbody>{parity_html or "<tr><td colspan=4>needs both CSVs</td></tr>"}</tbody></table></div></div>
</div>
<h2>Backend-specific settings</h2>
<div class="grid">
<div class="card"><b>certus</b><table>
<tr><td>devices</td><td>{esc(cp['devices'])}</td></tr>
<tr><td>channels</td><td>{esc(cp['channels'])}</td></tr>
<tr><td>eviction threshold</td><td>{esc(cp['eviction_threshold'])}</td></tr>
<tr><td>connector workers</td><td>{esc(cp['workers'])}</td></tr></table>
<p class="note">server: <code>{esc(cp['server_bin'])} {esc(cp['server_args'])}</code></p>
<p class="note">bench: <code>{esc(cp['bench_cmd'])}</code></p></div>
<div class="card"><b>cputier</b><table>
<tr><td>image</td><td>{esc(tp['image'])}</td></tr>
<tr><td>spec</td><td>{esc(tp['spec'])}</td></tr>
<tr><td>fs tier dir</td><td>{esc(tp['fs_tier'])}</td></tr>
<tr><td>vLLM</td><td>{esc(cputier[0].get('vllm_version', '—') if cputier else '—')}</td></tr></table>
<p class="note">bench: <code>{esc(tp['bench_cmd'])}</code></p></div>
</div>
<p class="note">The disk tiers are inherently different: certus drives raw NVMe through SPDK, cputier writes one file per block
under its fs tier directory. Put the fs tier on comparable NVMe for a fair cold-path comparison.</p>
</main></body></html>
"""
    with open(a.out, "w") as fh:
        fh.write(page)
    if mismatches:
        print("WARNING: parameters differ between arms: "
              + ", ".join(f"{c} ({x} vs {y})" for c, x, y, _ in mismatches))


def _distinct(values) -> list[str]:
    """Distinct non-empty values, in first-seen order."""
    out: list[str] = []
    for v in values:
        if v not in (None, "") and v not in out:
            out.append(v)
    return out


def _num_eq(x, y) -> bool:
    try:
        return float(x) == float(y)
    except (TypeError, ValueError):
        return False


if __name__ == "__main__":
    main()
