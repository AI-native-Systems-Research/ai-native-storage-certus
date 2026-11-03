#!/usr/bin/env python3
"""Render the connector-bench comparison (certus / cputier / lmcache / 3fs / cputier-3fs)
as one HTML page.

Reads <artifacts>/params.json and one CSV per arm (certus.csv, cputier.csv,
lmcache.csv, 3fs.csv, cputier-3fs.csv; written by run_compare.sh), joins result rows by label and writes
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
LAYERS_COLUMN = {"certus": "pieces_per_block", "cputier": "layers", "lmcache": "layers",
                 "3fs": "layers", "cputier-3fs": "layers"}
# Arms with no memory tier: excluded from the DRAM-tier parity check.
NO_DRAM_TIER = {"3fs"}

# (key, display name, legend description, CSS colour variable)
ARMS = [
    ("certus", "certus", "certus-server, SPDK NVMe", "--c"),
    ("cputier", "cputier", "vLLM TieringOffloadingSpec, fs tier", "--t"),
    ("lmcache", "lmcache", "LMCache MP server, L1 + fs_native L2", "--l"),
    ("3fs", "3fs", "vLLM HF3FSKVConnector, 3FS USRBIO, no memory tier", "--h"),
    ("cputier-3fs", "cputier-3fs", "vLLM TieringOffloadingSpec, CPU tier + fs tier on 3FS (FUSE)", "--h2"),
]
# Column holding each arm's DRAM tier size in bytes.
DRAM_COLUMN = {"cputier": "cpu_bytes", "lmcache": "l1_bytes", "cputier-3fs": "cpu_bytes"}


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
    ap.add_argument("--latex", metavar="PATH",
                    help="also write the throughput table as a LaTeX (booktabs) table to PATH")
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
            if k in NO_DRAM_TIER:
                continue
            if k == "certus":
                vals[k] = [str(dram)]
            else:
                vals[k] = _distinct(r.get(DRAM_COLUMN[k]) for r in data[k])
        same = all(len(v) == 1 and _num_eq(v[0], dram) for v in vals.values())
        dram_vals = {k: " / ".join(v) or "—" for k, v in vals.items()}
        dram_vals.update({k: "none (no memory tier)" for k, *_ in present if k in NO_DRAM_TIER})
        parity.append(("DRAM tier bytes (certus --memory-tier-size / cputier cpu_bytes / lmcache L1)",
                       dram_vals, same))
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
            "lmcache": "lmcache-bench.log", "3fs": "3fs-bench.log",
            "cputier-3fs": "cputier-3fs-bench.log"}
    for k, n, _, _ in ARMS:
        if data[k]:
            continue
        note = params.get(k, {}).get("note") if isinstance(params.get(k), dict) else None
        status.append(f"{n} arm: {note}." if note else f"{n} arm produced no results (see {logs[k]}).")
    h3_clients = _distinct(r.get("hf3fs_client") for r in data["3fs"])
    if "mock" in h3_clients:
        status.append("3fs arm ran on vLLM's file-backed MOCK client (no 3FS cluster): its rows test the "
                      "code path only and are NOT a 3FS measurement.")
    if mismatches:
        status.append(f"{len(mismatches)} parameter(s) differ between arms — results are not comparable.")
    banner = "".join(f"<div class='warn'>{esc(s)}</div>" for s in status)

    shared_html = "".join(f"<tr><td>{esc(k)}</td><td class='num'>{esc(v)}</td></tr>" for k, v in sh.items())
    cp, tp = params["certus"], params["cputier"]
    lp = params.get("lmcache", {})
    hp = params.get("3fs", {})
    c3 = params.get("cputier-3fs", {})
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
  --c:#2f6fb5; --t:#c9792b; --l:#3f8f5a; --h:#7d52b8; --h2:#c2549a; --bad:#b3261e; --badbg:#fdecea; --warnbg:#fff4e0; }}
@media (prefers-color-scheme: dark) {{ :root:not([data-theme="light"]) {{
  --bg:#171614; --fg:#ecebe8; --muted:#a19d96; --line:#34322e; --card:#201f1c;
  --c:#6aa5e6; --t:#e7a35f; --l:#6cc38a; --h:#b393e6; --h2:#e58cc2; --bad:#f28b82; --badbg:#3a1f1d; --warnbg:#3a2f1a; }} }}
:root[data-theme="dark"] {{ --bg:#171614; --fg:#ecebe8; --muted:#a19d96; --line:#34322e; --card:#201f1c;
  --c:#6aa5e6; --t:#e7a35f; --l:#6cc38a; --h:#b393e6; --h2:#e58cc2; --bad:#f28b82; --badbg:#3a1f1d; --warnbg:#3a2f1a; }}
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
<div class="card"><b>3fs</b><table>
<tr><td>image</td><td>{esc(hp.get('image', '—'))}</td></tr>
<tr><td>client</td><td>{esc(' / '.join(h3_clients) or hp.get('client', '—'))}</td></tr>
<tr><td>storage</td><td>{esc(hp.get('storage', '—'))}</td></tr>
<tr><td>data file</td><td>{esc(hp.get('file_size', '—'))}</td></tr>
<tr><td>I/O jobs</td><td>{esc(hp.get('numjobs', '—'))}</td></tr>
<tr><td>memory tier</td><td>none</td></tr></table>
<p class="note">bench: <code>{esc(hp.get('bench_cmd', '—'))}</code></p></div>
<div class="card"><b>cputier-3fs</b><table>
<tr><td>image</td><td>{esc(c3.get('image', '—'))}</td></tr>
<tr><td>spec</td><td>{esc(c3.get('spec', '—'))}</td></tr>
<tr><td>fs tier dir (3FS FUSE, O_DIRECT)</td><td>{esc(c3.get('fs_tier', '—'))}</td></tr>
<tr><td>memory tier</td><td>CPU tier, --dram</td></tr></table>
<p class="note">bench: <code>{esc(c3.get('bench_cmd', '—'))}</code></p></div>
</div>
<p class="note">The disk tiers are inherently different: certus drives raw NVMe through SPDK; cputier writes one file per block
under its fs tier directory; lmcache writes one file per chunk under its L2 directory; 3fs writes pages of one connector data
file on a 3FS cluster through the USRBIO client. Put the fs tier, the L2 dir and the 3FS storage targets on comparable NVMe
for a fair cold-path comparison.</p>
<p class="note">3fs has no memory tier: every store and load goes to 3FS, so its warm and cold rows are the same disk path,
and it has no mixed-eviction row. It is a disk-only baseline, not a tiered cache. cputier-3fs is the same cputier bench with
its fs tier moved onto the 3FS mount: 3FS behind a memory tier. Its difference from cputier is the disk (3FS vs local
RAID0); its difference from 3fs is the memory tier. It reaches 3FS through FUSE, not USRBIO.</p>
</main></body></html>
"""
    with open(a.out, "w") as fh:
        fh.write(page)
    if a.latex:
        with open(a.latex, "w") as fh:
            fh.write(latex_table(order, by, names, present, sh, work_flags))
        print(f"wrote {a.latex}")
    if mismatches:
        print("WARNING: parameters differ between arms: "
              + "; ".join(f"{c} ({', '.join(f'{k}={v}' for k, v in vals.items())})"
                          for c, vals, _ in mismatches))


def _tex(s: str) -> str:
    """Escape LaTeX specials in plain text."""
    out = str(s)
    for a, b in (("\\", r"\textbackslash{}"), ("&", r"\&"), ("%", r"\%"), ("$", r"\$"),
                 ("#", r"\#"), ("_", r"\_"), ("{", r"\{"), ("}", r"\}"),
                 ("~", r"\textasciitilde{}"), ("^", r"\textasciicircum{}"),
                 ("→", r"$\to$"), ("×", r"$\times$"), ("—", "---")):
        out = out.replace(a, b)
    return out


def latex_table(order, by, names, present, sh, work_flags) -> str:
    """Throughput (GB/s) per result row and arm, as a booktabs tabular.

    Rows are the report's result rows (label text before its parenthesised
    details); store rows with a sustained figure get an extra "incl. disk drain"
    row. The fastest arm in each row is bold. A row where an arm skipped work
    (the report's warning flag) carries a dagger on that arm's value and a
    footnote line under the table. A second table gives each row's transfer
    latency (latency_table). Needs \\usepackage{booktabs}.
    """
    arms = [k for k, *_ in present]
    lines, notes = [], []
    mark = 0

    def cells(vals: dict, exclude=()) -> list[str]:
        # Flagged arms (they skipped work) are not eligible for the bold.
        got = {k: v for k, v in vals.items() if v is not None and names[k] not in exclude}
        best = max(got.values()) if got else None
        return [("---" if vals.get(k) is None else
                 (rf"\textbf{{{vals[k]:.2f}}}" if best is not None and vals[k] == best and len(got) > 1
                  else f"{vals[k]:.2f}"))
                for k in arms]

    for key in order:
        rows = {k: by[k].get(key) for k in arms}
        if not any(rows.values()):
            continue
        label = key[0]
        vals = {k: (float(r["throughput_gbs"]) if r else None) for k, r in rows.items()}
        flags = work_flags(rows)
        flagged = {part.split(":", 1)[0] for part in flags.split("; ")} if flags else set()
        row = cells(vals, flagged)
        if flags:
            mark += 1
            sym = r"$^{\dagger" + ("" if mark == 1 else str(mark)) + "}$"
            row = [c + (sym if names[k] in flagged else "") for c, k in zip(row, arms)]
            notes.append(sym + " " + _tex(flags) + ".")
        lines.append(_tex(label) + " & " + " & ".join(row) + r" \\")
        sust = {k: (float(r["sustained_gbs"]) if r and r.get("sustained_gbs") else None)
                for k, r in rows.items()}
        if any(v is not None for v in sust.values()):
            lines.append(r"\quad incl.\ disk drain & " + " & ".join(cells(sust)) + r" \\")

    cols = "l" + "r" * len(arms)
    head = " & ".join(_tex(names[k]) for k in arms)
    caption = (f"KV-offload connector throughput (GB/s), mode {sh.get('mode')}, "
               f"{sh.get('min_duration')}\\,s per phase, {sh.get('dram')} memory tier, "
               f"block {int(sh.get('block_bytes', 0)) // 1024}\\,KiB, "
               f"{sh.get('layers', '?')} KV tensor(s) per block. Bold: fastest per row; "
               f"--- : the arm has no such phase. $^{{\\dagger}}$: the arm skipped work in that row "
               f"(not like for like; never bold).")
    out = [r"% generated by benchmarks/connector-sweep/make_report.py --latex; needs \usepackage{booktabs}",
           r"\begin{table}[t]", r"\centering", r"\small",
           rf"\begin{{tabular}}{{{cols}}}", r"\toprule",
           rf"Phase & {head} \\", r"\midrule", *lines, r"\bottomrule", r"\end{tabular}"]
    if notes:
        out += [r"\par\smallskip", r"\parbox{\linewidth}{\footnotesize " + r" \\ ".join(notes) + "}"]
    out += [rf"\caption{{{caption}}}", r"\label{tab:connector-sweep}", r"\end{table}", ""]
    out += latency_table(order, by, names, arms, sh)
    return "\n".join(out)


# Per row: the phase that is the KV transfer itself (submit -> completion seen
# by the connector), by the name each bench records it under. Mixed eviction
# records store and load waits separately.
LATENCY_PHASES = {
    "Store Lifecycle": [("store", ("get_finished",))],
    "Warm Load Lifecycle": [("load", ("get_finished",))],
    # cputier and lmcache read a cold block from disk while the lookup waits
    # (promotion / L2 prefetch), before the transfer; certus and 3fs read it
    # inside the transfer. Show both so the cold row is comparable.
    "Cold Load Lifecycle": [("transfer", ("get_finished",)),
                            ("disk fetch at lookup", ("lookup_wait",))],
    "Mixed Store+Load Under Eviction Pressure": [("store", ("store_finish",)),
                                                 ("load", ("load_finish",))],
    "Pipelined Store": [("store", ("pipeline_drain",))],
    "Pipelined Load": [("load", ("pipeline_drain",))],
}


def latency_table(order, by, names, arms, sh) -> list[str]:
    """p50 / p99 of each row's transfer wait (ms), per arm, as a booktabs table.

    The wait is the bench's get_finished phase (store_finish / load_finish in
    mixed eviction): from submitting a batch of --bs blocks until the connector
    reports it done, i.e. the GPU<->tier transfer plus the backend's queueing.
    Lookup and bookkeeping phases are left out; the HTML report lists them all.
    """
    rows = []
    for key in order:
        label = key[0]
        if label not in LATENCY_PHASES:
            continue
        recs = {k: by[k].get(key) for k in arms}
        if not any(recs.values()):
            continue
        for kind, cands in LATENCY_PHASES[label]:
            cells, any_val = [], False
            for k in arms:
                r = recs[k]
                v = None
                if r:
                    for c in cands:
                        p50, p99 = r.get(f"{c}_p50_us"), r.get(f"{c}_p99_us")
                        if p50 not in (None, "") and float(p50) > 0:
                            v = (float(p50) / 1e3, float(p99 or 0) / 1e3)
                            break
                any_val |= v is not None
                cells.append("---" if v is None else f"{_fmt_ms(v[0])} / {_fmt_ms(v[1])}")
            if any_val:
                name = _tex(label) + (f" ({kind})" if len(LATENCY_PHASES[label]) > 1 else "")
                rows.append(name + " & " + " & ".join(cells) + r" \\")
    if not rows:
        return []
    cols = "l" + "r" * len(arms)
    head = " & ".join(_tex(names[k]) for k in arms)
    caption = (f"Latency per batch of {sh.get('bs')} blocks, p50 / p99 in ms. Rows give the transfer wait: from "
               f"submitting the batch until the connector reports it done (get\\_finished). On a cold load, "
               f"cputier and lmcache read from disk during the lookup, before the transfer (disk fetch at "
               f"lookup row); certus and 3fs read inside the transfer. --- : the arm has no such phase.")
    return [r"\begin{table}[t]", r"\centering", r"\small",
            rf"\begin{{tabular}}{{{cols}}}", r"\toprule",
            rf"Phase & {head} \\", r"\midrule", *rows, r"\bottomrule", r"\end{tabular}",
            rf"\caption{{{caption}}}", r"\label{tab:connector-sweep-latency}", r"\end{table}", ""]


def _fmt_ms(v: float) -> str:
    return f"{v:.2f}" if v < 10 else (f"{v:.1f}" if v < 100 else f"{v:.0f}")


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
