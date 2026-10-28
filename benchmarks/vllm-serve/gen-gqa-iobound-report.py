#!/usr/bin/env python3
"""gen-gqa-iobound-report.py — build an HTML comparison report (throughput + TTFT,
with plots) from a results directory produced by run-gqa-iobound-matrix.sh.

Expects RESULTS_DIR to contain one subdir per arm, each with:
  guidellm.json          guidellm benchmark output (throughput, TTFT, counts)
  guidellm.metrics.txt   before/after /metrics snapshot (external cache hit)
  kvio.csv               poll-kv-metrics.sh timeline (KV load/store IO)
Missing files are tolerated (the arm shows N/A).

  ./gen-gqa-iobound-report.py /mnt/certus1/vllm-serve-matrix/gqa-iobound-<stamp>
Writes report.html (+ PNGs) into that directory.
"""
import base64
import csv
import io
import json
import os
import re
import sys

import matplotlib
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
        key = name + ("|" + tt.group(1) if tt else "") + ("|" + src.group(1) if src else "")
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
            "ext_hit": (100 * h / q) if q else None, "computed_pct": computed}


def timeline_png(rows, title, computed_pct=None):
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
    t, comp, lhit, ehit, ptok, load, store = [], [], [], [], [], [], []
    for a, b in zip(rows, rows[1:]):
        try:
            dt = float(b["wall_s"]) - float(a["wall_s"])
            dl = float(b["load_time_s"]) - float(a["load_time_s"])
            ds = float(b["store_time_s"]) - float(a["store_time_s"])
            dpt = float(b["prompt_tok"]) - float(a["prompt_tok"])
            if has_src:
                dc = float(b["computed_tok"]) - float(a["computed_tok"])
                dlh = float(b["local_hit_tok"]) - float(a["local_hit_tok"])
                deh = float(b["ext_hit_tok"]) - float(a["ext_hit_tok"])
            else:
                dc = dlh = deh = 0.0
        except (KeyError, ValueError):
            continue
        if dt <= 0 or min(dl, ds, dpt, dc, dlh, deh) < 0:   # skip teardown/zero row
            continue
        t.append(float(b["wall_s"]))
        load.append(dl / dt); store.append(ds / dt); ptok.append(dpt / dt)
        comp.append(dc / dt); lhit.append(dlh / dt); ehit.append(deh / dt)
    if len(t) < 2:
        return None
    fig, (ax, ax2) = plt.subplots(2, 1, figsize=(8, 5.2), sharex=True,
                                  gridspec_kw={"height_ratios": [3, 2]})
    if has_src:
        ax.stackplot(t, comp, lhit, ehit,
                     labels=["GPU-computed (recompute)", "GPU prefix-cache hit", "offload-tier hit (KV load)"],
                     colors=["#4C78A8", "#54A24B", "#F58518"], alpha=0.85)
        tot = sum(comp) + sum(lhit) + sum(ehit)
        if tot:
            computed_pct = 100 * sum(comp) / tot
    else:
        ax.plot(t, ptok, color="#4C78A8", label="prompt tokens (source split not in this kvio.csv)")
    if computed_pct is not None:
        title = f"{title} — {computed_pct:.1f}% of prompt tokens computed"
    ax.set_title(title, fontsize=11)
    ax.set_ylabel("prompt tokens / s")
    ax.legend(loc="upper right", fontsize=8, ncol=3)
    ax.grid(True, alpha=0.3)
    ax2.stackplot(t, load, store, labels=["KV load IO (CPU→GPU)", "KV store IO (GPU→CPU)"],
                  colors=["#F58518", "#E45756"], alpha=0.85)
    ax2.set_xlabel("wall time (s)")
    ax2.set_ylabel("device-s / wall-s")
    ax2.legend(loc="upper right", fontsize=8, ncol=2)
    ax2.grid(True, alpha=0.3)
    return png_b64(fig)


def png_b64(fig):
    buf = io.BytesIO()
    fig.savefig(buf, format="png", dpi=120, bbox_inches="tight")
    plt.close(fig)
    return base64.b64encode(buf.getvalue()).decode()


def fmt(v, nd=1):
    return "—" if v is None else f"{v:,.{nd}f}"


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
        sys.exit("usage: gen-gqa-iobound-report.py RESULTS_DIR")
    rd = sys.argv[1]

    # Run parameters passed through from the environment (set by the orchestrator /
    # the client wrappers). Fall back to the gqa-iobound client defaults when unset.
    env = os.environ
    p_turns = env.get("TURNS", "128")
    p_streams = env.get("CONCURRENT_STREAMS", "8")
    p_prefix = env.get("PREFIX_COUNT", "64")
    p_scale = env.get("DELAY_SCALE", "0.1")
    p_vllm = env.get("VLLM_LABEL", "0.30 both arms")
    try:  # reused working set = prefix_count x 8192 tok x 56 KiB/tok = prefix_count x ~0.4375 GiB
        p_ws = f"~{round(int(float(p_prefix)) * 8192 * 56 / (1024 * 1024))} GiB"
    except ValueError:
        p_ws = "?"
    data = []
    for key, label, color in ARMS:
        sub = os.path.join(rd, key)
        g = parse_guidellm(os.path.join(sub, "guidellm.json"))
        k = parse_kvio(os.path.join(sub, "kvio.csv"))
        mt = parse_metrics(os.path.join(sub, "guidellm.metrics.txt")) or {}
        sv = parse_serve_log(os.path.join(sub, "server.log"))
        # KV totals: prefer the full-run metrics.txt snapshot; fall back to kvio.
        kv = {"load_bytes": mt.get("load_bytes") or (k["load_bytes"] if k else 0),
              "store_bytes": mt.get("store_bytes") or (k["store_bytes"] if k else 0),
              "load_time_s": mt.get("load_time_s") or (k["load_time_s"] if k else 0),
              "store_time_s": mt.get("store_time_s") or (k["store_time_s"] if k else 0)}
        data.append({"key": key, "label": label, "color": color, "g": g, "k": k,
                     "kv": kv, "ext": mt.get("ext_hit"), "computed": mt.get("computed_pct"),
                     "block": sv.get("block"), "tier": sv.get("tier"),
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
            vals = [((d["g"]["ttft_ms"] or {}).get(p) or 0) for d in present]
            ax.bar(x + (i - 1) * w, vals, w, label=p)
        ax.set_xticks(x); ax.set_xticklabels(labels, rotation=15, ha="right")
        ax.set_ylabel("time to first token (ms)")
        ax.set_title("TTFT (lower is better)")
        ax.legend(); ax.grid(True, axis="y", alpha=0.3)
        imgs["ttft"] = png_b64(fig)

        # ── Plot 3: KV load IO (TiB) ─────────────────────────────────────────
        fig, ax = plt.subplots(figsize=(8, 4.2))
        lv = [d["kv"]["load_bytes"] / TIB for d in present]
        sv = [d["kv"]["store_bytes"] / TIB for d in present]
        x = np.arange(len(present)); w = 0.38
        ax.bar(x - w / 2, lv, w, label="KV load (CPU→GPU)", color="#F58518")
        ax.bar(x + w / 2, sv, w, label="KV store (GPU→CPU)", color="#E45756")
        ax.set_xticks(x); ax.set_xticklabels(labels, rotation=15, ha="right")
        ax.set_ylabel("KV transferred (TiB)")
        ax.set_title("KV offload traffic")
        ax.legend(); ax.grid(True, axis="y", alpha=0.3)
        imgs["kvio"] = png_b64(fig)

    # ── Table ────────────────────────────────────────────────────────────────
    cols = ["Arm", "offload block (tok)", "DRAM tier", "tokens/s", "out tok/s", "req/s",
            "TTFT p50 (ms)", "TTFT p90 (ms)", "TTFT p99 (ms)",
            "ok", "errored",
            "KV load (TiB)", "load time (s)", "load GB/s",
            "KV store (TiB)", "store time (s)", "store GB/s",
            "ext hit %", "computed %"]
    rows_html = []
    for d in data:
        g, kv, e = d["g"], d["kv"], d["ext"]
        if not d["present"]:
            rows_html.append(f"<tr><td>{d['label']}</td>" + "<td>—</td>" * (len(cols) - 1) + "</tr>")
            continue
        tt = g["ttft_ms"] or {}
        load_gbs = kv["load_bytes"] / GIB / kv["load_time_s"] if kv["load_time_s"] else None
        store_gbs = kv["store_bytes"] / GIB / kv["store_time_s"] if kv["store_time_s"] else None
        cells = [
            d["label"], d["block"] or "—", d["tier"] or "—", fmt(g["tokens_per_s"], 0), fmt(g["output_tokens_per_s"], 1),
            fmt(g["requests_per_s"], 2), fmt(tt.get("p50")), fmt(tt.get("p90")),
            fmt(tt.get("p99")), fmt(g["ok"], 0), fmt(g["errored"], 0),
            fmt(kv["load_bytes"] / TIB, 2), fmt(kv["load_time_s"], 0), fmt(load_gbs, 1),
            fmt(kv["store_bytes"] / TIB, 2), fmt(kv["store_time_s"], 0), fmt(store_gbs, 1),
            fmt(e, 1) if e is not None else "—",
            fmt(d["computed"], 1) if d["computed"] is not None else "—",
        ]
        rows_html.append("<tr>" + "".join(f"<td>{c}</td>" for c in cells) + "</tr>")

    img_html = "".join(
        f'<figure><img src="data:image/png;base64,{imgs[k]}"/></figure>'
        for k in ("throughput", "ttft", "kvio") if k in imgs
    )

    # ── Per-arm GPU-vs-KV-IO timelines (from poll-kv-metrics.sh / kvio.csv) ────
    tl = []
    for d in data:
        if d["present"] and d["k"] and d["k"].get("rows"):
            png = timeline_png(d["k"]["rows"], d["label"], d["computed"])
            if png:
                tl.append(f'<figure><img src="data:image/png;base64,{png}"/></figure>')
    timeline_html = ""
    if tl:
        timeline_html = ("<h2 style='font-size:1.1rem'>Per-arm prompt-token sources vs KV-IO over time</h2>"
                         "<p class='note'>Top: prompt tokens/s split by where their KV came from (vLLM "
                         "prompt_tokens_by_source): GPU-computed = actual recompute, GPU prefix-cache hit, and "
                         "offload-tier hit (reloaded over the connector). Bottom: KV load/store device-seconds per "
                         "wall-second. Arms whose kvio.csv predates the source split show total prompt tok/s with the "
                         "full-run computed % from the /metrics snapshot. "
                         "The certus timelines truncate early (the live poller stopped when /metrics slowed under load).</p>"
                         + "".join(tl))
    thead = "".join(f"<th>{c}</th>" for c in cols)
    html = f"""<!doctype html><html><head><meta charset="utf-8">
<title>GQA KV-IO-bound: Certus vs cputier</title>
<style>
 body{{font-family:system-ui,Arial,sans-serif;margin:2rem;max-width:1100px;color:#1a1a1a}}
 h1{{font-size:1.4rem}} table{{border-collapse:collapse;width:100%;margin:1rem 0;font-size:.9rem}}
 th,td{{border:1px solid #ddd;padding:.4rem .6rem;text-align:right}} th:first-child,td:first-child{{text-align:left}}
 th{{background:#f3f4f6}} tr:nth-child(even){{background:#fafafa}}
 figure{{margin:1rem 0;text-align:center}} img{{max-width:100%;border:1px solid #eee;border-radius:6px}}
 .note{{color:#555;font-size:.85rem}}
</style></head><body>
<h1>GQA KV-IO-bound experiment — Certus vs cputier, with/without think-time intervals</h1>
<p class="note">Qwen2.5-7B-Instruct (GQA, 56 KiB/token — 1/9th the MHA arm), TP=1, 32768 window, vLLM {p_vllm};
offload block size and DRAM/CPU tier per arm in the table (from each arm's server.log). At PREFIX_COUNT=64 the ~28 GiB working set fits a 32 GiB tier (DRAM-resident); use PREFIX_COUNT≈512 to byte-match the MHA arm (~224 GiB → SSD).
Run parameters: TURNS={p_turns}, CONCURRENT_STREAMS={p_streams}, PREFIX_COUNT={p_prefix} ({p_ws} reused working set),
DELAY_SCALE={p_scale} (intervals arms). Source: {rd}</p>
<table><thead><tr>{thead}</tr></thead><tbody>{''.join(rows_html)}</tbody></table>
{img_html}
{timeline_html}
<p class="note">Throughput = total tokens/s (higher better). TTFT = time to first token (lower better).
KV load = bytes reloaded from the offload tier onto the GPU; KV store = bytes offloaded from GPU to the tier.
computed % = prompt tokens recomputed on the GPU (hit neither the GPU prefix cache nor the offload tier).
GB/s = effective throughput (total bytes / cumulative device-time). "errored" counts requests that failed (e.g. HTTP 400 from window overflow on deep turns).</p>
</body></html>"""
    out = os.path.join(rd, "report.html")
    open(out, "w").write(html)
    print(f"wrote {out} ({len(present)}/{len(data)} arms present)")


if __name__ == "__main__":
    main()
