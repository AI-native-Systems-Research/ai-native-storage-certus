#!/usr/bin/env python3
"""scrape_prom.py — poll a served vLLM's /metrics and emit renderer [prom] lines.

The in-process KV-offload drivers (run_multiturn_*.py) read vLLM's Prometheus
counters straight from the in-process REGISTRY and print per-round
``[prom] round N: k=v …`` lines that ``tools/render_kvprofile.py`` parses. The
synthetic-agentic path instead drives a SEPARATE ``vllm serve`` container over
HTTP, so those same counters live on that server's ``/metrics`` endpoint, not in
this process. Nothing was scraping them, so an SA run carried no offload-tier
metrics and render_kvprofile had nothing to plot.

This poller closes that gap WITHOUT any renderer change: it samples ``/metrics``
on an interval and writes the exact same ``[prom] round N: k=v …`` lines (values
are per-interval DELTAS, which render sums to whole-run totals — see its
``_active_seconds`` / family-rollup logic). The metric→key mapping is identical
to run_multiturn_async._prom_key (drop the ``vllm:`` prefix and a trailing
``_total``), so the output is byte-compatible with what the batched/async drivers
produce.

We deliberately keep NO curated counter list here (nothing to drift): every vLLM
``*_total`` counter is emitted, and render_kvprofile's parser already keeps only
the keys it plots (``if k in COUNTER_KEYS`` — tools/render_kvprofile.py) and
ignores the rest. Histograms/gauges are skipped by the ``_total`` filter.

A baseline is taken at startup and NOT emitted, so the summed deltas equal
(final − baseline) = the movement during the driven window, excluding server
warmup. On SIGTERM/SIGINT it takes one final sample, flushes, and exits 0 — so
the teardown in profile_all loses no tail.

Usage:
  scrape_prom.py --url http://localhost:8000/metrics --interval 10 --out run.prom.log
"""

from __future__ import annotations

import argparse
import re
import signal
import sys
import json
import time
import urllib.request

# A Prometheus exposition sample line: `name{labels} value` (labels optional).
# vLLM prefixes every metric with `vllm:`; counters carry a `_total` suffix.
_SAMPLE_RE = re.compile(r"^(vllm:[A-Za-z_:][A-Za-z0-9_:]*)(\{[^}]*\})?\s+([-+0-9.eEnaN]+)\s*$")


def _prom_key(name: str) -> str:
    """vLLM counter name -> renderer key: drop `vllm:` prefix + one `_total`.

    Mirrors run_multiturn_async._prom_key so a scraped /metrics line maps to the
    same curated key an in-process driver would print (e.g.
    `vllm:kv_offload_store_bytes_total` -> `kv_offload_store_bytes`)."""
    if name.startswith("vllm:"):
        name = name[len("vllm:"):]
    if name.endswith("_total"):
        name = name[: -len("_total")]
    return name


def scrape(url: str, timeout: float) -> dict:
    """Fetch /metrics once and return {key: summed_value} for every counter.

    Keeps only ``*_total`` samples (the Prometheus counter convention — this drops
    histogram buckets/sums and gauges) and sums each metric across its label sets
    (a metric may appear once per model/engine/finish-reason). No curation beyond
    that: render_kvprofile filters to the keys it plots. A failed fetch returns {}
    so a transient blip (server busy, mid-teardown) just skips a tick."""
    try:
        with urllib.request.urlopen(url, timeout=timeout) as resp:
            text = resp.read().decode("utf-8", "replace")
    except Exception as e:  # noqa: BLE001 - transient scrape errors are non-fatal
        print(f"[scrape_prom] fetch failed: {e}", file=sys.stderr, flush=True)
        return {}
    out: dict = {}
    for line in text.splitlines():
        if not line or line[0] == "#":
            continue
        m = _SAMPLE_RE.match(line)
        if not m:
            continue
        raw = m.group(1)
        if not raw.endswith("_total"):    # counters only; skip histograms/gauges
            continue
        try:
            val = float(m.group(3))
        except ValueError:
            continue
        key = _prom_key(raw)
        out[key] = out.get(key, 0.0) + val
    return out


# vLLM request-latency histograms to capture. MIRRORS
# run_multiturn_common.LATENCY_HISTOGRAMS — that module can't be imported here (it
# lives in the driver image / kv-offload-replay dir; this runs host-side against a
# container's /metrics). Keep the two sets in sync.
LATENCY_HISTOGRAMS = {
    "vllm:time_to_first_token_seconds",
    "vllm:time_per_output_token_seconds",
    "vllm:e2e_request_latency_seconds",
    "vllm:request_queue_time_seconds",
    "vllm:request_decode_time_seconds",
}

# A histogram exposition sample: `name_bucket{le="X",…} v`, `name_sum{…} v`, or
# `name_count{…} v`. The base-name char class also matches the `_bucket/_sum/_count`
# suffix, so the regex backtracks to anchor the suffix at the end.
_HIST_RE = re.compile(
    r'^(vllm:[A-Za-z_:][A-Za-z0-9_:]*)_(bucket|sum|count)(?:\{([^}]*)\})?\s+'
    r'([-+0-9.eEnaN]+)\s*$')
_LE_RE = re.compile(r'le="([^"]*)"')


def scrape_histograms(url: str, timeout: float) -> dict:
    """Fetch /metrics once; return {name: {count, sum, buckets{le: cumcount}}} for
    each LATENCY_HISTOGRAMS name, summed across label sets. Bucket counts are the
    cumulative Prometheus form (each ``le`` includes all lower). A failed fetch
    returns {} so a transient blip just yields no histogram data."""
    try:
        with urllib.request.urlopen(url, timeout=timeout) as resp:
            text = resp.read().decode("utf-8", "replace")
    except Exception as e:  # noqa: BLE001 - transient scrape errors are non-fatal
        print(f"[scrape_prom] hist fetch failed: {e}", file=sys.stderr, flush=True)
        return {}
    out: dict = {}
    for line in text.splitlines():
        if not line or line[0] == "#":
            continue
        m = _HIST_RE.match(line)
        if not m:
            continue
        base, kind, labels, raw_val = m.group(1), m.group(2), m.group(3), m.group(4)
        if base not in LATENCY_HISTOGRAMS:
            continue
        try:
            val = float(raw_val)
        except ValueError:
            continue
        agg = out.setdefault(base, {"count": 0.0, "sum": 0.0, "buckets": {}})
        if kind == "count":
            agg["count"] += val
        elif kind == "sum":
            agg["sum"] += val
        else:  # bucket
            lm = _LE_RE.search(labels or "")
            if lm:
                le = lm.group(1)
                agg["buckets"][le] = agg["buckets"].get(le, 0.0) + val
    return out


def _hist_pct(buckets: dict, count: float, p: float):
    """Percentile from cumulative buckets: the smallest ``le`` whose cumulative
    count first reaches ``p*count``. Mirrors run_multiturn_common.hist_pct."""
    if not count:
        return None
    target = p * count

    def _le(k):
        return float("inf") if k in ("+Inf", "inf", "Inf") else float(k)

    for le in sorted(buckets, key=_le):
        if buckets[le] >= target:
            return _le(le)
    return float("inf")


def _hist_delta(prev: dict, cur: dict) -> dict:
    """Window the cumulative histograms to the driven interval: cur − prev per
    name (count, sum, and each le bucket). Drops names with no requests in-window."""
    out: dict = {}
    for name, h in cur.items():
        b0 = prev.get(name, {})
        cnt = h["count"] - b0.get("count", 0.0)
        tot = h["sum"] - b0.get("sum", 0.0)
        b0b = b0.get("buckets", {})
        buckets = {le: h["buckets"][le] - b0b.get(le, 0.0) for le in h["buckets"]}
        if cnt > 0:
            out[name] = {"count": int(cnt), "sum": tot,
                         "buckets": {le: int(c) for le, c in buckets.items()}}
    return out


_STOP = False


def _handle_stop(signum, _frame):
    global _STOP
    _STOP = True


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", required=True, help="vLLM /metrics URL")
    ap.add_argument("--interval", type=float, default=10.0, help="seconds between samples")
    ap.add_argument("--out", required=True, help="file to append [prom] lines to")
    ap.add_argument("--timeout", type=float, default=5.0, help="per-scrape HTTP timeout")
    args = ap.parse_args()

    signal.signal(signal.SIGTERM, _handle_stop)
    signal.signal(signal.SIGINT, _handle_stop)

    fmt = lambda v: (str(int(v)) if float(v).is_integer() else repr(v))  # noqa: E731

    with open(args.out, "a", encoding="utf-8") as f:
        # Baseline (not emitted): summed deltas then measure the driven window
        # only, excluding whatever the server accrued during warmup/model load.
        prev = scrape(args.url, args.timeout)
        # Retry the baseline briefly if /metrics wasn't ready yet (empty) — the
        # caller starts us right after readiness, but the connector's counters can
        # register a beat later.
        for _ in range(5):
            if prev:
                break
            time.sleep(1.0)
            prev = scrape(args.url, args.timeout)
        # Baseline for the latency histograms too, so the final snapshot reflects
        # only the driven window (any warmup requests are subtracted out).
        prev_h = scrape_histograms(args.url, args.timeout)
        print(f"[scrape_prom] baseline keys={sorted(prev)} url={args.url}",
              file=sys.stderr, flush=True)

        rnd = 0
        # Sleep in short slices so a stop signal is honoured within ~0.2s rather
        # than after a whole interval.
        while True:
            waited = 0.0
            while waited < args.interval and not _STOP:
                time.sleep(0.2)
                waited += 0.2
            cur = scrape(args.url, args.timeout)
            if cur:
                delta = {k: cur.get(k, 0.0) - prev.get(k, 0.0) for k in cur}
                # Clamp tiny negative jitter (a counter reset shouldn't happen on a
                # live server, but a missed label set can look like a dip).
                shown = " ".join(f"{k}={fmt(max(0.0, delta[k]))}"
                                 for k in sorted(delta))
                f.write(f"[prom] round {rnd}: {shown}\n")
                f.flush()
                prev = cur
                rnd += 1
            if _STOP:
                break

        # Latency histograms are cumulative, so emit ONE windowed snapshot at the
        # end (cur − baseline) rather than per round — the same [prom] hist /
        # histjson form the in-process drivers print, so a downstream parser treats
        # serve-mode and in-process runs identically.
        hists = _hist_delta(prev_h, scrape_histograms(args.url, args.timeout))
        for name, h in sorted(hists.items()):
            cnt, tot = h["count"], h["sum"]
            mean = tot / cnt if cnt else 0.0
            hfmt = lambda x: "n/a" if x is None else f"{x:.3f}s"  # noqa: E731
            f.write(f"[prom] hist {name[len('vllm:'):]}: n={cnt} mean={mean:.3f}s "
                    f"p50={hfmt(_hist_pct(h['buckets'], cnt, 0.50))} "
                    f"p90={hfmt(_hist_pct(h['buckets'], cnt, 0.90))} "
                    f"p99={hfmt(_hist_pct(h['buckets'], cnt, 0.99))}\n")
        if hists:
            f.write(f"[prom] histjson {json.dumps(hists, separators=(',', ':'))}\n")
        f.flush()
        print(f"[scrape_prom] histograms captured: {sorted(n[len('vllm:'):] for n in hists)}",
              file=sys.stderr, flush=True)
    print(f"[scrape_prom] stopped after {rnd} rounds -> {args.out}",
          file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
