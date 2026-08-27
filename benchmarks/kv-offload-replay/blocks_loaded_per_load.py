#!/usr/bin/env python3
"""blocks_loaded_per_load.py — blocks moved per KV transfer from an offload trace.

Reads an ``offloading_trace_<pid>.jsonl`` produced by ``tracing_connector.py``
and reports how many KV blocks were loaded (or stored) per transfer. There are
two families of source, differing in how directly they measure "blocks":

EXACT — scheduler-manager records (``prepare_load`` / ``prepare_store``), the
per-transfer block counts the tracer captures on the scheduler thread. Already
in blocks, so NO token→block conversion is applied:

  * ``--source load``  (recommended): ``load_blocks`` from ``prepare_load`` — the
    KV blocks handed to the worker's ``submit_load`` for a request (1:1 with the
    GPU blocks). This is the exact "blocks loaded per load".
  * ``--source store``: ``store_blocks`` from ``prepare_store`` — the blocks
    actually written per store. Excludes prefix-hit blocks already present in the
    store, which is precisely why it is smaller than ``num_external_tokens ÷ 16``.

PROXY — connector/scheduler-layer token counts, converted to blocks with
``--block-size`` (default 16). Use these on older traces that predate the
scheduler-manager instrumentation:

  * ``--source alloc`` (default, for backward compat): ``num_external_tokens``
    from ``update_state_after_alloc`` — tokens committed to load AFTER block
    allocation. A per-request proxy for what the worker then loads.
  * ``--source matched``: the first element of ``get_num_new_matched_tokens``'s
    result ``(matched_tokens, ...)`` — tokens the store reported it COULD provide
    at lookup time (before the scheduler decides what to allocate). Always >= the
    alloc count.

For the token sources the block size is auto-detected (GCD of nonzero token
counts) and a warning is printed if it disagrees with ``--block-size``; the exact
sources ignore ``--block-size`` for counting (it is used only to derive an
informational token column).

Usage:
    python3 blocks_loaded_per_load.py TRACE.jsonl --source load    # exact per-load
    python3 blocks_loaded_per_load.py TRACE.jsonl --source store   # exact per-store
    python3 blocks_loaded_per_load.py TRACE.jsonl                  # alloc proxy (default)
    python3 blocks_loaded_per_load.py TRACE.jsonl --source matched
    python3 blocks_loaded_per_load.py TRACE.jsonl --per-request    # one line per transfer
    python3 blocks_loaded_per_load.py TRACE.jsonl --csv loads.csv  # request_id,elapsed_s,tokens,blocks
    python3 blocks_loaded_per_load.py TRACE.jsonl --png loads.png  # distribution + over-time figure
    python3 blocks_loaded_per_load.py TRACE.jsonl --include-zero   # count no-transfer requests too
"""

import argparse
import json
import math
import re
import sys
from functools import reduce


_MATCHED_RE = re.compile(r"\(\s*(\d+)")

# Which trace ``method`` each source reads, and how to interpret its value.
_METHOD_FOR_SOURCE = {
    "alloc": "update_state_after_alloc",
    "matched": "get_num_new_matched_tokens",
    "load": "prepare_load",
    "store": "prepare_store",
}
# The two EXACT sources already report block counts (no ÷ block_size).
_BLOCK_SOURCES = {"load", "store"}
# Human labels for the value each source carries.
_SRC_LABEL = {
    "alloc": "num_external_tokens (loaded)",
    "matched": "matched tokens (store lookup)",
    "load": "prepare_load load_blocks (exact per-load)",
    "store": "prepare_store store_blocks (exact per-store)",
}
# Noun for the transfer a source counts, for report/plot wording.
_SRC_NOUN = {"alloc": "load", "matched": "load", "load": "load", "store": "store"}
# Past-tense verb for each noun ("loaded" / "stored"), so wording isn't "storeed".
_PAST = {"load": "loaded", "store": "stored"}


def _load_events(path, source):
    """Yield (request_id, ts, value) for each transfer event in the trace.

    ``value`` is a BLOCK count for the exact sources (load/store) and a TOKEN
    count for the proxy sources (alloc/matched). ``ts`` is the record's raw
    perf-counter timestamp (seconds); callers rebase it to run-relative. Zero
    rows are yielded too so callers can report the total population (filter with
    include_zero)."""
    want = _METHOD_FOR_SOURCE[source]
    is_block = source in _BLOCK_SOURCES
    block_field = "load_blocks" if source == "load" else "store_blocks"
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

            if is_block:
                # Scheduler-manager records: req_id is a top-level field and the
                # block count is already the value we want.
                req_id = rec.get("req_id")
                ts = rec.get("ts")
                val = rec.get(block_field)
                if val is None:
                    continue
                try:
                    val = int(val)
                except (TypeError, ValueError):
                    continue
                yield req_id, ts, val
                continue

            # Proxy (token) sources: req_id lives in the summarized request.
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


def _render_png(path, rows, src_label, noun, block_size, bin_seconds, exact):
    """Two panels: (left) distribution of blocks/transfer, (right) blocks/transfer
    over time — per-transfer points plus a binned-mean trend on one shared blocks
    axis. ``exact`` selects the block-vs-token axis wording."""
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    nz = [(el, b) for _, el, _, b in rows if b > 0]
    timed = [(el, b) for el, b in nz if el is not None]
    if not nz:
        raise SystemExit("nothing to plot: no transfers with blocks > 0")

    blocks = [b for _, b in nz]

    # binned mean of blocks/transfer over time
    bins = {}
    for el, b in timed:
        k = int(el // bin_seconds)
        bins.setdefault(k, []).append(b)
    bin_centers = [(k + 0.5) * bin_seconds for k in sorted(bins)]
    bin_means = [sum(bins[k]) / len(bins[k]) for k in sorted(bins)]

    # "16 tok each" only makes sense for the token-derived proxy sources; the
    # exact sources are counted directly in blocks.
    per_block = "" if exact else f" ({block_size} tok each)"
    past = _PAST[noun]
    blocks_axis = f"KV-cache blocks {past} per {noun}{per_block}"

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
    axd.set_xlabel(blocks_axis)
    axd.set_ylabel(f"number of {noun}s")
    axd.set_title(f"Distribution  (n={len(blocks)} {noun}s)")
    axd.legend(frameon=False)

    # ── over time ──
    if timed:
        xs = [el for el, _ in timed]
        ys = [b for _, b in timed]
        axt.scatter(xs, ys, s=6, color=C_PT, alpha=0.25, linewidths=0,
                    label=f"per {noun}")
        axt.plot(bin_centers, bin_means, color=C_LINE, linewidth=2,
                 label=f"mean / {bin_seconds:.0f}s")
        axt.set_xlim(left=0)
        axt.legend(frameon=False)
    else:
        axt.text(0.5, 0.5, "no timestamps in trace", ha="center", va="center",
                 transform=axt.transAxes)
    axt.set_ylim(bottom=0)
    axt.set_xlabel("elapsed (s)")
    axt.set_ylabel(blocks_axis)
    axt.set_title("Over time")

    fig.suptitle(f"KV-cache blocks {past} per {noun}  ({src_label})",
                 fontweight="bold")
    fig.tight_layout(rect=(0, 0, 1, 0.97))
    fig.savefig(path, dpi=130)
    plt.close(fig)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("trace", help="offloading_trace_<pid>.jsonl path")
    ap.add_argument("--source", choices=["alloc", "matched", "load", "store"],
                    default="alloc",
                    help="load = exact prepare_load load_blocks (recommended); "
                         "store = exact prepare_store store_blocks; "
                         "alloc = num_external_tokens proxy (default); "
                         "matched = tokens the store reported at lookup")
    ap.add_argument("--block-size", type=int, default=16,
                    help="tokens per KV block (default 16); token sources only")
    ap.add_argument("--include-zero", action="store_true",
                    help="also count requests that moved 0 blocks")
    ap.add_argument("--per-request", action="store_true",
                    help="print one 'request_id tokens blocks' line per transfer")
    ap.add_argument("--csv", metavar="FILE",
                    help="write request_id,elapsed_s,tokens,blocks rows to FILE")
    ap.add_argument("--png", metavar="FILE",
                    help="render a 2-panel figure (distribution + over time) to FILE")
    ap.add_argument("--bin-seconds", type=float, default=30.0,
                    help="time-bin width for the over-time trend line (default 30s)")
    args = ap.parse_args(argv)

    if args.block_size <= 0:
        ap.error("--block-size must be positive")

    exact = args.source in _BLOCK_SOURCES
    noun = _SRC_NOUN[args.source]

    events = list(_load_events(args.trace, args.source))
    if not events:
        print(f"no {args.source} records found in {args.trace}"
              + ("" if not exact else " — was the trace produced by a "
                 "TracingConnector new enough to instrument the scheduler "
                 "manager? (older traces have only alloc/matched)"),
              file=sys.stderr)
        return 1

    all_vals = [v for _, _, v in events]
    detected = _gcd_all(all_vals)
    if not exact and detected and detected % args.block_size != 0 \
            and args.block_size % detected != 0:
        print(f"warning: token counts look block-aligned to {detected}, not "
              f"--block-size {args.block_size}; blocks may be fractional",
              file=sys.stderr)

    # run-relative time origin from the first event with a timestamp
    ts0 = next((ts for _, ts, _ in events if ts is not None), 0.0)

    kept = [(rid, ts, v) for rid, ts, v in events if v > 0 or args.include_zero]
    # (request_id, elapsed_s, tokens, blocks). Exact sources ARE blocks (derive an
    # informational token column); token sources convert with ceil so a partial
    # trailing block counts.
    if exact:
        rows = [(rid, (ts - ts0) if ts is not None else None,
                 v * args.block_size, v) for rid, ts, v in kept]
    else:
        rows = [(rid, (ts - ts0) if ts is not None else None, v,
                 math.ceil(v / args.block_size)) for rid, ts, v in kept]

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
        _render_png(args.png, rows, _SRC_LABEL[args.source], noun,
                    args.block_size, args.bin_seconds, exact)
        print(f"wrote figure -> {args.png}", file=sys.stderr)

    blocks = sorted(b for _, _, _, b in rows)
    nz = [b for b in blocks if b > 0]
    total_reqs = len(events)
    n_xfers = len(nz)

    print(f"\n== blocks {_PAST[noun]} per {noun} ==", file=sys.stderr)
    print(f"trace       : {args.trace}", file=sys.stderr)
    print(f"source      : {args.source}  [{_SRC_LABEL[args.source]}]"
          f"{'  (exact block counts)' if exact else ''}", file=sys.stderr)
    if exact:
        print(f"block_size  : {args.block_size} tokens/block  (for the derived "
              f"token column only; counts are exact)", file=sys.stderr)
    else:
        print(f"block_size  : {args.block_size} tokens/block"
              f"  (auto-detected alignment: {detected or 'n/a'})", file=sys.stderr)
    print(f"{noun}s (>0)  : {n_xfers}  of {total_reqs} requests"
          f"  ({100.0 * n_xfers / total_reqs:.1f}%)", file=sys.stderr)
    if nz:
        print(f"blocks total: {sum(nz)}", file=sys.stderr)
        print(f"blocks/{noun} : min={nz[0]} max={nz[-1]} "
              f"mean={sum(nz) / len(nz):.2f} "
              f"median={_percentile(nz, 0.50)} "
              f"p90={_percentile(nz, 0.90)} p99={_percentile(nz, 0.99)}",
              file=sys.stderr)
        # compact histogram
        hist = {}
        for b in nz:
            hist[b] = hist.get(b, 0) + 1
        print(f"blocks -> {noun}s:", file=sys.stderr)
        for b in sorted(hist):
            bar = "#" * min(50, hist[b])
            print(f"  {b:>4}: {hist[b]:>6}  {bar}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
