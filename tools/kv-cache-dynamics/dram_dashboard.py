#!/usr/bin/env python3
"""Local web dashboard for the DRAM-size sweep, backed by the real simulator.

Serves ``dram_dashboard.html`` and a ``/api/sweep`` endpoint that **invokes
``kv_cache_dynamics.py``** (the actual CLI, via ``--json``) once per DRAM size
with the SSD tier on, and once more with it off, then returns the per-tier hit
shares and recompute volumes as JSON. The page lets you change the workload and
re-run; every number comes from the simulator, not a reimplementation.

Usage
-----
    ./dram_dashboard.py                 # serve on http://127.0.0.1:8077
    ./dram_dashboard.py --port 9000
    ./dram_dashboard.py --no-browser    # don't auto-open a browser

Stdlib only; no network access beyond localhost. It runs the simulator as a
subprocess, so results match the CLI exactly (same Mersenne-Twister RNG).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import threading
import webbrowser
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
SIM = os.path.join(SCRIPT_DIR, "kv_cache_dynamics.py")
HTML = os.path.join(SCRIPT_DIR, "dram_dashboard.html")

DISTS = {
    "burstgpt": "burstgpt-3-intervals.yaml",
    "ccweka": "cc-trace-weka-062126-intervals.yaml",
}
ADMISSIONS = {"interval", "random", "round-robin"}
TIER_NAMES = ("HBM", "DRAM", "SSD")

# One shared pool: simulator runs are subprocesses, so they release the GIL.
POOL = ThreadPoolExecutor(max_workers=min(8, (os.cpu_count() or 4)))


def _num(qs, key, default, *, cast=float, lo=None, hi=None):
    try:
        v = cast(qs.get(key, [str(default)])[0])
    except (ValueError, TypeError):
        v = default
    if lo is not None:
        v = max(lo, v)
    if hi is not None:
        v = min(hi, v)
    return v


def build_base_args(qs):
    """Translate query params into kv_cache_dynamics.py flags (SSD/DRAM added later)."""
    dist = qs.get("dist", ["burstgpt"])[0]
    dist_file = DISTS.get(dist, DISTS["burstgpt"])
    admission = qs.get("admission", ["interval"])[0]
    if admission not in ADMISSIONS:
        admission = "interval"
    ssd_raw = qs.get("ssd", ["20000"])[0].strip().lower()
    ssd = "inf" if ssd_raw in ("inf", "infinity") else str(_num(qs, "ssd", 20000, lo=0))

    args = [
        os.path.join(SCRIPT_DIR, dist_file),
        "--hbm-gb", str(_num(qs, "hbm", 10.0, lo=0.001)),
        "--concurrent-sessions", str(_num(qs, "concurrent", 8, cast=int, lo=1)),
        "--sessions", str(_num(qs, "sessions", 100, cast=int, lo=1)),
        "--num-turns", str(_num(qs, "turns", 100, cast=int, lo=1)),
        "--max-model-len", str(_num(qs, "mml", 131072, cast=int, lo=0)),
        "--prefix-tokens", str(_num(qs, "prefix", 20000, cast=int, lo=0)),
        "--avg-prompt-tokens", str(_num(qs, "prompt", 364, cast=int, lo=0)),
        "--avg-gen-tokens", str(_num(qs, "gen", 1398, cast=int, lo=0)),
        "--shared-fraction", str(_num(qs, "shared", 0.0, lo=0.0, hi=1.0)),
        "--admission", admission,
        "--block-size", str(_num(qs, "block", 64, cast=int, lo=1)),
        "--seed", str(_num(qs, "seed", 1, cast=int)),
        "--num-layers", str(_num(qs, "layers", 48, cast=int, lo=1)),
        "--num-kv-heads", str(_num(qs, "kvheads", 8, cast=int, lo=1)),
        "--head-dim", str(_num(qs, "headdim", 128, cast=int, lo=1)),
        "--bytes-per-element", str(_num(qs, "bpe", 2, cast=int, lo=1, hi=2)),
    ]
    if qs.get("decode", ["0"])[0] == "1":
        args += ["--decode",
                 "--decode-step-s", str(_num(qs, "decodeStep", 0.03, lo=1e-6)),
                 "--decode-batch", str(_num(qs, "decodeBatch", 16, cast=int, lo=1))]
    return args, ssd


def run_sim(base_args, dram_gb, ssd_gb, with_ssd):
    """Run kv_cache_dynamics.py for one (DRAM, SSD?) point; return its JSON doc."""
    fd, path = tempfile.mkstemp(suffix=".json", prefix="dram-dash-")
    os.close(fd)
    try:
        args = [sys.executable, SIM] + base_args + [
            "--dram-gb", str(dram_gb),
            "--ssd-gb", (ssd_gb if with_ssd else "0"),
            "--json", path,
        ]
        proc = subprocess.run(args, stdout=subprocess.DEVNULL,
                              stderr=subprocess.PIPE, text=True)
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr.strip() or "simulator failed")
        with open(path) as f:
            return json.load(f)
    finally:
        try:
            os.remove(path)
        except OSError:
            pass


def shares(doc):
    """Per-tier % of looked-up tokens, plus miss %, from a simulator JSON doc."""
    st = doc["stats"]
    lookup = st["lookup_tokens"] or 1
    names = [t["name"] for t in doc["tiers"]]
    out = {n: 0.0 for n in TIER_NAMES}
    for name, hits in zip(names, st["tier_hit_tokens"]):
        out[name] = hits / lookup * 100.0
    out["miss"] = (st["lookup_tokens"] - sum(st["tier_hit_tokens"])) / lookup * 100.0
    return out


def decode_shares(doc):
    st = doc["stats"]
    reads = st.get("decode_lookup_tokens", 0)
    if not reads:
        return None
    names = [t["name"] for t in doc["tiers"]]
    out = {n: 0.0 for n in TIER_NAMES}
    for name, hits in zip(names, st.get("decode_tier_hit_tokens", [])):
        out[name] = hits / reads * 100.0
    out["miss"] = st.get("decode_recompute_tokens", 0) / reads * 100.0
    return out


def sweep(qs):
    base_args, ssd = build_base_args(qs)
    sizes = []
    for tok in qs.get("dram", ["8,16,32,64,128,256"])[0].split(","):
        tok = tok.strip()
        if not tok:
            continue
        try:
            v = float(tok)
        except ValueError:
            continue
        if v >= 0:
            sizes.append(v)
    if not sizes:
        raise ValueError("no valid DRAM sizes")

    # Fan out all with/without runs, then collect.
    futs = {}
    for gb in sizes:
        futs[(gb, True)] = POOL.submit(run_sim, base_args, gb, ssd, True)
        futs[(gb, False)] = POOL.submit(run_sim, base_args, gb, ssd, False)
    docs = {k: f.result() for k, f in futs.items()}

    rows = []
    for gb in sizes:
        w, wo = docs[(gb, True)], docs[(gb, False)]
        rows.append({
            "dram": gb,
            "with": shares(w),
            "without": {"miss": shares(wo)["miss"]},
            "withRecompute": w["stats"]["recompute_tokens"],
            "withoutRecompute": wo["stats"]["recompute_tokens"],
            "measuredConvs": w["stats"]["measured_convs"],
            "dropped": w["stats"]["dropped_blocks"],
            "decode": decode_shares(w),
        })

    first = docs[(sizes[0], True)]
    a = first["args"]
    meta = {
        "distKey": qs.get("dist", ["burstgpt"])[0],
        "bytesPerBlock": first["bytes_per_block"],
        "turnTokens": first["turn_tokens"],
        "promptTok": a["avg_prompt_tokens"],
        "genTok": a["avg_gen_tokens"],
        "windowBlocks": first["window_blocks"],
        "prefixBlocks": first["prefix_blocks"],
        "workingBlocksEst": first.get("working_blocks_est"),
        "oversub": first["oversubscription_vs_hbm"],
        "hbmGb": a["hbm_gb"],
        "ssdGb": (None if ssd == "inf" else float(ssd)),
        "maxModelLen": a["max_model_len"],
        "decode": bool(a.get("decode")),
    }
    return {"meta": meta, "rows": rows}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass  # quiet

    def _send(self, code, body, ctype):
        data = body if isinstance(body, bytes) else body.encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path in ("/", "/index.html"):
            try:
                with open(HTML, "rb") as f:
                    self._send(200, f.read(), "text/html; charset=utf-8")
            except OSError:
                self._send(500, "dram_dashboard.html not found next to the server",
                           "text/plain")
            return
        if parsed.path == "/api/sweep":
            try:
                result = sweep(parse_qs(parsed.query))
                self._send(200, json.dumps(result), "application/json")
            except Exception as e:  # surface simulator/arg errors to the page
                self._send(400, json.dumps({"error": str(e)}), "application/json")
            return
        self._send(404, "not found", "text/plain")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=8077)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--no-browser", action="store_true")
    args = ap.parse_args()

    if not os.path.exists(SIM):
        print(f"error: {SIM} not found", file=sys.stderr)
        return 1

    httpd = ThreadingHTTPServer((args.host, args.port), Handler)
    url = f"http://{args.host}:{args.port}/"
    print(f"DRAM sweep dashboard on {url}  (Ctrl-C to stop)")
    print(f"  driving {SIM}")
    if not args.no_browser:
        threading.Timer(0.5, lambda: webbrowser.open(url)).start()
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\nstopping")
        httpd.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
