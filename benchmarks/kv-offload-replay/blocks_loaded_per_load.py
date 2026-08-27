#!/usr/bin/env python3
"""blocks_loaded_per_load.py — blocks loaded per load from an offloading trace.

Reads an ``offloading_trace_<pid>.jsonl`` produced by ``tracing_connector.py``
and reports how many KV blocks were loaded from the external store per load.

WHY NOT ``submit_load`` DIRECTLY: ``submit_load`` is a *worker-internal* call
inside vLLM's ``OffloadingConnector`` — it lives BELOW the connector layer that
``TracingConnector`` wraps, so it never appears in the JSONL (this is the
connector-agnostic tracer's stated granularity: request/allocation level, not
per-worker-submit). The faithful 1:1 proxy is the per-request external load the
connector commits on the scheduler side, which each maps to one worker load:

  * ``--source alloc`` (default): ``num_external_tokens`` from
    ``update_state_after_alloc`` — the tokens actually committed to be loaded for
    a request AFTER block allocation, i.e. what the worker's ``submit_load`` then
    fetches. This is the "blocks loaded" number.
  * ``--source matched``: the first element of ``get_num_new_matched_tokens``'s
    result ``(matched_tokens, ...)`` — how many tokens the store reported it COULD
    provide at lookup time (before the scheduler decides what to allocate). Always
    >= the alloc count; useful to see lookup hits vs. what was loaded.

Tokens are converted to blocks with ``--block-size`` (default 16, vLLM's KV block
size). The block size is auto-detected (GCD of nonzero token counts) and a warning
is printed if it disagrees with ``--block-size``.

Usage:
    python3 blocks_loaded_per_load.py TRACE.jsonl
    python3 blocks_loaded_per_load.py TRACE.jsonl --source matched
    python3 blocks_loaded_per_load.py TRACE.jsonl --per-request        # one line per load
    python3 blocks_loaded_per_load.py TRACE.jsonl --csv loads.csv      # request_id,tokens,blocks
    python3 blocks_loaded_per_load.py TRACE.jsonl --include-zero       # count no-load requests too
"""

import argparse
import json
import math
import re
import sys
from functools import reduce


_MATCHED_RE = re.compile(r"\(\s*(\d+)")


def _load_events(path, source):
    """Yield (request_id, tokens_loaded) for each load event in the trace.

    Only requests with tokens_loaded > 0 are real loads; zero rows are yielded too
    so callers can report the total request population (filter with include_zero).
    """
    want = (
        "update_state_after_alloc" if source == "alloc" else "get_num_new_matched_tokens"
    )
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("method") != want:
                continue
            req_id = (rec.get("summary", {}).get("request") or {}).get("request_id")

            if source == "alloc":
                # update_state_after_alloc(request, blocks, num_external_tokens):
                # request+blocks are summarized, so the lone trailing raw arg is
                # num_external_tokens; fall back to a kwarg if ever passed as one.
                tokens = None
                args = rec.get("args") or []
                if args:
                    try:
                        tokens = int(args[-1])
                    except (TypeError, ValueError):
                        tokens = None
                if tokens is None:
                    kw = rec.get("kwargs") or {}
                    if "num_external_tokens" in kw:
                        try:
                            tokens = int(kw["num_external_tokens"])
                        except (TypeError, ValueError):
                            tokens = None
                if tokens is None:
                    continue
            else:
                # get_num_new_matched_tokens -> "(matched, load_async_bool)"
                m = _MATCHED_RE.match(rec.get("result") or "")
                if not m:
                    continue
                tokens = int(m.group(1))

            yield req_id, tokens


def _gcd_all(values):
    vs = [v for v in values if v > 0]
    if not vs:
        return 0
    return reduce(math.gcd, vs)


def _percentile(sorted_vals, q):
    if not sorted_vals:
        return 0
    idx = min(len(sorted_vals) - 1, int(math.ceil(q * len(sorted_vals)) - 1))
    return sorted_vals[max(0, idx)]


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("trace", help="offloading_trace_<pid>.jsonl path")
    ap.add_argument("--source", choices=["alloc", "matched"], default="alloc",
                    help="alloc = num_external_tokens actually loaded (default); "
                         "matched = tokens the store reported at lookup")
    ap.add_argument("--block-size", type=int, default=16,
                    help="tokens per KV block (default 16)")
    ap.add_argument("--include-zero", action="store_true",
                    help="also count requests that loaded 0 blocks")
    ap.add_argument("--per-request", action="store_true",
                    help="print one 'request_id tokens blocks' line per load")
    ap.add_argument("--csv", metavar="FILE",
                    help="write request_id,tokens,blocks rows to FILE")
    args = ap.parse_args(argv)

    if args.block_size <= 0:
        ap.error("--block-size must be positive")

    events = list(_load_events(args.trace, args.source))
    if not events:
        print(f"no {args.source} records found in {args.trace}", file=sys.stderr)
        return 1

    all_tokens = [t for _, t in events]
    detected = _gcd_all(all_tokens)
    if detected and detected % args.block_size != 0 and args.block_size % detected != 0:
        print(f"warning: token counts look block-aligned to {detected}, not "
              f"--block-size {args.block_size}; blocks may be fractional",
              file=sys.stderr)

    loads = [(rid, t) for rid, t in events if t > 0 or args.include_zero]
    # blocks per load (ceil so a partial trailing block still counts as loaded)
    rows = [(rid, t, math.ceil(t / args.block_size)) for rid, t in loads]

    if args.csv:
        with open(args.csv, "w") as out:
            out.write("request_id,tokens,blocks\n")
            for rid, t, b in rows:
                out.write(f"{rid},{t},{b}\n")
        print(f"wrote {len(rows)} rows -> {args.csv}", file=sys.stderr)

    if args.per_request:
        for rid, t, b in rows:
            print(f"{rid}\t{t}\t{b}")

    blocks = sorted(b for _, _, b in rows)
    nz = [b for b in blocks if b > 0]
    total_reqs = len(events)
    n_loads = len(nz)
    src_label = ("num_external_tokens (loaded)" if args.source == "alloc"
                 else "matched tokens (store lookup)")

    print(f"\n== blocks loaded per load ==", file=sys.stderr)
    print(f"trace       : {args.trace}", file=sys.stderr)
    print(f"source      : {args.source}  [{src_label}]", file=sys.stderr)
    print(f"block_size  : {args.block_size} tokens/block"
          f"  (auto-detected alignment: {detected or 'n/a'})", file=sys.stderr)
    print(f"loads (>0)  : {n_loads}  of {total_reqs} requests"
          f"  ({100.0 * n_loads / total_reqs:.1f}%)", file=sys.stderr)
    if nz:
        print(f"blocks total: {sum(nz)}", file=sys.stderr)
        print(f"blocks/load : min={nz[0]} max={nz[-1]} "
              f"mean={sum(nz) / len(nz):.2f} "
              f"median={_percentile(nz, 0.50)} "
              f"p90={_percentile(nz, 0.90)} p99={_percentile(nz, 0.99)}",
              file=sys.stderr)
        # compact histogram
        hist = {}
        for b in nz:
            hist[b] = hist.get(b, 0) + 1
        print("blocks -> loads:", file=sys.stderr)
        for b in sorted(hist):
            bar = "#" * min(50, hist[b])
            print(f"  {b:>4}: {hist[b]:>6}  {bar}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
