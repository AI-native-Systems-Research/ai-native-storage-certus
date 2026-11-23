"""Collect the reworked report's numbers into report/data2.json: TTFT, cache use and
bandwidth for each configuration's final run, bootstrap CIs for the comparisons,
and the aggregated repeats."""
import json, random, re, statistics as st, collections, glob, os
import gzip
HERE = os.path.dirname(os.path.abspath(__file__))
# Run directories: ../results by default; RESULTS=<dir> for runs elsewhere.
B = os.environ.get("RESULTS", os.path.join(HERE, "..", "results"))
REPORT = os.path.join(HERE, "..", "report")
def opn(path, **kw):
    """Open a run file as text, or its .gz copy if only that exists (large logs are committed gzipped)."""
    return open(path, **kw) if os.path.exists(path) else gzip.open(path + ".gz", "rt", **kw)
CFG = {  # key: (run dir, short label)
    "pd_certus_fetch":  "fx2-1-arm1",
    "pd_certus_nofetch":"fx2-2-arm2",
    "agg_certus_nofetch":"c13-3a-arm3a",
    "agg_certus_fetch": "c13-3b-arm3b",
    "pd_vllm_cpu_fs":   "ov-4-arm4",
    "pd_vllm_cpu_p2p":  "ov-5-arm5",
    "pd_vllm_p2p_fs_x": "ov-5fs-arm5fs",
}
KV_PER_TOKEN = 2359296 // 16  # bytes: Qwen3-8B, 36 layers x 8 KV heads x 128 x K,V x bf16
def turns(d): return [json.loads(l) for l in open(f"{B}/{d}/turns.jsonl")]
def q(xs, p): xs = sorted(xs); return xs[min(len(xs) - 1, int(p * len(xs)))]
def counters(d):
    """Sum each per-source counter over the run's 10-s scrape rounds (deltas)."""
    tot = collections.defaultdict(float)
    for l in opn(f"{B}/{d}/prom.log"):
        if not l.startswith("[prom] round"): continue
        for k, v in re.findall(r"(\S+)=([0-9.eE+-]+)", l):
            m = re.match(r"^(prefill\d?|decode\d|certus-[a-z0-9]+-numa\d)\.(.+)$", k)
            if not m: continue
            src = "prefill" if m.group(1).startswith("prefill") else ("certus" if m.group(1).startswith("certus") else "decode")
            try: tot[(src, m.group(2))] += float(v)
            except ValueError: pass
    return tot
def stats(key, d):
    rs = turns(d); ok = [r for r in rs if "error" not in r]
    later = [r["ttft_s"] for r in ok if r["turn"] > 0]
    c = counters(d)
    pd = key.startswith("pd")
    role = "prefill" if pd else "decode"           # who loads history into the GPU
    hits, qs = c[(role, "external_prefix_cache_hits")], c[(role, "external_prefix_cache_queries")]
    prompt = sum(r["prompt_tokens"] for r in ok)
    lb, lt = c[(role, "kv_offload_load_bytes")], c[(role, "kv_offload_load_time")]
    p2p = collections.Counter(re.findall(r'"outcome":"([^"]*)"', open(f"{B}/{d}/p2p.log").read())) if os.path.exists(f"{B}/{d}/p2p.log") else {}
    out = dict(run=d, turns=len(rs), ok=len(ok), errors=len(rs) - len(ok),
        p50=round(q(later, .5), 3), p90=round(q(later, .9), 3), p95=round(q(later, .95), 2),
        mean=round(st.mean(later), 3), t0=round(q([r["ttft_s"] for r in ok if r["turn"] == 0], .5), 3),
        per_turn=[round(st.median([r["ttft_s"] for r in ok if r["turn"] == t]), 3) for t in range(8)],
        over10=sum(x > 10 for x in later), band25=sum(2 < x <= 5 for x in later),
        cached=round(hits / qs, 4) if qs else 0.0, recomputed=round(prompt * (1 - (hits / qs if qs else 0))),
        pulls=p2p.get("pulled", 0),
        tok_s=round(sum(r["answer_tokens"] for r in ok) / (max(r["sent"] + r["latency_s"] for r in ok) - min(r["sent"] for r in rs)), 1),
        gpu_load_gb=round(lb / 1e9, 1), gpu_load_s=round(lt, 1), gpu_load_gbs=round(lb / lt / 1e9, 1) if lt else 0.0,
        fetch_gb=round(c[("certus", "certus_fetch_bytes")] / 1e9, 1), fetches=int(c[("certus", "certus_fetch_requests")]),
        certus_ssd_read_gb=round(c[("certus", "certus_nvme_read_bytes")] / 1e9, 1),
        certus_ssd_write_gb=round(c[("certus", "certus_nvme_write_bytes")] / 1e9, 1),
        vllm_tier_read_gb=round((c[("prefill", "kv_offload_tiering_read_bytes")] + c[("decode", "kv_offload_tiering_read_bytes")]) / 1e9, 1),
        vllm_tier_read_s=round(c[("prefill", "kv_offload_tiering_read_time")] + c[("decode", "kv_offload_tiering_read_time")], 1),
        history_gb=round(hits * KV_PER_TOKEN / 1e9, 1))
    return out
def per_tier(d):
    """Per-tier vLLM counters from the pods' final /metrics (only the experiment kept them)."""
    res = collections.defaultdict(float)
    for f in glob.glob(f"{B}/{d}/final-metrics/*.prom"):
        role = "prefill" if "prefill" in f else "decode"
        for line in open(f):
            m = re.match(r'^vllm:kv_offload_tiering_(\w+?)(?:_total)?\{[^}]*tier="([^"]+)"[^}]*\} ([0-9.e+-]+)$', line)
            if m and not m.group(1).endswith(("bucket", "created")):
                res[f"{role}.{m.group(2)}.{m.group(1)}"] += float(m.group(3))
    return {k: v for k, v in res.items() if v}
A = {k: stats(k, d) for k, d in CFG.items()}
x = per_tier(CFG["pd_vllm_p2p_fs_x"])
A["pd_vllm_p2p_fs_x"]["p2p_read_gb"] = round(x["prefill.1:p2p.read_bytes"] / 1e9, 1)
A["pd_vllm_p2p_fs_x"]["prefill_fs_read_gb"] = round(x["prefill.2:fs.read_bytes"] / 1e9, 1)
A["pd_vllm_p2p_fs_x"]["decode_fs_read_gb"] = round(x["decode.2:fs.read_bytes"] / 1e9, 1)
A["pd_vllm_p2p_fs_x"]["decode_fs_read_gbs"] = round(x["decode.2:fs.read_bytes"] / x["decode.2:fs.read_time"] / 1e9, 1)
A["pd_vllm_p2p_fs_x"]["p2p_wait_ms"] = round(x["prefill.1:p2p.lookup_async_delay_seconds_sum"] / x["prefill.1:p2p.lookup_async_delay_seconds_count"] * 1000)
A["pd_vllm_p2p_fs_x"]["decode_p2p_served_gb"] = round(x["decode.1:p2p.write_bytes"] / 1e9, 1)
# Where each configuration's prompt KV came from, GB over the run: loaded from the
# GPU's own store, fetched from a peer, or recomputed (as KV bytes).
PEER = {"pd_certus_fetch": A["pd_certus_fetch"]["fetch_gb"], "agg_certus_fetch": A["agg_certus_fetch"]["fetch_gb"],
        "pd_vllm_p2p_fs_x": A["pd_vllm_p2p_fs_x"]["p2p_read_gb"]}
for k, v in A.items():
    v["peer_gb"] = round(PEER.get(k, 0.0), 1)
    v["local_gb"] = round(max(0.0, v["gpu_load_gb"] - v["peer_gb"]), 1)
    v["recompute_gb"] = round(v["recomputed"] * KV_PER_TOKEN / 1e9, 1)
    v["dist"] = [sum(lo < r["ttft_s"] <= hi for r in turns(v["run"]) if "error" not in r and r["turn"] > 0)
                 for lo, hi in ((0, .5), (.5, 2), (2, 5), (5, 10), (10, 1e9))]
# vLLM's filesystem tier read rate into its CPU tier (prefill side), from its counters.
def fs_rate(d):
    c = counters(d); b, t = c[("prefill", "kv_offload_tiering_read_bytes")], c[("prefill", "kv_offload_tiering_read_time")]
    return round(b / t / 1e9, 1) if t else None
A["pd_vllm_cpu_fs"]["disk_read_gbs"] = fs_rate(CFG["pd_vllm_cpu_fs"])
# The scrape sums every tier's read bytes, and the P2P tier records no read time, so
# the experiment's disk rate comes from its per-tier final counters instead.
A["pd_vllm_p2p_fs_x"]["disk_read_gbs"] = round(x["prefill.2:fs.read_bytes"] / x["prefill.2:fs.read_time"] / 1e9, 1)
random.seed(1)
def later(k): return [r["ttft_s"] for r in turns(CFG[k]) if "error" not in r and r["turn"] > 0]
def boot(a, b, f=lambda xs: q(xs, .5), n=4000):
    xa, xb = later(a), later(b)
    ds = sorted(f(random.choices(xa, k=len(xa))) - f(random.choices(xb, k=len(xb))) for _ in range(n))
    return dict(d=round(f(xa) - f(xb), 3), lo=round(ds[int(.025 * n)], 3), hi=round(ds[int(.975 * n)], 3))
tm5 = lambda xs: st.mean([v for v in xs if v <= 5])
C = {f"{a}|{b}": boot(a, b) for a, b in [
    ("pd_certus_fetch", "pd_certus_nofetch"), ("pd_certus_nofetch", "pd_vllm_cpu_fs"),
    ("pd_certus_fetch", "pd_vllm_cpu_p2p"), ("pd_certus_fetch", "pd_vllm_p2p_fs_x"),
    ("pd_vllm_p2p_fs_x", "pd_vllm_cpu_fs"), ("agg_certus_fetch", "pd_certus_fetch"),
    ("agg_certus_fetch", "agg_certus_nofetch"), ("pd_certus_fetch", "pd_vllm_cpu_fs")]}
C["agg_certus_fetch|agg_certus_nofetch|tm5"] = boot("agg_certus_fetch", "agg_certus_nofetch", tm5)
# Aggregated repeats: four runs each on the CUDA 13.2 build, alternating, each from a
# fresh Certus with every instance's four drives checked.
REP = {"nofetch": ["c13-3a-arm3a", "c13-3a-arm3a-r2", "c13-3a-arm3a-r3", "c13-3a-arm3a-r4"],
       "fetch": ["c13-3b-arm3b", "c13-3b-arm3b-r2", "c13-3b-arm3b-r3", "c13-3b-arm3b-r4"]}
reps = []
for v, ds in REP.items():
    for i, d in enumerate(ds):
        lt_ = [r["ttft_s"] for r in turns(d) if "error" not in r and r["turn"] > 0]
        reps.append(dict(v=v, run=i + 1, tm5=round(tm5(lt_), 3), over10=sum(x > 10 for x in lt_), band25=sum(2 < x <= 5 for x in lt_), med=round(q(lt_, .5), 3), mean=round(st.mean(lt_), 3)))
import math
from itertools import combinations
def rep_cmp(key):
    a = [r[key] for r in reps if r["v"] == "nofetch"]; b = [r[key] for r in reps if r["v"] == "fetch"]
    d = st.mean(b) - st.mean(a); se = math.sqrt(st.variance(a) / 4 + st.variance(b) / 4)
    vals = a + b; n = tot = 0
    for idx in combinations(range(8), 4):
        bb = [vals[i] for i in idx]; aa = [vals[i] for i in range(8) if i not in idx]; tot += 1
        if abs(st.mean(bb) - st.mean(aa)) >= abs(d) - 1e-12: n += 1
    return dict(nofetch=round(st.mean(a), 3), fetch=round(st.mean(b), 3), d=round(d, 3), lo=round(d - 3.18 * se, 3), hi=round(d + 3.18 * se, 3), p=round(n / tot, 3))
REPCMP = {k: rep_cmp(k) for k in ("tm5", "band25", "med", "mean", "over10")}
# The experiment rerun with P2P debug logging (ov-5fs-probe): each fetch's steps,
# paired across the prefill and decode logs by kv_request_id.
import datetime
PROBE = "ov-5fs-probe"
EVT = {"C.SEND": r"P2P LOOKUP client \S+: SEND LookupMsg kv_request_id=(\S+) keys=(\d+)",
       "P.RECV": r"P2P LOOKUP server \S+: RECV LookupMsg kv_request_id=(\S+) round=\S+ keys=(\d+)",
       "P.SEND": r"P2P LOOKUP server \S+: SEND LookupRespMsg kv_request_id=(\S+) keys=(\d+)",
       "C.FETCH": r"P2PSession \S+: request_blocks job_id=\d+ kv_request_id=(\S+) blocks=(\d+)",
       "P.FINAL": r"P2PSession \S+: finalize kv_request_id=(\S+) round=\S+ success=(\S+)"}
ev = collections.defaultdict(dict)
for f in sorted(f.removesuffix(".gz") for f in glob.glob(f"{B}/{PROBE}-mslogs/*.log*")):
    for line in opn(f, errors="replace"):
        if "P2P" not in line: continue
        m = re.search(r"(\d\d-\d\d \d\d:\d\d:\d\d\.\d{3})", line)
        for name, pat in EVT.items():
            mm = m and re.search(pat, line)
            if mm: ev[mm.group(1)].setdefault(name, (datetime.datetime.strptime("2026-" + m.group(1), "%Y-%m-%d %H:%M:%S.%f").timestamp(), mm.group(2)))
full = [e for e in ev.values() if all(k in e for k in EVT)]
ms = lambda a, b: round(st.median((e[b][0] - e[a][0]) * 1000 for e in full))
asked = sum(int(e["C.SEND"][1]) for e in full); fetched = sum(int(e["C.FETCH"][1]) for e in full)
xp = per_tier(PROBE)
PR = dict(n=len(full), keys_med=round(st.median(int(e["C.SEND"][1]) for e in full)),
          fetched_med=round(st.median(int(e["C.FETCH"][1]) for e in full)),
          resolve_ms=ms("P.RECV", "P.SEND"), rest_ms=ms("P.SEND", "P.FINAL"), total_ms=ms("C.SEND", "P.FINAL"),
          asked=asked, fetched=fetched, unused=asked - fetched,
          unused_gb=round((asked - fetched) * 2359296 / 1e9, 1),
          prefill_fs_gb=round(xp["prefill.2:fs.read_bytes"] / 1e9, 1),
          p2p_gb=round(xp["prefill.1:p2p.read_bytes"] / 1e9, 1),
          resolve_gbs=round(st.median(int(e["P.RECV"][1]) * 2359296 / (e["P.SEND"][0] - e["P.RECV"][0]) / 1e9 for e in full), 1),
          p50=stats("x", PROBE)["p50"],
          prefill_fs_write_gb=round(xp["prefill.2:fs.write_bytes"] / 1e9, 1),
          certus_fetch_keys=int(counters(CFG["pd_certus_fetch"])[("certus", "certus_fetch_keys_requested")]))
# Where each looked-up block was on the decode side when the lookup arrived.
PR["decode_in_cpu"] = PR["decode_on_disk"] = 0
for f in sorted(f.removesuffix(".gz") for f in glob.glob(f"{B}/{PROBE}-mslogs/*decode*.log*")):
    for line in opn(f, errors="replace"):
        m = re.search(r"RESOLVED kv_request_id=\S+ hits=(\d+) misses=\d+ pending=(\d+)", line)
        if m: PR["decode_in_cpu"] += int(m.group(1)); PR["decode_on_disk"] += int(m.group(2))
# Second experiment: the same with the fs tier ahead of P2P (ov-5fs-swap).
SWAP = "ov-5fs-swap"
xs = per_tier(SWAP)
sw_l = [r["ttft_s"] for r in turns(SWAP) if "error" not in r and r["turn"] > 0]
pr_l = [r["ttft_s"] for r in turns(PROBE) if "error" not in r and r["turn"] > 0]
ds = sorted(q(random.choices(sw_l, k=len(sw_l)), .5) - q(random.choices(pr_l, k=len(pr_l)), .5) for _ in range(4000))
PR["swap"] = dict(p50=round(q(sw_l, .5), 3), d=round(q(sw_l, .5) - q(pr_l, .5), 3), lo=round(ds[100], 3), hi=round(ds[3900], 3),
                  decode_fs_gb=round(xs["decode.2:fs.read_bytes"] if "decode.2:fs.read_bytes" in xs else xs["decode.1:fs.read_bytes"] if "decode.1:fs.read_bytes" in xs else xs["decode.0:fs.read_bytes"], 1) / 1e9)
PR["swap"]["decode_fs_gb"] = round(PR["swap"]["decode_fs_gb"], 1)
PR["decode_fs_gb"] = round(xp["decode.2:fs.read_bytes"] / 1e9, 1)
# Each decode GPU's memory tier against what it takes in between a conversation's turns.
gaps = []
for d in (CFG["pd_certus_fetch"], PROBE):
    by = collections.defaultdict(dict)
    for r in turns(d): by[r["conv"]][r["turn"]] = r
    gaps += [t[n + 1]["sent"] - t[n]["sent"] - t[n]["latency_s"] for t in by.values() for n in t if n + 1 in t]
PR["gap_s"] = round(st.median(gaps))
wall = lambda d: (lambda rs: max(r["sent"] + r["latency_s"] for r in rs) - min(r["sent"] for r in rs))(turns(d))
PR["tier_gb"] = round(8556380160 / 1e9, 2)
PR["certus_in_gb"] = round(counters(CFG["pd_certus_fetch"])[("decode", "kv_offload_store_bytes")] / 3 / 1e9 * PR["gap_s"] / wall(CFG["pd_certus_fetch"]), 1)
PR["vllm_own_gb"] = round(xp["decode.2:fs.write_bytes"] / 3 / 1e9 * PR["gap_s"] / wall(PROBE), 1)
PR["vllm_peer_gb"] = round(xp["decode.2:fs.read_bytes"] / 3 / 1e9 * PR["gap_s"] / wall(PROBE), 1)
json.dump({"probe": PR, "configs": A, "cmp": C, "repeats": reps, "repcmp": REPCMP, "kv_per_token": KV_PER_TOKEN}, open(f"{REPORT}/data2.json", "w"), indent=1)
for k, v in A.items(): print(k, {kk: v[kk] for kk in ("p50", "p90", "cached", "pulls", "gpu_load_gb", "gpu_load_gbs", "fetch_gb", "certus_ssd_read_gb", "vllm_tier_read_gb", "history_gb")})
print(json.dumps(C, indent=0))
