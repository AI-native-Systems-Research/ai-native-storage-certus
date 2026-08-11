#!/usr/bin/env python3
"""Measure the width-by-depth profile of an LLM trace, for the T074 derivation.

The `branching` parameter of `contracts/workload-schema.md` is fitted from
`w(d) = distinct keys at depth d`. This script produces that profile, plus the two
quantities any honest reading of it needs:

* `survivors(d)` — requests whose block list reaches depth `d`. Observed width is
  bounded above by this, so where it is small the profile says more about how many
  sessions were still running than about how wide the trunk is.
* `sessions(d)` — distinct sessions reaching depth `d`, which is the occupancy
  denominator and the thing FR-055's "trustworthy only where occupancy is high"
  qualification is about.

Measurement only. The segmentation rule itself is derived in `segment.py`, so that
the rule can be re-derived without re-reading 800 MB of parquet.

Usage:
    python3 -m venv /tmp/pqenv && /tmp/pqenv/bin/pip install pyarrow
    /tmp/pqenv/bin/python width_profile.py TRACE_DIR [--block-size N]
        [--max-invocations N] [--out profile.json]

Both block encodings of `contracts/trace-io.md` are handled, and which one is in
use is read from the manifest rather than guessed — a reader assuming one
convention is silently off by one block per request on the other.
"""

import argparse
import glob
import json
import os
import sys
from collections import defaultdict


def load_manifest(trace_dir):
    with open(os.path.join(trace_dir, "manifest.json")) as f:
        return json.load(f)


def pick_block_size(trace_dir, manifest, requested):
    """The block size to read, preferring the smallest for depth resolution.

    Block counts are not comparable across block sizes, so a profile is always
    reported against the size it was measured at. The smallest available gives the
    finest depth axis, which is what a segmentation rule needs.
    """
    available = manifest.get("block_sizes_available") or [manifest.get("block_size")]
    available = [b for b in available if b]
    if requested is not None:
        if requested not in available:
            sys.exit(f"block size {requested} not in {available}")
        return requested
    present = []
    for b in available:
        if glob.glob(os.path.join(trace_dir, f"invocations/block_size_{b}/part-*.parquet")):
            present.append(b)
    if not present:
        sys.exit("no invocation parquet files found")
    return min(present)


def read_invocations(trace_dir, block_size, max_invocations):
    """Rows in file order, as a list of dicts, reading only the needed columns."""
    import pyarrow.parquet as pq

    files = sorted(
        glob.glob(os.path.join(trace_dir, f"invocations/block_size_{block_size}/part-*.parquet"))
    )
    if not files:
        sys.exit(f"no parquet under invocations/block_size_{block_size}/")
    wanted = [
        "session_id",
        "invocation_index",
        "reuse_from",
        "new_input_blocks",
        "new_output_blocks",
        "full_input_blocks",
        "request_start",
    ]
    rows = []
    for f in files:
        schema = pq.ParquetFile(f).schema_arrow
        cols = [c for c in wanted if c in schema.names]
        table = pq.read_table(f, columns=cols)
        rows.extend(table.to_pylist())
        if max_invocations and len(rows) >= max_invocations:
            del rows[max_invocations:]
            break
    return rows


def reconstruct(rows, encoding):
    """Full input block list per row, per `contracts/trace-io.md`.

    Delta: `full_input(n) = concat over a in reuse_from(n) of
    (new_input(a) ++ new_output(a)) ++ new_input(n)`, excluding the trailing
    partial block. Full: `full_input_blocks` is already complete and *includes*
    the trailing partial.
    """
    if encoding == "full":
        return [r.get("full_input_blocks") or [] for r in rows]

    # Delta needs each session's earlier invocations addressable by index.
    by_session = defaultdict(dict)
    for r in rows:
        by_session[r.get("session_id")][r.get("invocation_index")] = r
    out = []
    for r in rows:
        session = by_session[r.get("session_id")]
        blocks = []
        for ancestor_index in r.get("reuse_from") or []:
            a = session.get(ancestor_index)
            if a is None:
                # A truncated read can cut a session's head off. Recorded rather
                # than guessed at: the profile for such a row starts mid-path and
                # would put its blocks at the wrong depths.
                blocks = None
                break
            blocks.extend(a.get("new_input_blocks") or [])
            blocks.extend(a.get("new_output_blocks") or [])
        if blocks is None:
            out.append(None)
            continue
        blocks.extend(r.get("new_input_blocks") or [])
        out.append(blocks)
    return out


def order_rows(rows, manifest):
    """Chronological where timestamps are native, file order otherwise.

    Returned with a flag, because a width profile taken in file order is order
    dependent and `fit` must report it as such rather than as measured.
    """
    status = (manifest.get("field_status") or {}).get("request_start")
    native = status == "native"
    if not native:
        return list(range(len(rows))), False
    starts = [r.get("request_start") for r in rows]
    if any(s is None for s in starts) or len(set(starts)) <= 1:
        return list(range(len(rows))), False
    return sorted(range(len(rows)), key=lambda i: starts[i]), True


def profile(trace_dir, block_size, max_invocations):
    manifest = load_manifest(trace_dir)
    rows = read_invocations(trace_dir, block_size, max_invocations)
    encoding = "full" if (manifest.get("field_status") or {}).get(
        "full_input_blocks"
    ) == "native" else "delta"
    paths = reconstruct(rows, encoding)
    order, chronological = order_rows(rows, manifest)

    # A block's identity is a rolling hash over its prefix chain, so its depth is
    # a property of the block and the per-depth sets partition the distinct ids.
    keys_at_depth = defaultdict(set)
    sessions_at_depth = defaultdict(set)
    survivors = defaultdict(int)
    used = 0
    skipped = 0
    for i in order:
        blocks = paths[i]
        if blocks is None:
            skipped += 1
            continue
        used += 1
        session = rows[i].get("session_id")
        for depth, block in enumerate(blocks):
            keys_at_depth[depth].add(block)
            sessions_at_depth[depth].add(session)
            survivors[depth] += 1

    max_depth = max(keys_at_depth) if keys_at_depth else -1
    return {
        "trace": os.path.basename(os.path.abspath(trace_dir)),
        "block_size": block_size,
        "source_class": manifest.get("source_class"),
        "encoding": encoding,
        "chronological": chronological,
        "invocations_used": used,
        "invocations_skipped": skipped,
        "sessions": len({rows[i].get("session_id") for i in order}),
        "depths": [
            {
                "depth": d,
                "width": len(keys_at_depth[d]),
                "survivors": survivors[d],
                "sessions": len(sessions_at_depth[d]),
            }
            for d in range(max_depth + 1)
        ],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("trace_dir")
    ap.add_argument("--block-size", type=int, default=None)
    ap.add_argument("--max-invocations", type=int, default=0)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    manifest = load_manifest(args.trace_dir)
    block_size = pick_block_size(args.trace_dir, manifest, args.block_size)
    if block_size == 0:
        sys.exit(f"{args.trace_dir}: metadata_only trace, no block data to profile")
    result = profile(args.trace_dir, block_size, args.max_invocations)
    text = json.dumps(result)
    if args.out:
        with open(args.out, "w") as f:
            f.write(text)
        d = result["depths"]
        print(
            f"{result['trace']:26} bs={block_size:<4} depths={len(d):<6} "
            f"invocations={result['invocations_used']:<8} sessions={result['sessions']:<7} "
            f"chronological={result['chronological']}"
        )
    else:
        print(text)


if __name__ == "__main__":
    main()
