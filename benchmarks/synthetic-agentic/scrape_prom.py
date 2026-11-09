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

Several sources (llm-d, or any fleet of servers): ``--url`` repeats, each
optionally ``LABEL=URL``. Every source keeps its own baseline and its own
previous sample, so a source that misses a scrape contributes nothing that
round and its movement lands, whole, on its next good one -- never a negative
delta or a double count. The ``[prom] round`` line carries the SUM over sources
under the usual keys (so render_kvprofile reads a fleet as one server), plus
each labelled source's own values as ``LABEL.key``, which the renderer ignores
and a reader can split by role. certus servers' ``certus_*`` counters are kept
too, under their own names minus ``_total``.

Under P/D a request runs on two engines, so its per-request counters
(``request_success``, ``prefix_cache_queries``) appear on BOTH prefill and
decode and the sum counts it twice; read those per role. Where a role's count
is what you want, ``prefill.request_success`` is the client request count.

``--k8s`` finds the sources instead: every Running llm-d model server in
``--llm-d-namespace`` (labelled by its ``llm-d.ai/role``: prefill, decode, or
the pod name for aggregated serving) and every certus server in
``--certus-namespace`` (labelled ``certus-<node>-<numa>``). Discovery runs once,
at start; restarting a pod mid-run changes its IP, so restart the scraper too.

Usage:
  scrape_prom.py --url http://localhost:8000/metrics --interval 10 --out run.prom.log
  scrape_prom.py --url prefill=http://10.244.4.38:8000/metrics \
                 --url decode=http://10.244.2.131:8200/metrics --out run.prom.log
  scrape_prom.py --k8s --out run.prom.log      # llm-d-quickstart + certus
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import signal
import sys
import json
import time
import urllib.request

# A Prometheus exposition sample line: `name{labels} value` (labels optional).
# vLLM prefixes every metric with `vllm:`; counters carry a `_total` suffix.
# certus servers export `certus_*` counters under the same convention.
_SAMPLE_RE = re.compile(
    r"^((?:vllm:|certus_)[A-Za-z_:][A-Za-z0-9_:]*)(\{[^}]*\})?\s+([-+0-9.eEnaN]+)\s*$")


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
    "vllm:inter_token_latency_seconds",
    "vllm:request_time_per_output_token_seconds",
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


def _parse_url(spec: str) -> tuple[str, str]:
    """``LABEL=URL`` -> (label, url); a bare URL has no label."""
    head, sep, rest = spec.partition("=")
    if sep and "://" not in head:
        return head, rest
    return "", spec


def _kubectl_json(args: list) -> dict:
    out = subprocess.run(["kubectl", *args, "-o", "json"], check=True,
                         capture_output=True, text=True).stdout
    return json.loads(out)


def discover_k8s(llmd_ns: str, certus_ns: str) -> list:
    """[(label, url)] for every Running llm-d model server and certus server."""
    found = []
    for p in _kubectl_json(["get", "pods", "-n", llmd_ns])["items"]:
        if p["status"].get("phase") != "Running" or not p["status"].get("podIP"):
            continue
        for c in p["spec"]["containers"]:
            if c["name"] != "modelserver":
                continue
            port = "8000"  # vLLM's default; decode sits behind its sidecar on --port
            for a in c.get("args", []) + c.get("command", []):
                if a.startswith("--port="):
                    port = a.split("=", 1)[1]
            role = p["metadata"].get("labels", {}).get("llm-d.ai/role") or p["metadata"]["name"]
            found.append((role, f"http://{p['status']['podIP']}:{port}/metrics"))
    for p in _kubectl_json(["get", "pods", "-n", certus_ns, "-l", "app=certus-server"])["items"]:
        if p["status"].get("phase") != "Running":
            continue
        env = {e["name"]: e.get("value") for e in p["spec"]["containers"][0].get("env", [])}
        port, numa = env.get("CERTUS_METRICS_PORT"), env.get("CERTUS_NUMA_ID", "?")
        if not port:
            continue
        node = p["spec"].get("nodeName", "?").split(".")[0]
        found.append((f"certus-{node}-numa{numa}", f"http://{p['status']['hostIP']}:{port}/metrics"))
    return found


def _unique_labels(sources: list) -> list:
    """Two decode pods are both "decode": suffix repeats so each is its own."""
    seen: dict = {}
    out = []
    for label, url in sources:
        if label:
            n = seen.get(label, 0)
            seen[label] = n + 1
            if n:
                label = f"{label}{n}"
        out.append((label, url))
    # The first of a repeated label also gets its index, so a role reads
    # decode0, decode1 rather than decode, decode1.
    return [((f"{l}0" if l and seen.get(l, 0) > 1 else l), u) for l, u in out]


def _hist_merge(parts: list) -> dict:
    """Sum windowed histograms across sources (count, sum, each le bucket)."""
    out: dict = {}
    for hs in parts:
        for name, h in hs.items():
            agg = out.setdefault(name, {"count": 0, "sum": 0.0, "buckets": {}})
            agg["count"] += h["count"]
            agg["sum"] += h["sum"]
            for le, c in h["buckets"].items():
                agg["buckets"][le] = agg["buckets"].get(le, 0) + c
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", action="append", default=[],
                    help="a /metrics URL, or LABEL=URL; repeat for several sources")
    ap.add_argument("--k8s", action="store_true",
                    help="discover llm-d model servers and certus servers with kubectl")
    ap.add_argument("--llm-d-namespace", default=os.environ.get("NAMESPACE", "llm-d-quickstart"))
    ap.add_argument("--certus-namespace", default=os.environ.get("CERTUS_NAMESPACE", "certus"))
    ap.add_argument("--interval", type=float, default=10.0, help="seconds between samples")
    ap.add_argument("--out", required=True, help="file to append [prom] lines to")
    ap.add_argument("--timeout", type=float, default=5.0, help="per-scrape HTTP timeout")
    args = ap.parse_args()

    sources = [_parse_url(u) for u in args.url]
    if args.k8s:
        sources += discover_k8s(args.llm_d_namespace, args.certus_namespace)
    if not sources:
        ap.error("give --url (one or more) or --k8s")
    sources = _unique_labels(sources)
    # Per-label keys only when there is more than one source: a single-server
    # run's lines stay exactly as they were.
    split = len(sources) > 1

    signal.signal(signal.SIGTERM, _handle_stop)
    signal.signal(signal.SIGINT, _handle_stop)

    fmt = lambda v: (str(int(v)) if float(v).is_integer() else repr(v))  # noqa: E731

    with open(args.out, "a", encoding="utf-8") as f:
        # Baseline (not emitted): summed deltas then measure the driven window
        # only, excluding whatever the server accrued during warmup/model load.
        prev = [scrape(u, args.timeout) for _, u in sources]
        # Retry the baseline briefly if /metrics wasn't ready yet (empty) — the
        # caller starts us right after readiness, but the connector's counters can
        # register a beat later.
        for _ in range(5):
            if all(prev):
                break
            time.sleep(1.0)
            prev = [p or scrape(u, args.timeout) for p, (_, u) in zip(prev, sources)]
        # Baseline for the latency histograms too, so the final snapshot reflects
        # only the driven window (any warmup requests are subtracted out).
        prev_h = [scrape_histograms(u, args.timeout) for _, u in sources]
        for (label, url), p in zip(sources, prev):
            print(f"[scrape_prom] baseline {label or 'source'} keys={len(p)} url={url}",
                  file=sys.stderr, flush=True)
            if not p:
                print(f"[scrape_prom] WARNING: no baseline from {url}; its first good "
                      f"sample becomes its baseline", file=sys.stderr, flush=True)

        rnd = 0
        # Sleep in short slices so a stop signal is honoured within ~0.2s rather
        # than after a whole interval.
        while True:
            waited = 0.0
            while waited < args.interval and not _STOP:
                time.sleep(0.2)
                waited += 0.2
            total: dict = {}
            labelled: dict = {}
            got = False
            for i, (label, url) in enumerate(sources):
                cur = scrape(url, args.timeout)
                if not cur:
                    continue  # its movement lands on its next good sample
                got = True
                if not prev[i]:
                    prev[i] = cur  # a late baseline: nothing to report yet
                    continue
                for k in cur:
                    # Clamp tiny negative jitter (a counter reset shouldn't happen
                    # on a live server, but a missed label set can look like a dip).
                    d = max(0.0, cur[k] - prev[i].get(k, 0.0))
                    total[k] = total.get(k, 0.0) + d
                    if split and label:
                        labelled[f"{label}.{k}"] = d
                prev[i] = cur
            if got:
                shown = " ".join(f"{k}={fmt(v)}" for k, v in sorted(total.items()))
                if labelled:
                    shown += " " + " ".join(f"{k}={fmt(v)}" for k, v in sorted(labelled.items()))
                f.write(f"[prom] round {rnd}: {shown}\n")
                f.flush()
                rnd += 1
            if _STOP:
                break

        # Latency histograms are cumulative, so emit ONE windowed snapshot at the
        # end (cur − baseline) rather than per round — the same [prom] hist /
        # histjson form the in-process drivers print, so a downstream parser treats
        # serve-mode and in-process runs identically. Summed over sources.
        hists = _hist_merge([_hist_delta(p, scrape_histograms(u, args.timeout))
                             for p, (_, u) in zip(prev_h, sources)])
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
