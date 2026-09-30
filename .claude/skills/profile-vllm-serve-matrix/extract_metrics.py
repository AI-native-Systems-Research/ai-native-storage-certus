#!/usr/bin/env python3
"""Extract (throughput, mean TTFT) from a guidellm JSON result for the serve matrix.

Reads a guidellm >= 0.7 `--output kind=json` file and prints one summary line:

    thr=<total_tok_s> ttft=<ms> reqs=<n>

  throughput  <- metrics.tokens_per_second.successful.mean   (total tok/s: prompt + output)
  mean_ttft   <- metrics.time_to_first_token_ms.successful.mean       (ms)

A guidellm run may hold several benchmark stages (a `sweep` profile emits one per
rate; `concurrent`/`throughput` emit one). We pick the stage with the highest
successful output throughput. Any metric that is missing, or a stage with zero
successful requests, prints `X` for that field so a failed/empty run maps cleanly
to an "X" cell in the matrix. This never raises: a corrupt/absent file -> all X.

Usage: extract_metrics.py <guidellm_result.json>
"""
import json
import sys


def _get(d, *path):
    for k in path:
        if not isinstance(d, dict) or k not in d:
            return None
        d = d[k]
    return d


def main():
    if len(sys.argv) < 2:
        print("thr=X ttft=X reqs=0 err=no-file")
        return
    try:
        with open(sys.argv[1]) as f:
            d = json.load(f)
    except Exception as e:  # noqa: BLE001 - report, never crash the matrix
        print(f"thr=X ttft=X reqs=0 err={type(e).__name__}")
        return

    benches = d.get("benchmarks") or []
    best = None
    best_thr = -1.0
    for b in benches:
        m = b.get("metrics", {})
        thr = _get(m, "tokens_per_second", "successful", "mean")
        n = (
            _get(b, "metrics", "request_totals", "successful")
            or _get(b, "scheduler_state", "successful_requests")
            or 0
        )
        if thr is not None and thr > best_thr:
            best_thr = thr
            best = (thr, m, n)

    if best is None:
        print("thr=X ttft=X reqs=0 err=no-benchmarks")
        return

    thr, m, n = best
    ttft = _get(m, "time_to_first_token_ms", "successful", "mean")
    if not n or thr is None:
        print(f"thr=X ttft=X reqs={n or 0}")
        return

    ts = f"{thr:.2f}" if thr is not None else "X"
    tf = f"{ttft:.1f}" if ttft is not None else "X"
    print(f"thr={ts} ttft={tf} reqs={n}")


if __name__ == "__main__":
    main()
