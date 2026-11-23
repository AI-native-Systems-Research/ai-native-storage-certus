#!/usr/bin/env python3
"""Side-by-side summary of arm result directories (turns.jsonl, prom.log, p2p.log).

Usage: compare_runs.py <run dir> [<run dir> ...]"""
import gzip, json, os, re, statistics, sys, collections

def opn(path):
    """A run file as text, or its .gz copy (committed runs keep prom.log gzipped)."""
    return open(path) if os.path.exists(path) else gzip.open(path + ".gz", "rt")

def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None

def prom_totals(path):
    tot = collections.defaultdict(float)
    for l in opn(path):
        if l.startswith("[prom]"):
            for k, v in re.findall(r"(\S+)=([0-9.eE+-]+)", l):
                try:
                    tot[k] += float(v)
                except ValueError:
                    pass
    return tot

rows = []
for d in sys.argv[1:]:
    rs = [json.loads(l) for l in open(f"{d}/turns.jsonl")]
    ok = [r for r in rs if "error" not in r]
    later = [r for r in ok if r["turn"] > 0]
    t0 = min(r["sent"] for r in rs)
    wall = max(r["sent"] + r.get("latency_s", 0) for r in ok) - t0
    p = prom_totals(f"{d}/prom.log")
    p2p = collections.Counter(re.findall(r'"outcome":"([^"]*)"', open(f"{d}/p2p.log").read()))
    gen = sum(r["answer_tokens"] for r in ok)
    rows.append((d.rstrip("/").split("/")[-1], {
        "turns ok / errors": f"{len(ok)} / {len(rs) - len(ok)}",
        "wall clock (s)": round(wall),
        "output tok/s (all sessions)": round(gen / wall, 1),
        "TTFT turn 0 p50 (s)": round(pct([r["ttft_s"] for r in ok if r["turn"] == 0], .5), 2),
        "TTFT later p50 (s)": round(pct([r["ttft_s"] for r in later], .5), 2),
        "TTFT later p90 (s)": round(pct([r["ttft_s"] for r in later], .9), 2),
        "TTFT turns 1-4 p50 (s)": round(pct([r["ttft_s"] for r in later if r["turn"] <= 4], .5), 2),
        "TTFT turns 5-7 p50 (s)": round(pct([r["ttft_s"] for r in later if r["turn"] >= 5], .5), 2),
        "latency p50 (s)": round(pct([r["latency_s"] for r in ok], .5), 1),
        "router: pulled / no-best-match": f'{p2p.get("pulled", 0)} / {p2p.get("no-best-match", 0)}',
        "prefill external hits / queries (tok)": f'{p["prefill.external_prefix_cache_hits"]:.0f} / {p["prefill.external_prefix_cache_queries"]:.0f}',
        "prefill GPU prefix hits / queries (tok)": f'{p["prefill.prefix_cache_hits"]:.0f} / {p["prefill.prefix_cache_queries"]:.0f}',
        "decode preemptions": int(sum(v for k, v in p.items() if k.endswith(".num_preemptions") and k.startswith("decode"))),
        "prefill KV loaded into GPU (GB)": round(p["prefill.kv_offload_load_bytes"] / 1e9, 1) if "prefill.kv_offload_load_bytes" in p else None,
    }))
keys = list(rows[0][1])
w = max(len(k) for k in keys)
print(f'{"":{w}}  ' + "  ".join(f"{n:>22}" for n, _ in rows))
for k in keys:
    print(f"{k:{w}}  " + "  ".join(f"{str(r[k]):>22}" for _, r in rows))
