#!/usr/bin/env python3
"""Render the connector-bench comparison (certus / cputier / lmcache) as one HTML page.

Reads <artifacts>/params.json and one CSV per arm (certus.csv, cputier.csv,
lmcache.csv; written by run_compare.sh), joins result rows by label and writes
a self-contained HTML report: parameter parity, throughput/IOPS side by side,
and per-phase latency. An arm with no CSV is shown as having no results.
"""
from __future__ import annotations

import argparse
import csv
import html
import json
import os
import re
from collections import Counter

# Config columns every bench writes to its CSV; each must match across arms.
PARITY_COLUMNS = ["bs", "num_blocks", "block_bytes", "working_set", "tp", "lookup_order",
                  "pipeline_depth", "direction", "hit_ratio", "requests_per_step",
                  "num_sessions"]
# Per-block KV tensors: certus records it as pieces_per_block, the others as layers.
LAYERS_COLUMN = {"certus": "pieces_per_block", "cputier": "layers", "lmcache": "layers"}

# (key, display name, legend description, CSS colour variable)
ARMS = [
    ("certus", "certus", "certus-server, SPDK NVMe", "--c"),
    ("cputier", "cputier", "vLLM TieringOffloadingSpec, fs tier", "--t"),
    ("lmcache", "lmcache", "LMCache MP server, L1 + fs_native L2", "--l"),
]
# Column holding each arm's DRAM tier size in bytes.
DRAM_COLUMN = {"cputier": "cpu_bytes", "lmcache": "l1_bytes"}


def parse_size(s: str) -> int:
    m = re.fullmatch(r"(\d+(?:\.\d+)?)([KMGT]?)", str(s).strip().upper())
    if not m:
        return int(s)
    return int(float(m.group(1)) * (1 << {"": 0, "K": 10, "M": 20, "G": 30, "T": 40}[m.group(2)]))


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
    data = {k: load_csv(os.path.join(a.artifacts, f"{k}.csv")) for k, *_ in ARMS}
    present = [arm for arm in ARMS if data[arm[0]]]
    names = {k: n for k, n, _, _ in ARMS}

    # ── parameter parity, checked from what each bench actually recorded ──
    parity = []   # (column, {arm: value}, same)
    if len(present) >= 2:
        for col in PARITY_COLUMNS:
            vals = {k: _distinct(r.get(col) for r in data[k]) for k, *_ in present}
            if not any(vals.values()):
                continue
            flat = [v[0] for v in vals.values() if len(v) == 1]
            same = len(flat) == len(vals) and all(x == flat[0] or _num_eq(x, flat[0]) for x in flat)
            parity.append((col, {k: " / ".join(v) or "—" for k, v in vals.items()}, same))
        dram = parse_size(params["shared"]["dram"])
        vals = {}
        for k, *_ in present:
            if k == "certus":
                vals[k] = [str(dram)]
            else:
                vals[k] = _distinct(r.get(DRAM_COLUMN[k]) for r in data[k])
        same = all(len(v) == 1 and _num_eq(v[0], dram) for v in vals.values())
        parity.append(("DRAM tier bytes (certus --memory-tier-size / cputier cpu_bytes / lmcache L1)",
                       {k: " / ".join(v) or "—" for k, v in vals.items()}, same))
        vals = {k: _distinct(r.get(LAYERS_COLUMN[k]) or ("1" if k == "certus" else "")
                             for r in data[k]) for k, *_ in present}
        flat = [v[0] for v in vals.values() if len(v) == 1]
        same = len(flat) == len(vals) and all(_num_eq(x, flat[0]) for x in flat)
        parity.append(("KV tensors per block (certus pieces_per_block / cputier, lmcache layers)",
                       {k: " / ".join(v) or "—" for k, v in vals.items()}, same))
    mismatches = [p for p in parity if not p[2]]

    # ── join result rows across arms ──
    by: dict[str, dict] = {}
    order: list[tuple[str, int]] = []
    for k, *_ in ARMS:
        keys = row_key(data[k])
        by[k] = dict(zip(keys, data[k]))
        order += [key for key in keys if key not in order]

    all_rows = [r for k, *_ in ARMS for r in data[k]]
    max_gbs = max([float(r["throughput_gbs"]) for r in all_rows] + [1e-9])

    sh = params["shared"]
    git = params["git"]

    def bar(v, var):
        if v is None:
            return '<div class="bar empty">no result</div>'
        w = max(0.5, 100 * float(v) / max_gbs)
        return (f'<div class="bar"><span class="fill" style="width:{w:.1f}%;background:var({var})"></span>'
                f'<span class="val">{f(v)} GB/s</span></div>')

    def work_flags(rows: dict) -> str:
        """Why a row may not compare like for like: work an arm skipped."""
        out = []
        for k, r in rows.items():
            if not r:
                continue
            bits = []
            for col, what in (("store_drops", "dropped stores"), ("load_misses", "load misses")):
                n = int(float(r.get(col) or 0))
                if n:
                    bits.append(f"{n:,} {what}")
            if r["label"].startswith("Cold") and r.get("disk_read_bytes") not in (None, ""):
                rd = float(r["disk_read_bytes"])
                want = float(r["total_bytes"])
                if rd < 0.5 * want:
                    bits.append(f"read {rd / 1e9:.1f} of {want / 1e9:.1f} GB from disk")
            if bits:
                out.append(f"{names[k]}: " + ", ".join(bits))
        return "; ".join(out)

    def fastest(rows: dict) -> str:
        got = {k: float(r["throughput_gbs"]) for k, r in rows.items() if r and float(r["throughput_gbs"]) > 0}
        if len(got) < 2:
            return "—"
        ranked = sorted(got, key=got.get, reverse=True)
        return f"{names[ranked[0]]} ×{got[ranked[0]] / got[ranked[1]]:.2f}"

    rows_html, details_html = [], []
    for key in order:
        rows = {k: by[k].get(key) for k, *_ in ARMS}
        first = next(r for r in rows.values() if r)
        name = first["label"]
        bars = "".join(bar(rows[k]["throughput_gbs"] if rows[k] else None, var) for k, _, _, var in ARMS)
        iops = "".join(f"<td class='num'>{f(rows[k] and rows[k]['iops'], 0)}</td>" for k, *_ in ARMS)
        flags = work_flags(rows)
        sust = " · ".join(f"{names[k]} {f(r.get('sustained_gbs'))}" for k, r in rows.items()
                          if r and r.get("sustained_gbs"))
        notes = ""
        if sust:
            notes += f"<div class='note'>incl. disk drain: {esc(sust)} GB/s</div>"
        if flags:
            notes += f"<div class='flag'>⚠ {esc(flags)}</div>"
        rows_html.append(f"<tr><td class='lbl'>{esc(name)}{notes}</td><td class='bars'>{bars}</td>"
                         f"{iops}<td class='num'>{esc(fastest(rows))}</td></tr>")

        ph = {k: phases(r) if r else {} for k, r in rows.items()}
        pnames: list[str] = []
        for k, *_ in ARMS:
            pnames += [p for p in ph[k] if p not in pnames]
        prow = []
        for p in pnames:
            cells = ""
            for i, (k, *_) in enumerate(ARMS):
                x = ph[k].get(p, {})
                sep = " sep" if i else ""
                cells += (f"<td class='num{sep}'>{f(x.get('p50'), 1)}</td>"
                          f"<td class='num'>{f(x.get('p99'), 1)}</td><td class='num'>{f(x.get('mean'), 1)}</td>")
            prow.append(f"<tr><td>{esc(p)}</td>{cells}</tr>")
        other_labels = "".join(
            f"<p class='note'>{esc(names[k])} label: {esc(r['label'])}</p>"
            for k, r in rows.items() if r and r["label"] != name)
        head1 = "".join(f"<th colspan=3{' class=sep' if i else ''}>{esc(n)} (µs)</th>"
                        for i, (_, n, _, _) in enumerate(ARMS))
        head2 = "".join(f"<th{' class=sep' if i else ''}>p50</th><th>p99</th><th>mean</th>"
                        for i in range(len(ARMS)))
        totals = " · ".join(
            f"{esc(names[k])}: {f(r and r['total_blocks'], 0)} blocks, {f(r and r['wall_seconds'], 3)} s"
            for k, r in rows.items())
        details_html.append(
            f"<details><summary>{esc(name)}</summary>{other_labels}"
            f"<div class='scroll'><table class='lat'><thead><tr><th rowspan=2>phase</th>{head1}</tr>"
            f"<tr>{head2}</tr></thead><tbody>{''.join(prow)}</tbody></table></div>"
            f"<p class='note'>{totals}</p></details>")

    parity_head = "".join(f"<th class='num'>{esc(n)}</th>" for k, n, _, _ in present)
    parity_html = "".join(
        f"<tr class='{'' if ok else 'bad'}'><td>{esc(col)}</td>"
        + "".join(f"<td class='num'>{esc(vals.get(k, '—'))}</td>" for k, *_ in present)
        + f"<td>{'✓ same' if ok else '✗ differs'}</td></tr>"
        for col, vals, ok in parity)

    status = []
    logs = {"certus": "certus-bench.log / server.log", "cputier": "cputier-bench.log",
            "lmcache": "lmcache-bench.log"}
    for k, n, _, _ in ARMS:
        if data[k]:
            continue
        note = params.get(k, {}).get("note") if isinstance(params.get(k), dict) else None
        status.append(f"{n} arm: {note}." if note else f"{n} arm produced no results (see {logs[k]}).")
    if mismatches:
        status.append(f"{len(mismatches)} parameter(s) differ between arms — results are not comparable.")
    banner = "".join(f"<div class='warn'>{esc(s)}</div>" for s in status)

    shared_html = "".join(f"<tr><td>{esc(k)}</td><td class='num'>{esc(v)}</td></tr>" for k, v in sh.items())
    cp, tp = params["certus"], params["cputier"]
    lp = params.get("lmcache", {})
    legend = "".join(f'<span><i class="sw" style="background:var({var})"></i>{esc(n)} ({esc(d)})</span>'
                     for _, n, d, var in ARMS)
    iops_head = "".join(f'<th class="num">{esc(n)} IOPS</th>' for _, n, _, _ in ARMS)
    title_arms = " vs ".join(n for _, n, _, _ in ARMS)
    lmc_ver = data["lmcache"][0].get("lmcache_version", "—") if data["lmcache"] else "—"
    lmc_chunk = data["lmcache"][0].get("chunk_tokens", lp.get("chunk_size", "—")) if data["lmcache"] else lp.get("chunk_size", "—")

    page = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Connector Bench Comparison</title>
<style>
:root {{ --bg:#fbfaf8; --fg:#1d1c1a; --muted:#6b6862; --line:#e4e1db; --card:#ffffff;
  --c:#2f6fb5; --t:#c9792b; --l:#3f8f5a; --bad:#b3261e; --badbg:#fdecea; --warnbg:#fff4e0; }}
@media (prefers-color-scheme: dark) {{ :root:not([data-theme="light"]) {{
  --bg:#171614; --fg:#ecebe8; --muted:#a19d96; --line:#34322e; --card:#201f1c;
  --c:#6aa5e6; --t:#e7a35f; --l:#6cc38a; --bad:#f28b82; --badbg:#3a1f1d; --warnbg:#3a2f1a; }} }}
:root[data-theme="dark"] {{ --bg:#171614; --fg:#ecebe8; --muted:#a19d96; --line:#34322e; --card:#201f1c;
  --c:#6aa5e6; --t:#e7a35f; --l:#6cc38a; --bad:#f28b82; --badbg:#3a1f1d; --warnbg:#3a2f1a; }}
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
.bars {{ width:40%; min-width:180px; }}
.bar {{ position:relative; height:18px; margin:2px 0; }}
.bar.empty {{ color:var(--muted); font-size:12px; }}
.fill {{ position:absolute; left:0; top:0; bottom:0; border-radius:2px; }}
.val {{ position:relative; padding-left:6px; font-size:12px; font-variant-numeric:tabular-nums;
  text-shadow:0 0 3px var(--card); }}
.lbl {{ max-width:300px; }}
tr.bad td {{ background:var(--badbg); color:var(--bad); }}
.flag {{ color:var(--bad); font-size:12px; margin-top:2px; }}
.warn {{ background:var(--warnbg); border:1px solid var(--line); border-radius:6px; padding:8px 12px; margin:8px 0; }}
details {{ border:1px solid var(--line); border-radius:6px; margin:6px 0; background:var(--card); }}
summary {{ cursor:pointer; padding:8px 12px; font-weight:500; }}
details > .scroll, details > .note {{ padding:0 12px 8px; }}
.grid {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(300px,1fr)); gap:12px; }}
code {{ font-size:12px; word-break:break-all; }}
</style></head><body><main>
<h1>Connector bench: {esc(title_arms)}</h1>
<div class="sub">{esc(params.get('host', ''))} · GPU {esc(sh['gpus'])} {esc(params.get('gpu_name', ''))} ·
branch {esc(git['branch'])} @ {esc(git['commit'])}{' (dirty)' if git['dirty'] else ''} ·
mode {esc(sh['mode'])} · lookup order {esc(sh['lookup_order'])}</div>
{banner}

<h2>Throughput</h2>
<div class="legend sub">{legend}</div>
<div class="card scroll"><table>
<thead><tr><th>result</th><th>GB/s</th>{iops_head}<th class="num">fastest</th></tr></thead>
<tbody>{''.join(rows_html)}</tbody></table></div>
<p class="note">IOPS counts blocks (cputier: offload chunks; lmcache: LMCache chunks of {esc(lmc_chunk)} tokens, each
<code>block_bytes</code>). Every arm is timed the same way: store rows exclude the per-pass drain of the disk write-through
(certus DRAM→SSD, cputier CPU→fs, lmcache L1→L2) and give the sustained figure including it under the row label; cold rows
empty the memory tier before every pass, outside the timed window, so each block is read back from disk. ⚠ marks a row
where an arm skipped work — dropped stores, load misses, or a cold pass that read less than half its bytes from disk —
so its throughput is not like for like.</p>

<h2>Per-phase latency</h2>
{''.join(details_html)}

<h2>Parameters</h2>
<div class="grid">
<div class="card"><b>Shared (passed identically to every arm)</b><table>{shared_html}</table></div>
<div class="card"><b>Recorded by each bench</b><div class="scroll"><table>
<thead><tr><th>column</th>{parity_head}<th></th></tr></thead>
<tbody>{parity_html or "<tr><td colspan=5>needs at least two CSVs</td></tr>"}</tbody></table></div></div>
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
<tr><td>vLLM</td><td>{esc(data['cputier'][0].get('vllm_version', '—') if data['cputier'] else '—')}</td></tr></table>
<p class="note">bench: <code>{esc(tp['bench_cmd'])}</code></p></div>
<div class="card"><b>lmcache</b><table>
<tr><td>image</td><td>{esc(lp.get('image', '—'))}</td></tr>
<tr><td>LMCache</td><td>{esc(lmc_ver)}</td></tr>
<tr><td>L1 (GiB)</td><td>{esc(lp.get('l1_gb', '—'))}</td></tr>
<tr><td>L2 dir (fs_native, O_DIRECT)</td><td>{esc(lp.get('l2_dir', '—'))}</td></tr>
<tr><td>chunk size (tokens)</td><td>{esc(lmc_chunk)}</td></tr></table>
<p class="note">bench: <code>{esc(lp.get('bench_cmd', '—'))}</code></p></div>
</div>
<p class="note">The disk tiers are inherently different: certus drives raw NVMe through SPDK; cputier writes one file per block
under its fs tier directory; lmcache writes one file per chunk under its L2 directory. Put the fs tier and the L2 dir on
comparable NVMe for a fair cold-path comparison.</p>
</main></body></html>
"""
    with open(a.out, "w") as fh:
        fh.write(page)
    if mismatches:
        print("WARNING: parameters differ between arms: "
              + "; ".join(f"{c} ({', '.join(f'{k}={v}' for k, v in vals.items())})"
                          for c, vals, _ in mismatches))


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
