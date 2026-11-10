#!/usr/bin/env python3
"""Extract the per-conversation *turn-count* distribution from cc-weka traces.

A cc-weka trace is JSON-lines, one conversation per line, in the schema:

    {"id": "...", "models": [...], "block_size": 64, "hash_id_scope": "local",
     "requests": [{"t": 0.0, ...}, {"t": 0.249, ...}, ...]}

A conversation's *turn count* is the number of entries in ``requests`` (one
request = one model round / turn). These counts are heavily right-skewed (a
handful of short chats, a long tail of thousand-turn sessions), so the histogram
is log-spaced by default.

The output YAML matches ``extract_interval_distribution.py`` exactly (metadata,
summary percentiles, bucketed histogram with counts + fractions) so the same
sampler can draw from it — ``kv_cache_dynamics.py`` reads it to pick a turn count
per simulated conversation.

Examples
--------
    # Default: 20 log-spaced buckets, output to turn-distribution.yaml
    ./extract_turn_distribution.py /mnt/certus1/cc-traces-weka-062126.jsonl

    # Custom output file and linear buckets
    ./extract_turn_distribution.py trace.jsonl -o turns.yaml --scale linear
"""

from __future__ import annotations

import argparse
import json
import math
import sys

import yaml


def iter_turn_counts(path):
    """Yield each conversation's turn count (>= 1 request) from a JSONL trace."""
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
            if reqs:
                yield len(reqs)


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
    ap.add_argument("-o", "--output", default="turn-distribution.yaml",
                    help="output YAML file (default: turn-distribution.yaml)")
    ap.add_argument("--buckets", type=int, default=20,
                    help="number of histogram buckets (default: 20)")
    ap.add_argument("--scale", choices=("log", "linear"), default="log",
                    help="bucket spacing (default: log — turn counts are "
                         "heavily right-skewed)")
    args = ap.parse_args()

    if args.buckets < 1:
        ap.error("--buckets must be >= 1")

    turns = [float(t) for t in iter_turn_counts(args.trace)]
    if not turns:
        print("error: no conversations found (need >=1 request per line)",
              file=sys.stderr)
        return 1

    turns.sort()
    n = len(turns)
    vmin, vmax = turns[0], turns[-1]
    total = sum(turns)

    edges = make_edges(vmin, vmax, args.buckets, args.scale)

    # Bucket assignment: [edge[i], edge[i+1]); last bucket is closed on the right.
    counts = [0] * args.buckets
    for v in turns:
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
            "lower": round(edges[i], 6),
            "upper": round(edges[i + 1], 6),
            "count": counts[i],
            "fraction": round(counts[i] / n, 6),
        })

    doc = {
        "source": args.trace,
        "units": "turns",
        "metric": "requests (turns) per conversation",
        "scale": args.scale,
        "num_conversations": n,
        "total_turns": int(total),
        "summary": {
            "min": int(vmin),
            "max": int(vmax),
            "mean": round(total / n, 6),
            "median": round(percentile(turns, 50), 6),
            "p90": round(percentile(turns, 90), 6),
            "p99": round(percentile(turns, 99), 6),
        },
        "buckets": buckets,
    }

    with open(args.output, "w") as f:
        yaml.safe_dump(doc, f, sort_keys=False, default_flow_style=False)

    print(f"wrote {args.output}: {n} conversations, {args.buckets} {args.scale} "
          f"buckets (min={int(vmin)} max={int(vmax)} turns)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
