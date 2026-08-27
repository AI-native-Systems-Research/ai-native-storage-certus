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
    python3 blocks_loaded_per_load.py TRACE.jsonl --csv loads.csv      # request_id,elapsed_s,tokens,blocks
    python3 blocks_loaded_per_load.py TRACE.jsonl --png loads.png      # distribution + over-time figure
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
    """Yield (request_id, ts, tokens_loaded) for each load event in the trace.

    ``ts`` is the record's raw perf-counter timestamp (seconds); callers rebase it
    to run-relative. Only requests with tokens_loaded > 0 are real loads; zero rows
    are yielded too so callers can report the total request population (filter with
    include_zero).
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
            ts = rec.get("ts")

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

            yield req_id, ts, tokens


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


def _render_png(path, rows, source, block_size, bin_seconds):
    """Two panels: (left) distribution of blocks/load, (right) blocks/load over
    time — per-load points plus a binned-mean trend on one shared blocks axis."""
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    nz = [(el, b) for _, el, _, b in rows if b > 0]
    timed = [(el, b) for el, b in nz if el is not None]
    if not nz:
        raise SystemExit("nothing to plot: no loads with blocks > 0")

    blocks = [b for _, b in nz]
    # Name the raw source field AND the conversion, so the axis ("blocks") is never
    # mistaken for the token field it is derived from.
    field = "num_external_tokens" if source == "alloc" else "get_num_new_matched_tokens"
    src_label = f"{field} ÷ {block_size} tok/block"

    # binned mean of blocks/load over time
    bins = {}
    for el, b in timed:
        k = int(el // bin_seconds)
        bins.setdefault(k, []).append(b)
    bin_centers = [(k + 0.5) * bin_seconds for k in sorted(bins)]
    bin_means = [sum(bins[k]) / len(bins[k]) for k in sorted(bins)]

    C_PT, C_LINE = "#4C78A8", "#E4572E"  # blue points, warm trend line
    plt.rcParams.update({"axes.grid": True, "grid.alpha": 0.25,
                         "axes.axisbelow": True, "font.size": 10})
    fig, (axd, axt) = plt.subplots(1, 2, figsize=(13, 5))

    # ── distribution ──
    hi = max(blocks)
    axd.hist(blocks, bins=range(1, hi + 2), color=C_PT, edgecolor="white", linewidth=0.3)
    mean_b = sum(blocks) / len(blocks)
    axd.axvline(mean_b, color=C_LINE, linewidth=2,
                label=f"mean {mean_b:.1f}")
    axd.set_xlabel(f"KV-cache blocks loaded per load ({block_size} tok each)")
    axd.set_ylabel("number of loads")
    axd.set_title(f"Distribution  (n={len(blocks)} loads)")
    axd.legend(frameon=False)

    # ── over time ──
    if timed:
        xs = [el for el, _ in timed]
        ys = [b for _, b in timed]
        axt.scatter(xs, ys, s=6, color=C_PT, alpha=0.25, linewidths=0,
                    label="per load")
        axt.plot(bin_centers, bin_means, color=C_LINE, linewidth=2,
                 label=f"mean / {bin_seconds:.0f}s")
        axt.set_xlim(left=0)
        axt.legend(frameon=False)
    else:
        axt.text(0.5, 0.5, "no timestamps in trace", ha="center", va="center",
                 transform=axt.transAxes)
    axt.set_ylim(bottom=0)
    axt.set_xlabel("elapsed (s)")
    axt.set_ylabel(f"KV-cache blocks loaded per load ({block_size} tok each)")
    axt.set_title("Over time")

    fig.suptitle(f"KV-cache blocks loaded per load  ({src_label})", fontweight="bold")
    fig.tight_layout(rect=(0, 0, 1, 0.97))
    fig.savefig(path, dpi=130)
    plt.close(fig)


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
    ap.add_argument("--png", metavar="FILE",
                    help="render a 2-panel figure (distribution + over time) to FILE")
    ap.add_argument("--bin-seconds", type=float, default=30.0,
                    help="time-bin width for the over-time trend line (default 30s)")
    args = ap.parse_args(argv)

    if args.block_size <= 0:
        ap.error("--block-size must be positive")

    events = list(_load_events(args.trace, args.source))
    if not events:
        print(f"no {args.source} records found in {args.trace}", file=sys.stderr)
        return 1

    all_tokens = [t for _, _, t in events]
    detected = _gcd_all(all_tokens)
    if detected and detected % args.block_size != 0 and args.block_size % detected != 0:
        print(f"warning: token counts look block-aligned to {detected}, not "
              f"--block-size {args.block_size}; blocks may be fractional",
              file=sys.stderr)

    # run-relative time origin from the first event with a timestamp
    ts0 = next((ts for _, ts, _ in events if ts is not None), 0.0)

    loads = [(rid, ts, t) for rid, ts, t in events if t > 0 or args.include_zero]
    # (request_id, elapsed_s, tokens, blocks); ceil so a partial trailing block counts
    rows = [(rid, (ts - ts0) if ts is not None else None, t,
             math.ceil(t / args.block_size)) for rid, ts, t in loads]

    if args.csv:
        with open(args.csv, "w") as out:
            out.write("request_id,elapsed_s,tokens,blocks\n")
            for rid, el, t, b in rows:
                out.write(f"{rid},{'' if el is None else f'{el:.3f}'},{t},{b}\n")
        print(f"wrote {len(rows)} rows -> {args.csv}", file=sys.stderr)

    if args.per_request:
        for rid, el, t, b in rows:
            print(f"{rid}\t{'' if el is None else f'{el:.1f}'}\t{t}\t{b}")

    if args.png:
        _render_png(args.png, rows, args.source, args.block_size, args.bin_seconds)
        print(f"wrote figure -> {args.png}", file=sys.stderr)

    blocks = sorted(b for _, _, _, b in rows)
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
