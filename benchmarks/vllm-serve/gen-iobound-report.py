#!/usr/bin/env python3
"""gen-iobound-report.py — build an HTML comparison report (throughput + TTFT,
with plots) from a results directory produced by run-gqa-iobound-matrix.sh or
run-mha-iobound-matrix.sh. The model, window, TP, KV size per token, vLLM version
and workload come from each arm's own logs, so one script serves both.

Expects RESULTS_DIR to contain one subdir per arm, each with:
  guidellm.json          guidellm benchmark output (throughput, TTFT, counts)
  guidellm.metrics.txt   before/after /metrics snapshot (external cache hit)
  kvio.csv               poll-kv-metrics.sh timeline (KV load/store IO)
Missing files are tolerated (the arm shows N/A).

  ./gen-iobound-report.py /mnt/certus1/vllm-serve-matrix/gqa-iobound-<stamp>
Writes report.html (+ PNGs) into that directory.
"""
import base64
import csv
import io
import json
import os
import re
import sys

try:
    import matplotlib
except ImportError:
    sys.exit(f"{os.path.basename(__file__)}: needs matplotlib for {sys.executable}: "
             f"{sys.executable} -m pip install --user matplotlib numpy")
matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

GIB = 1 << 30
TIB = 1 << 40

# Arm order and display labels. Keys are the subdir names the orchestrator writes.
ARMS = [
    ("certus-nointervals", "Certus (no intervals)", "#4C78A8"),
    ("certus-intervals", "Certus (intervals)", "#72B7B2"),
    ("cputier-nointervals", "cputier (no intervals)", "#F58518"),
    ("cputier-intervals", "cputier (intervals)", "#E45756"),
]


def _stat(metric, field="successful"):
    """Pull a guidellm metric's stat block, or None."""
    if not isinstance(metric, dict):
        return None
    return metric.get(field)


def parse_guidellm(path):
    try:
        d = json.load(open(path))
    except Exception:
        return None
    b = d["benchmarks"][0]
    m = b["metrics"]

    def mean(name):
        s = _stat(m.get(name, {}))
        return s.get("mean") if s else None

    def pcts(name):
        s = _stat(m.get(name, {}))
        if not s:
            return {}
        p = s.get("percentiles", {}) or {}
        return {"p50": p.get("p50"), "p90": p.get("p90"), "p99": p.get("p99"),
                "mean": s.get("mean")}

    lat = _stat(m.get("request_latency", {})) or {}
    err = m.get("request_latency", {}).get("errored", {}) or {}
    return {
        "duration": b.get("duration"),
        "tokens_per_s": mean("tokens_per_second"),
        "output_tokens_per_s": mean("output_tokens_per_second"),
        "prompt_tokens_per_s": mean("prompt_tokens_per_second"),
        "requests_per_s": mean("requests_per_second"),
        "ttft_ms": pcts("time_to_first_token_ms"),
        "ok": lat.get("count", 0),
        "errored": err.get("count", 0),
    }


def parse_kvio(path):
    try:
        rows = list(csv.DictReader(open(path)))
    except Exception:
        return None
    # The poller can append a trailing all-zero row (a scrape after teardown) and
    # may stop early; use the first row and the LAST row that has real data
    # (load_time_s > 0) so the delta reflects what was actually captured.
    good = [r for r in rows if float(r.get("load_time_s", 0) or 0) > 0
            or float(r.get("store_time_s", 0) or 0) > 0]
    if not good:
        return None
    a, b = rows[0], good[-1]
    def d(k):
        return float(b[k]) - float(a[k])
    return {
        "dur": d("wall_s") or 1.0,
        "load_time_s": d("load_time_s"), "store_time_s": d("store_time_s"),
        "load_bytes": d("load_bytes"), "store_bytes": d("store_bytes"),
        "prefill_sum_s": d("prefill_sum_s"), "rows": rows,
    }


def parse_metrics(path):
    """Full-run deltas from the before/after /metrics snapshot: KV load/store
    (authoritative, unaffected by the live poller) + external cache hit %.
    Handles both counter naming schemes (split load/store, or transfer_type)."""
    try:
        txt = open(path).read()
    except Exception:
        return None
    phase, B, A = None, {}, {}
    for ln in txt.splitlines():
        if ln.startswith("### BEFORE"):
            phase = B; continue
        if ln.startswith("### AFTER"):
            phase = A; continue
        if phase is None or ln.startswith("#"):
            continue
        mo = re.match(r"(vllm:\S+?)(\{[^}]*\})?\s+([0-9.eE+-]+)", ln)
        if not mo:
            continue
        name = mo.group(1).replace("vllm:", ""); labels = mo.group(2) or ""
        try:
            v = float(mo.group(3))
        except ValueError:
            continue
        tt = re.search(r'transfer_type="([^"]+)"', labels)
        src = re.search(r'source="([^"]+)"', labels)
        tier = re.search(r'tier="([^"]+)"', labels)
        key = (name + ("|" + tt.group(1) if tt else "") + ("|" + src.group(1) if src else "")
               + ("|tier" if tier else ""))
        phase[key] = phase.get(key, 0.0) + v
    def d(k):
        return A.get(k, 0.0) - B.get(k, 0.0)
    def xfer(flat, labeled):
        # vLLM 0.30 reports each transfer twice: under the flat name and under the
        # deprecated transfer_type-labelled one (equal for cputier; the labelled one
        # stays 0 for Certus). Take the flat one; the labelled one only without it.
        return d(flat) if flat in A else d(labeled)
    load_t = xfer("kv_offload_load_time_total", "kv_offload_total_time_total|CPU_to_GPU")
    store_t = xfer("kv_offload_store_time_total", "kv_offload_total_time_total|GPU_to_CPU")
    load_b = xfer("kv_offload_load_bytes_total", "kv_offload_total_bytes_total|CPU_to_GPU")
    store_b = xfer("kv_offload_store_bytes_total", "kv_offload_total_bytes_total|GPU_to_CPU")
    # cputier copies a block on its SSD tier to its CPU tier as a separate job
    # before the load, timed only under kv_offload_tiering_read_*; Certus's load
    # job includes its SSD read. Load time incl. tier reads covers SSD to GPU on
    # both (0 for Certus).
    tier_read_t = d("kv_offload_tiering_read_time_total|tier")
    tier_read_b = d("kv_offload_tiering_read_bytes_total|tier")
    tier_write_t = d("kv_offload_tiering_write_time_total|tier")
    tier_write_b = d("kv_offload_tiering_write_bytes_total|tier")
    def mean(name):
        n = d(name + "_count")
        return d(name + "_sum") / n if n else None
    q = d("external_prefix_cache_queries_total")
    h = d("external_prefix_cache_hits_total")
    # Computed (recompute) share of prompt tokens, from vLLM's prompt_tokens_by_source
    # (computed + local_cache_hit + external_kv_transfer = prompt tokens), the same
    # counter the timeline plots. Snapshots without it: of the tokens queried against
    # the GPU prefix cache, those hit neither there nor in the offload tier (counts a
    # preempted request's lookup again on re-admission). Snapshots from before
    # run-guidellm.sh matched prompt_tokens_by_source keep only its external line.
    pc = d("prompt_tokens_by_source_total|local_compute")
    ptot = pc + d("prompt_tokens_by_source_total|local_cache_hit") \
        + d("prompt_tokens_by_source_total|external_kv_transfer")
    lq = d("prefix_cache_queries_total")
    lh = d("prefix_cache_hits_total")
    if ptot and "prompt_tokens_by_source_total|local_compute" in A:
        computed = 100 * pc / ptot
    else:
        computed = (100 * max(lq - lh - h, 0.0) / lq) if lq else None
    return {"load_time_s": load_t, "store_time_s": store_t,
            "load_bytes": load_b, "store_bytes": store_b,
            "ext_hit": (100 * h / q) if q else None, "computed_pct": computed,
            "tier_read_time_s": tier_read_t, "tier_read_bytes": tier_read_b,
            "tier_write_time_s": tier_write_t, "tier_write_bytes": tier_write_b,
            # vLLM's per-request timings, measured the same for every backend: a
            # request whose KV is loading stays queued until the load is done.
            "queue_s": mean("request_queue_time_seconds"),
            "prefill_s": mean("request_prefill_time_seconds"),
            "ttft_s": mean("time_to_first_token_seconds")}


def timeline_png(rows, title, computed_pct=None, tier_read_s=0.0):
    """Prompt tokens/s by source (GPU-computed vs GPU prefix-cache hit vs offload-
    tier hit) above KV load/store IO device-seconds per wall-second, from the
    poll-kv-metrics.sh rows. The token split comes from vLLM's
    prompt_tokens_by_source counter (computed + local_cache_hit +
    external_kv_transfer = prompt tokens), so it measures real recompute; summed
    request prefill time does not (it counts every queued stream's wait). CSVs from
    an older poller lack the split: plot total prompt tok/s and annotate the
    full-run computed_pct instead. Robust to a trailing all-zero teardown row and
    to early truncation: skips intervals where a cumulative counter decreased or
    dt<=0. Returns base64 PNG, or None."""
    has_src = bool(rows) and "computed_tok" in rows[0]
    # cputier's SSD->CPU reads (separate jobs before its CPU->GPU load); CSVs from
    # an older poller lack the column.
    has_tier = bool(rows) and "tier_read_s" in rows[0]
    t, comp, lhit, ehit, ptok, load, store, tier = [], [], [], [], [], [], [], []
    for a, b in zip(rows, rows[1:]):
        try:
            dt = float(b["wall_s"]) - float(a["wall_s"])
            dl = float(b["load_time_s"]) - float(a["load_time_s"])
            ds = float(b["store_time_s"]) - float(a["store_time_s"])
            dpt = float(b["prompt_tok"]) - float(a["prompt_tok"])
            dtr = (float(b["tier_read_s"]) - float(a["tier_read_s"])) if has_tier else 0.0
            if has_src:
                dc = float(b["computed_tok"]) - float(a["computed_tok"])
                dlh = float(b["local_hit_tok"]) - float(a["local_hit_tok"])
                deh = float(b["ext_hit_tok"]) - float(a["ext_hit_tok"])
            else:
                dc = dlh = deh = 0.0
        except (KeyError, ValueError):
            continue
        if dt <= 0 or min(dl, ds, dpt, dc, dlh, deh, dtr) < 0:   # skip teardown/zero row
            continue
        t.append(float(b["wall_s"]))
        load.append(dl / dt); store.append(ds / dt); ptok.append(dpt / dt); tier.append(dtr / dt)
        comp.append(dc / dt); lhit.append(dlh / dt); ehit.append(deh / dt)
    if len(t) < 2:
        return None
    fig, (ax, ax2) = plt.subplots(2, 1, figsize=(8, 5.2), sharex=True,
                                  gridspec_kw={"height_ratios": [3, 2]})
    if has_src:
        ax.stackplot(t, comp, lhit, ehit,
                     labels=["GPU-computed (recompute)", "GPU prefix-cache hit", "offload-tier hit (KV load)"],
                     colors=["#4C78A8", "#54A24B", "#F58518"], alpha=0.85)
        # The title uses the full-run /metrics snapshot's computed % (the table's) when
        # given; the poller's own split only when it is not (it misses scrapes).
        tot = sum(comp) + sum(lhit) + sum(ehit)
        if tot and computed_pct is None:
            computed_pct = 100 * sum(comp) / tot
    else:
        ax.plot(t, ptok, color="#4C78A8", label="prompt tokens (source split not in this kvio.csv)")
    if computed_pct is not None:
        title = f"{title} — {computed_pct:.1f}% of prompt tokens computed"
    ax.set_title(title, fontsize=11)
    ax.set_ylabel("prompt tokens / s")
    ax.legend(loc="upper right", fontsize=8, ncol=3)
    ax.grid(True, alpha=0.3)
    io_series, io_labels, io_colors = [load, store], ["KV load IO (to GPU)", "KV store IO (GPU→tier)"], ["#F58518", "#E45756"]
    if has_tier and any(tier):
        io_series.insert(0, tier)
        io_labels.insert(0, "SSD→CPU tier read (cputier)")
        io_colors.insert(0, "#9D755D")
    ax2.stackplot(t, *io_series, labels=io_labels, colors=io_colors, alpha=0.85)
    xlabel = "wall time (s)"
    if tier_read_s and not has_tier:
        xlabel += (f"\nnot plotted: {tier_read_s:,.0f} s of SSD→CPU tier reads "
                   "(older kvio.csv; in the KV I/O table)")
    ax2.set_xlabel(xlabel)
    ax2.set_ylabel("device-s / wall-s")
    ax2.legend(loc="upper right", fontsize=8, ncol=len(io_series))
    ax2.grid(True, alpha=0.3)
    return png_b64(fig)


def png_b64(fig):
    buf = io.BytesIO()
    fig.savefig(buf, format="png", dpi=120, bbox_inches="tight")
    plt.close(fig)
    return base64.b64encode(buf.getvalue()).decode()


def fmt(v, nd=1):
    return "—" if v is None else f"{v:,.{nd}f}"


def wall_load_rate(rows, window_s=300.0):
    """Bytes loaded to the GPU per wall-clock second, from poll-kv-metrics.sh rows:
    over the whole capture and over its busiest ``window_s`` stretch (loads ramp up
    late in these runs, so the whole-run average hides the loading phase). Returns
    (whole_gbs, peak_gbs) in GB/s (10^9), or (None, None)."""
    pts = []
    for r in rows or []:
        try:
            pts.append((float(r["wall_s"]), float(r["load_bytes"])))
        except (KeyError, ValueError):
            continue
    # Drop a trailing all-zero teardown row and anything after a counter reset.
    return _wall_rate(pts, window_s)


def _wall_rate(pts, window_s):
    clean = []
    for t, b in pts:
        if clean and (b < clean[-1][1] or t <= clean[-1][0]):
            break
        clean.append((t, b))
    if len(clean) < 2 or clean[-1][0] - clean[0][0] <= 0:
        return None, None
    whole = (clean[-1][1] - clean[0][1]) / (clean[-1][0] - clean[0][0]) / 1e9
    peak, j = 0.0, 0
    for i in range(len(clean)):
        while clean[i][0] - clean[j][0] > window_s:
            j += 1
        span = clean[i][0] - clean[j][0]
        if span >= window_s * 0.9:
            peak = max(peak, (clean[i][1] - clean[j][1]) / span / 1e9)
    return whole, (peak or None)


def parse_certus_tier_events(path):
    """Certus's last tier-events line (certus-server.log): blocks served to the GPU
    and blocks promoted SSD -> memory tier first, so the share of loads read from
    SSD. Returns {} when the log or the line is absent (cputier arms)."""
    try:
        with open(path, errors="replace") as f:
            lines = [ln for ln in f if "tier-events" in ln]
    except Exception:
        return {}
    if not lines:
        return {}
    mo = re.search(r"promotions\[->memory (\d+), ->gpu (\d+)\]", lines[-1])
    if not mo:
        return {}
    to_mem, to_gpu = int(mo.group(1)), int(mo.group(2))
    return {"to_memory": to_mem, "to_gpu": to_gpu,
            "ssd_pct": 100.0 * min(to_mem, to_gpu) / to_gpu if to_gpu else None}


def parse_run_facts(sub):
    """What the arm's own logs say it ran: model, window, TP, KV bytes per token and
    vLLM version (server.log), the client's prefix length/count, turns and think
    time (client.log), and streams / run length (guidellm.json). Missing ones are
    absent from the dict."""
    f = {}
    try:
        with open(os.path.join(sub, "server.log"), errors="replace") as fh:
            head = fh.read(400000)
    except Exception:
        head = ""
    for key, pat in (("model", r"'model': '([^']+)'"),
                     ("window", r"'max_model_len': (\d+)"),
                     ("vllm", r"version (\d+\.\d+\.\d+)"),
                     ("tp", r"\bTP=(\d+)"),
                     ("kv_kib", r"\(\w+, (\d+) KiB/token\)"),
                     ("attn", r"\((GQA|MHA|MLA)\b")):
        mo = re.search(pat, head)
        if mo:
            f[key] = mo.group(1)
    try:
        with open(os.path.join(sub, "client.log"), errors="replace") as fh:
            chead = fh.read(20000)
    except Exception:
        chead = ""
    mo = re.search(r"prefix=(\d+)x(\d+)", chead)
    if mo:
        f["prefix_tokens"], f["prefix_count"] = int(mo.group(1)), int(mo.group(2))
    mo = re.search(r"turns=(\d+)", chead)
    if mo:
        f["turns"] = mo.group(1)
    mo = re.search(r"think time \(s\):.*?\(cap=[^,]*, scale=([0-9.]+)\)", chead)
    if mo:
        f["think_scale"] = mo.group(1)
    try:
        b = json.load(open(os.path.join(sub, "guidellm.json")))["benchmarks"][0]
        strat = (b.get("config") or {}).get("strategy") or {}
        if strat.get("streams") or strat.get("max_concurrency"):
            f["streams"] = str(strat.get("streams") or strat.get("max_concurrency"))
    except Exception:
        pass
    return f


def parse_certus_key_bytes(path):
    """Bytes per Certus key from the connector's startup line in server.log
    ("[certus-shmq] KV N region(s) block_bytes=B ... blocks_per_chunk=C"; C is 1 when
    absent, as before keys could hold several GPU blocks), or None."""
    try:
        with open(path, errors="replace") as fh:
            head = fh.read(400000)
    except Exception:
        return None
    mo = re.search(r"\[certus-shmq\] KV \d+ region\(s\) block_bytes=(\d+)(?:.*?blocks_per_chunk=(\d+))?", head)
    if not mo:
        return None
    return int(mo.group(1)) * int(mo.group(2) or 1)


def parse_serve_log(path):
    """Offload block size and DRAM tier size from the serve wrapper's header line
    ("... offload-block=64 ... DRAM-tier=64G" or "CPU-tier=64G"), or {}."""
    try:
        with open(path, errors="replace") as f:
            head = f.read(65536)
    except Exception:
        return {}
    blk = re.search(r"offload-block=(\S+)", head)
    tier = re.search(r"(?:DRAM|CPU)-tier=(\S+)", head)
    return {"block": ("16" if blk.group(1) == "gpu" else blk.group(1)) if blk else None,
            "tier": tier.group(1) if tier else None}


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: gen-iobound-report.py RESULTS_DIR")
    rd = sys.argv[1]

    # Run parameters: read from each arm's logs (parse_run_facts); the environment
    # (set by the orchestrator / client wrappers) only fills what the logs lack.
    env = os.environ
    facts = {key: parse_run_facts(os.path.join(rd, key)) for key, _, _ in ARMS}

    def run_value(name, env_key=None):
        vals = sorted({str(f[name]) for f in facts.values() if name in f})
        if vals:
            return " / ".join(vals)
        return env.get(env_key, "?") if env_key else "?"

    p_model = run_value("model")
    p_attn = run_value("attn")
    p_kv = run_value("kv_kib")
    p_tp = run_value("tp")
    p_window = run_value("window")
    p_vllm = run_value("vllm", "VLLM_LABEL")
    p_turns = run_value("turns", "TURNS")
    p_streams = run_value("streams", "CONCURRENT_STREAMS")
    p_prefix = run_value("prefix_count", "PREFIX_COUNT")
    p_prefix_tok = run_value("prefix_tokens", "PREFIX_TOKENS")
    p_scale = run_value("think_scale", "DELAY_SCALE")
    has_intervals = any(d.endswith("-intervals") and facts[d] for d in facts)
    scale_note = (f"think-time scale={p_scale} (intervals arms)" if has_intervals
                  else "no intervals arms")
    try:  # reused working set = prefix count x prefix tokens x KV bytes per token
        p_ws = f"~{round(int(p_prefix) * int(p_prefix_tok) * int(p_kv) / (1024 * 1024))} GiB"
    except ValueError:
        p_ws = "?"
    data = []
    for key, label, color in ARMS:
        sub = os.path.join(rd, key)
        g = parse_guidellm(os.path.join(sub, "guidellm.json"))
        k = parse_kvio(os.path.join(sub, "kvio.csv"))
        mt = parse_metrics(os.path.join(sub, "guidellm.metrics.txt")) or {}
        sv = parse_serve_log(os.path.join(sub, "server.log"))
        te = parse_certus_tier_events(os.path.join(sub, "certus-server.log"))
        certus_ssd_b = None
        key_b = parse_certus_key_bytes(os.path.join(sub, "server.log"))
        lb = mt.get("load_bytes") or 0.0
        if te.get("to_gpu") and key_b and lb:
            # Only when the server's served-key count reproduces vLLM's load bytes (within
            # 2%): then each counted key is one key of key_b bytes.
            if abs(te["to_gpu"] * key_b - lb) <= 0.02 * lb:
                certus_ssd_b = te["to_memory"] * key_b
        wall_gbs, peak_gbs = wall_load_rate(k["rows"] if k else None)
        st_pts = []
        for r in (k["rows"] if k else []):
            try:
                st_pts.append((float(r["wall_s"]), float(r["store_bytes"])))
            except (KeyError, ValueError):
                continue
        st_wall, st_peak = _wall_rate(st_pts, 300.0)
        # KV totals: prefer the full-run metrics.txt snapshot; fall back to kvio.
        kv = {"load_bytes": mt.get("load_bytes") or (k["load_bytes"] if k else 0),
              "store_bytes": mt.get("store_bytes") or (k["store_bytes"] if k else 0),
              "load_time_s": mt.get("load_time_s") or (k["load_time_s"] if k else 0),
              "store_time_s": mt.get("store_time_s") or (k["store_time_s"] if k else 0)}
        data.append({"key": key, "label": label, "color": color, "g": g, "k": k,
                     "kv": kv, "ext": mt.get("ext_hit"), "computed": mt.get("computed_pct"),
                     "tier_read_s": mt.get("tier_read_time_s") or 0.0,
                     "tier_read_b": mt.get("tier_read_bytes") or 0.0,
                     "queue_s": mt.get("queue_s"), "prefill_s": mt.get("prefill_s"),
                     "srv_ttft_s": mt.get("ttft_s"),
                     "block": sv.get("block"), "tier": sv.get("tier"),
                     "ssd_pct": te.get("ssd_pct"), "wall_gbs": wall_gbs, "peak_gbs": peak_gbs,
                     "certus_ssd_b": certus_ssd_b,
                     "st_wall_gbs": st_wall, "st_peak_gbs": st_peak,
                     "tier_write_s": mt.get("tier_write_time_s") or 0.0,
                     "tier_write_b": mt.get("tier_write_bytes") or 0.0,
                     "present": g is not None})

    present = [d for d in data if d["present"]]
    labels = [d["label"] for d in present]
    colors = [d["color"] for d in present]

    # ── Plot 1: throughput (tokens/s) ────────────────────────────────────────
    imgs = {}
    if present:
        fig, ax = plt.subplots(figsize=(8, 4.2))
        vals = [(d["g"]["tokens_per_s"] or 0) for d in present]
        bars = ax.bar(labels, vals, color=colors)
        ax.set_ylabel("tokens / second (total)")
        ax.set_title("Throughput")
        ax.grid(True, axis="y", alpha=0.3)
        for b, v in zip(bars, vals):
            ax.text(b.get_x() + b.get_width() / 2, v, f"{v:,.0f}", ha="center", va="bottom", fontsize=9)
        plt.xticks(rotation=15, ha="right")
        imgs["throughput"] = png_b64(fig)

        # ── Plot 2: TTFT p50/p90/p99 grouped ─────────────────────────────────
        fig, ax = plt.subplots(figsize=(8, 4.2))
        import numpy as np
        x = np.arange(len(present)); w = 0.25
        for i, p in enumerate(("p50", "p90", "p99")):
            vals = [((d["g"]["ttft_ms"] or {}).get(p) or 0) / 1000 for d in present]
            ax.bar(x + (i - 1) * w, vals, w, label=p)
        ax.set_xticks(x); ax.set_xticklabels(labels, rotation=15, ha="right")
        ax.set_ylabel("time to first token (s)")
        ax.set_title("TTFT (lower is better)")
        ax.legend(); ax.grid(True, axis="y", alpha=0.3)
        imgs["ttft"] = png_b64(fig)

        # ── Plot 3: KV load IO (TiB) ─────────────────────────────────────────
        fig, ax = plt.subplots(figsize=(8, 4.2))
        lv = [d["kv"]["load_bytes"] / TIB for d in present]
        sv = [d["kv"]["store_bytes"] / TIB for d in present]
        tv = [(d["tier_read_b"] or d["certus_ssd_b"] or 0) / TIB for d in present]
        x = np.arange(len(present))
        if any(tv):
            w = 0.27
            ax.bar(x - w, tv, w, label="SSD read into host memory", color="#9D755D")
            ax.bar(x, lv, w, label="KV load (to GPU)", color="#F58518")
            ax.bar(x + w, sv, w, label="KV store (GPU→tier)", color="#E45756")
        else:
            w = 0.38
            ax.bar(x - w / 2, lv, w, label="KV load (to GPU)", color="#F58518")
            ax.bar(x + w / 2, sv, w, label="KV store (GPU→tier)", color="#E45756")
        ax.set_xticks(x); ax.set_xticklabels(labels, rotation=15, ha="right")
        ax.set_ylabel("KV transferred (TiB)")
        ax.set_title("KV offload traffic")
        ax.legend(); ax.grid(True, axis="y", alpha=0.3)
        imgs["kvio"] = png_b64(fig)

    # ── Tables: results (client + vLLM per-request), then KV I/O ──────────────
    # Arms that did not run are left out; the note lists them.
    def table(cols, rows):
        head = "".join(f"<th>{c}</th>" for c in cols)
        body = "".join("<tr>" + "".join(f"<td>{c}</td>" for c in r) + "</tr>" for r in rows)
        return f"<table><thead><tr>{head}</tr></thead><tbody>{body}</tbody></table>"

    def sec(ms):
        return fmt(ms / 1000, 1) if ms is not None else "—"

    res_cols = ["Arm", "offload block (tok)", "DRAM tier", "run (s)", "tokens/s", "req/s", "ok",
                "errored", "TTFT p50 (s)", "TTFT p90 (s)", "TTFT p99 (s)", "vLLM queue mean (s)",
                "vLLM prefill mean (s)", "ext hit %", "computed %"]
    io_cols = ["Arm", "KV load (TiB)", "load GB/s, wall clock (run)",
               "load GB/s, wall clock (busiest 5 min)", "SSD read into host memory (TiB)",
               "SSD read not delivered to GPU, at least (TiB)", "summed load job time, SSD→GPU (s)"]
    io_cols += ["KV store (TiB)", "store GB/s, wall clock (run)",
                "store GB/s, wall clock (busiest 5 min)", "CPU→SSD tier write (TiB)"]
    res_rows, io_rows = [], []
    for d in present:
        g, kv, tt = d["g"], d["kv"], d["g"]["ttft_ms"] or {}
        lt, tr = kv["load_time_s"], d["tier_read_s"]
        res_rows.append([
            d["label"], d["block"] or "—", d["tier"] or "—", fmt(g.get("duration"), 0),
            fmt(g["tokens_per_s"], 0), fmt(g["requests_per_s"], 2), fmt(g["ok"], 0),
            fmt(g["errored"], 0), sec(tt.get("p50")), sec(tt.get("p90")), sec(tt.get("p99")),
            fmt(d["queue_s"], 1), fmt(d["prefill_s"], 2),
            fmt(d["ext"], 1) if d["ext"] is not None else "—",
            fmt(d["computed"], 1) if d["computed"] is not None else "—",
        ])
        # SSD bytes that never reached the GPU: read into host memory, then evicted
        # before their load, or for requests that recomputed instead. A lower bound:
        # loads served from memory without an SSD read offset it.
        ssd_b = d["tier_read_b"] if tr else d["certus_ssd_b"]
        if ssd_b and kv["load_bytes"]:
            ssd_cell = fmt(max(ssd_b - kv["load_bytes"], 0.0) / TIB, 2)
        else:
            ssd_cell = "—"
        # Certus has no separate tier step (its load reads SSD itself; it writes SSD in
        # the background); a cputier arm without an SSD tier reads and writes none.
        certus = d["key"].startswith("certus")
        no_tier_write = "n/a (background)" if certus else "0"
        row = [
            d["label"], fmt(kv["load_bytes"] / TIB, 2),
            fmt(d["wall_gbs"], 1), fmt(d["peak_gbs"], 1),
            fmt(ssd_b / TIB, 2) if ssd_b else "—",
            ssd_cell,
            # cputier's SSD->CPU copies are separate jobs: add their time. Certus's SSD
            # read is inside its load jobs, so its load time already covers it.
            fmt(lt + tr, 0),
        ]
        tw = d["tier_write_s"]
        row += [fmt(kv["store_bytes"] / TIB, 2), fmt(d["st_wall_gbs"], 1), fmt(d["st_peak_gbs"], 1),
                fmt(d["tier_write_b"] / TIB, 2) if tw else no_tier_write]
        io_rows.append(row)
    missing = [d["label"] for d in data if not d["present"]]
    missing_note = (f'<p class="note">Not run: {", ".join(missing)}.</p>' if missing else "")
    tables_html = (table(res_cols, res_rows) + missing_note
                   + "<h2 style='font-size:1.1rem'>KV I/O</h2>" + table(io_cols, io_rows))

    img_html = "".join(
        f'<figure><img src="data:image/png;base64,{imgs[k]}"/></figure>'
        for k in ("throughput", "ttft", "kvio") if k in imgs
    )

    # ── Per-arm GPU-vs-KV-IO timelines (from poll-kv-metrics.sh / kvio.csv) ────
    tl = []
    for d in data:
        if d["present"] and d["k"] and d["k"].get("rows"):
            png = timeline_png(d["k"]["rows"], d["label"], d["computed"], d["tier_read_s"])
            if png:
                tl.append(f'<figure><img src="data:image/png;base64,{png}"/></figure>')
    timeline_html = ""
    if tl:
        timeline_html = ("<h2 style='font-size:1.1rem'>Per-arm prompt-token sources vs KV-IO over time</h2>"
                         "<p class='note'>Top: prompt tokens/s split by where their KV came from (vLLM "
                         "prompt_tokens_by_source): GPU-computed = actual recompute, GPU prefix-cache hit, and "
                         "offload-tier hit (reloaded over the connector). Bottom: KV load/store device-seconds per "
                         "wall-second; for cputier also its SSD→CPU tier reads, which run as separate jobs before "
                         "its CPU→GPU load (Certus's load includes its SSD read). Arms whose kvio.csv predates the source split show total prompt tok/s with the "
                         "full-run computed % from the /metrics snapshot. A timeline that ends before the run "
                         "does means the live poller missed scrapes when /metrics slowed under load.</p>"
                         + "".join(tl))
    html = f"""<!doctype html><html><head><meta charset="utf-8">
<title>KV-IO-bound: Certus vs cputier</title>
<style>
 body{{font-family:system-ui,Arial,sans-serif;margin:2rem;max-width:1100px;color:#1a1a1a}}
 h1{{font-size:1.4rem}} table{{border-collapse:collapse;width:100%;margin:1rem 0;font-size:.9rem}}
 th,td{{border:1px solid #ddd;padding:.4rem .6rem;text-align:right}} th:first-child,td:first-child{{text-align:left}}
 th{{background:#f3f4f6}} tr:nth-child(even){{background:#fafafa}}
 figure{{margin:1rem 0;text-align:center}} img{{max-width:100%;border:1px solid #eee;border-radius:6px}}
 .note{{color:#555;font-size:.85rem}}
</style></head><body>
<h1>{p_attn} KV-IO-bound experiment — Certus vs cputier</h1>
<p class="note">{p_model} ({p_attn}, {p_kv} KiB/token), TP={p_tp}, {p_window}-token window, vLLM {p_vllm};
offload block size and DRAM/CPU tier per arm in the table (from each arm's server.log).
Run parameters (from the arms' logs): turns={p_turns}, concurrent streams={p_streams}, prefixes={p_prefix} x
{p_prefix_tok} tokens ({p_ws} reused working set), {scale_note}. Source: {rd}</p>
{tables_html}
{img_html}
{timeline_html}
<div class="note"><p>How to read the columns:</p><ul>
<li><b>tokens/s, req/s, TTFT</b>: from the client (guidellm). TTFT is send to first token. <b>run</b> is the client's
measurement window; compare arms only at the same run length.</li>
<li><b>vLLM queue / prefill</b>: vLLM's per-request means, measured the same on every backend. A request whose KV is
still loading stays queued, so queue time includes its load wait.</li>
<li><b>ext hit %</b>: of the prompt tokens the GPU prefix cache did not hold, the share found in the offload tier.
<b>computed %</b>: of all prompt tokens, the share recomputed on the GPU (in neither the GPU prefix cache nor the
offload tier).</li>
<li><b>KV load / KV store</b>: bytes moved offload tier → GPU and GPU → offload tier, from vLLM's counters over the run.</li>
<li><b>load GB/s, wall clock</b>: KV bytes delivered to the GPU per second of elapsed time (live poller), over the whole
run and over its busiest 5 minutes. This is the load throughput to compare. The same for store.</li>
<li><b>SSD read into host memory</b>: KV read from SSD on its way to the GPU. cputier: its separate SSD→CPU-tier copies,
counted by vLLM. Certus: from its server's counters (keys moved SSD → memory tier, from certus-server.log, × bytes
per key, from the connector's line in server.log), read inside its load jobs; shown only when the server's
served-key count reproduces KV load.</li>
<li><b>SSD read not delivered to GPU, at least</b>: SSD read minus KV load. At least this much was fetched from SSD
and then evicted before its load ran, or belonged to requests that recomputed. A lower bound: loads served from
memory without an SSD read hide some, so 0 does not mean nothing was wasted.</li>
<li><b>summed load job time, SSD→GPU</b>: the time of all KV load jobs added up, from SSD to GPU. Certus: its load
jobs, which include the SSD read. cputier: its CPU→GPU load jobs plus its separate SSD→CPU copy jobs (vLLM's own
load time for cputier covers only the CPU→GPU copy). Jobs overlap, so this is work done, not elapsed time.</li>
<li><b>CPU→SSD tier write</b>: cputier's separate copies from its CPU tier to SSD. Certus writes to SSD in the
background without reporting it to vLLM.</li>
<li><b>errored</b>: requests that failed (e.g. HTTP 400 from window overflow on deep turns).</li>
<li>Runs whose kvio.csv predates the 2026-10-28 poller double-counted cputier's loads, so their cputier wall-clock rates
are about twice the true rate (the TiB columns come from vLLM's counters and are right).</li>
</ul></div>
</body></html>"""
    out = os.path.join(rd, "report.html")
    open(out, "w").write(html)
    print(f"wrote {out} ({len(present)}/{len(data)} arms present)")


if __name__ == "__main__":
    main()
