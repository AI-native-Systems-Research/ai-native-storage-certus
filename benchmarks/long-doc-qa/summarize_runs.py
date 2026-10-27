#!/usr/bin/env python3
"""Summarize LongDocQA runs written by run_one.sh.

Each run directory is <label>-<n>/ under ROOT. Per run, prints the client's query and
warmup round times and TTFT, the vLLM-side KV transfer totals (summed load, store and
lookup time from its "KV Transfer metrics" log lines), the external prefix-cache hit
rate, and checks that the run is valid (regions per block logged, memory tier on the
expected NUMA node). Then prints medians per label and the change against --base.

    summarize_runs.py ROOT [--base base]
"""
from __future__ import annotations

import argparse
import re
import statistics
from collections import defaultdict
from pathlib import Path


def client(d: Path) -> dict:
    s = (d / "client.stdout.log").read_text(errors="replace")
    out = {}
    for key, pat in (("query_s", r"Query round time: ([0-9.]+)s"),
                     ("query_ttft_s", r"Query round mean TTFT: ([0-9.]+)s"),
                     ("warmup_s", r"Warmup round time: ([0-9.]+)s"),
                     ("query_ok", r"Query round successful prompt count: (\d+)")):
        m = re.search(pat, s)
        out[key] = float(m.group(1)) if m else None
    return out


def vllm(d: Path) -> dict:
    s = (d / "server.log").read_text(errors="replace") if (d / "server.log").exists() else ""
    tot: dict[str, float] = defaultdict(float)
    for line in re.findall(r"KV Transfer metrics: (.*)", s):
        for k, v in re.findall(r"vllm:(kv_offload_\w+)=([0-9.e+-]+)", line):
            tot[k] += float(v)
    m = re.search(r"KV (\d+) region\(s\) block_bytes=(\d+)", s)
    lay = re.search(r"KV cache layout \(vLLM 0\.30\+\): (\w+)", s)
    return {"load_s": tot.get("kv_offload_load_time"),
            "store_s": tot.get("kv_offload_store_time"),
            "lookup_s": tot.get("kv_offload_lookup_sync_delay_seconds_sum"),
            "regions": int(m.group(1)) if m else None,
            "layout": lay.group(1) if lay else None}


def hit_rate(d: Path) -> float | None:
    p = d / "metrics.txt"
    if not p.exists():
        return None
    t = p.read_text()
    vals = {}
    for name in ("vllm:external_prefix_cache_hits_total", "vllm:external_prefix_cache_queries_total"):
        m = re.search(rf"^{re.escape(name)}(?:{{[^}}]*}})? ([0-9.e+]+)$", t, re.M)
        vals[name] = float(m.group(1)) if m else None
    h, q = vals.values()
    return 100 * h / q if h is not None and q else None


def numa(d: Path) -> str | None:
    p = d / "certus.stderr.log"
    m = re.search(r"memory-tier: allocated from SPDK hugepages \(NUMA node (-?\d+)\)",
                  p.read_text(errors="replace")) if p.exists() else None
    return m.group(1) if m else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("root", type=Path)
    ap.add_argument("--base", default="base", help="label to compare against (default: base)")
    args = ap.parse_args()

    per: dict[str, list[dict]] = defaultdict(list)
    print(f"{'run':16} {'query s':>8} {'q ttft':>7} {'warm s':>7} {'load s':>7} {'store s':>8} "
          f"{'lookup s':>8} {'ext hit':>8} {'layout':>6} {'reg':>4} {'numa':>4}")
    for d in sorted(p for p in args.root.iterdir() if p.is_dir()):
        if not (d / "client.stdout.log").exists():
            print(f"{d.name:16} (no client output)")
            continue
        r = {**client(d), **vllm(d), "hit": hit_rate(d), "numa": numa(d)}
        f = lambda v, w, p=2: f"{v:{w}.{p}f}" if isinstance(v, float) else f"{'-':>{w}}"
        print(f"{d.name:16} {f(r['query_s'], 8)} {f(r['query_ttft_s'], 7, 3)} {f(r['warmup_s'], 7, 1)} "
              f"{f(r['load_s'], 7, 1)} {f(r['store_s'], 8, 1)} {f(r['lookup_s'], 8)} "
              f"{f(r['hit'], 7, 1)}% {r['layout'] or '-':>6} {r['regions'] or '-':>4} {r['numa'] or '-':>4}")
        per[d.name.rsplit("-", 1)[0]].append(r)

    def med(rs, k):
        xs = [r[k] for r in rs if isinstance(r[k], float)]
        return statistics.median(xs) if xs else None

    print()
    base = per.get(args.base)
    for label, rs in per.items():
        q, t, ld, lk = (med(rs, k) for k in ("query_s", "query_ttft_s", "load_s", "lookup_s"))
        rel = ""
        if base and label != args.base and q and med(base, "query_s"):
            rel = f"  query {100 * (q / med(base, 'query_s') - 1):+.1f}% vs {args.base}"
        print(f"{label:12} runs={len(rs)} query {q or 0:.2f}s ttft {t or 0:.3f}s "
              f"load {ld or 0:.1f}s lookup {lk or 0:.2f}s{rel}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
