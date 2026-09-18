#!/usr/bin/env python3
"""Extract the request inter-arrival *interval* distribution from cc-weka traces.

A cc-weka trace is JSON-lines, one conversation per line, in the schema:

    {"id": "...", "models": [...], "block_size": 64, "hash_id_scope": "local",
     "requests": [{"t": 0.0, "model": "...", "in": 640, "out": 20, ...},
                  {"t": 0.249, ...}, ...]}

Within a conversation the requests are ordered by ``t`` (seconds from the
conversation's first request). The *interval* is the think-time gap between two
consecutive requests, i.e. ``t[i+1] - t[i]``. These gaps span many orders of
magnitude (sub-second to thousands of seconds), so the histogram is
log-spaced by default.

The result is written as YAML: metadata, summary percentiles, and the bucketed
histogram (counts + fractions), suitable for driving synthetic trace replay.

Examples
--------
    # Default: 20 log-spaced buckets, output to interval-distribution.yaml
    ./extract_interval_distribution.py /mnt/certus1/cc-traces-weka-062126.jsonl

    # Custom output file and linear buckets
    ./extract_interval_distribution.py trace.jsonl -o dist.yaml --scale linear
"""

from __future__ import annotations

import argparse
import json
import math
import sys

import yaml


def iter_intervals(path):
    """Yield each intra-conversation interval (seconds) from a JSONL trace.

    Also returns per-line/per-request accounting via the closure counters the
    caller passes in; here we just yield the positive gaps.
    """
    with open(path, errors="replace") as f:
        for lineno, raw in enumerate(f, 1):
            raw = raw.strip()
            if not raw:
                continue
            try:
                conv = json.loads(raw)
            except json.JSONDecodeError as e:
                print(f"warning: skipping malformed line {lineno}: {e}",
                      file=sys.stderr)
                continue
            reqs = conv.get("requests") or []
            ts = [r["t"] for r in reqs if "t" in r]
            ts.sort()
            for i in range(len(ts) - 1):
                yield ts[i + 1] - ts[i]


def percentile(sorted_vals, q):
    """Linear-interpolated percentile q in [0,100] over a sorted list."""
    if not sorted_vals:
        return float("nan")
    if len(sorted_vals) == 1:
        return sorted_vals[0]
    pos = (q / 100.0) * (len(sorted_vals) - 1)
    lo = math.floor(pos)
    hi = math.ceil(pos)
    if lo == hi:
        return sorted_vals[lo]
    frac = pos - lo
    return sorted_vals[lo] * (1 - frac) + sorted_vals[hi] * frac


def make_edges(vmin, vmax, buckets, scale):
    """Return ``buckets + 1`` monotonically increasing bin edges."""
    if scale == "log":
        # Guard against non-positive minima; a floor keeps log() finite.
        lo = max(vmin, 1e-6)
        hi = max(vmax, lo * (1 + 1e-9))
        lo_e, hi_e = math.log10(lo), math.log10(hi)
        return [10 ** (lo_e + (hi_e - lo_e) * i / buckets)
                for i in range(buckets + 1)]
    # linear
    hi = vmax if vmax > vmin else vmin + 1e-9
    return [vmin + (hi - vmin) * i / buckets for i in range(buckets + 1)]


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("trace", help="path to the cc-weka JSONL trace")
    ap.add_argument("-o", "--output", default="interval-distribution.yaml",
                    help="output YAML file (default: interval-distribution.yaml)")
    ap.add_argument("--buckets", type=int, default=20,
                    help="number of histogram buckets (default: 20)")
    ap.add_argument("--scale", choices=("log", "linear"), default="log",
                    help="bucket spacing (default: log — intervals span orders "
                         "of magnitude)")
    args = ap.parse_args()

    if args.buckets < 1:
        ap.error("--buckets must be >= 1")

    # First pass: collect intervals. Traces can be large, but the interval count
    # is bounded by total requests and fits comfortably in memory as floats.
    intervals = list(iter_intervals(args.trace))
    if not intervals:
        print("error: no intervals found (need >=2 requests in a conversation)",
              file=sys.stderr)
        return 1

    intervals.sort()
    n = len(intervals)
    vmin, vmax = intervals[0], intervals[-1]
    total = sum(intervals)

    edges = make_edges(vmin, vmax, args.buckets, args.scale)

    # Bucket assignment: [edge[i], edge[i+1]); last bucket is closed on the right.
    counts = [0] * args.buckets
    for v in intervals:
        placed = False
        for i in range(args.buckets):
            upper = edges[i + 1]
            if v < upper or (i == args.buckets - 1 and v <= upper):
                counts[i] += 1
                placed = True
                break
        if not placed:  # numerical edge case: value at/above top edge
            counts[-1] += 1

    buckets = []
    for i in range(args.buckets):
        buckets.append({
            "index": i,
            "lower_s": round(edges[i], 6),
            "upper_s": round(edges[i + 1], 6),
            "count": counts[i],
            "fraction": round(counts[i] / n, 6),
        })

    doc = {
        "source": args.trace,
        "units": "seconds",
        "metric": "intra-conversation request inter-arrival interval",
        "scale": args.scale,
        "num_intervals": n,
        "summary": {
            "min_s": round(vmin, 6),
            "max_s": round(vmax, 6),
            "mean_s": round(total / n, 6),
            "median_s": round(percentile(intervals, 50), 6),
            "p90_s": round(percentile(intervals, 90), 6),
            "p99_s": round(percentile(intervals, 99), 6),
        },
        "buckets": buckets,
    }

    with open(args.output, "w") as f:
        yaml.safe_dump(doc, f, sort_keys=False, default_flow_style=False)

    print(f"wrote {args.output}: {n} intervals, {args.buckets} {args.scale} "
          f"buckets (min={vmin:.3f}s max={vmax:.3f}s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
