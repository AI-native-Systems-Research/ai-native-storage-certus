#!/usr/bin/env python3
"""The full segment census of a trace's prefix trie — the fitting input for a node-level
branching process.

`trie_profile.py` recovers the trie and reports its shape; this dumps **every** shared
segment so the distributions a generator would draw from can be fitted, rather than the
top twelve by weight.

# Why a segment is the unit

A segment is a maximal chain of trie nodes with out-degree 1 and a constant distinct-session
count: an indivisible run of blocks that the same cohort of sessions walks together. That is
simultaneously

* what the trace's structure is made of — measured, every root preamble in the corpus ends in
  a fanout rather than trailing off, and plateau lengths are per-root and multi-modal
  (`appworld` 23 / 3194 / 5556, `browsecompplus` 1 / 141 / 939);
* what a KV cache sees — a run of blocks sharing one fan-in, which is the quantity an
  eviction hint would carry; and
* what the current schema **cannot** express, because `branching` is a function of depth
  alone and so must fan out at depth 141 for every root or for none.

# What is measured, and the distinction that matters most

Per segment: start depth, length, fan-in (distinct sessions), and how it ENDS —

* `fanout`   — the last node has more than one child. A genuine cohort **split**.
* `sessions` — out-degree 1 but the child has fewer sessions. **Attrition**: sessions ended,
  the trunk did not divide.
* `leaf`     — nothing below.

Keeping those apart is the whole point. A design that reads attrition as splitting sheds
sessions off the trunk at random, and that was measured to make `sharing_depth` three times
worse (2026-08-14) — in the trace the sessions still on a branch at depth are precisely the
ones that go deep, so survival correlates with depth and random shedding destroys that.

**MEASURED, six traces, and it settles the mechanism:** among *shared* segments, `->attrition`
is 0 in almost every band and the leakage at splits — `1 - sum(child fan-in) / fan-in`, the
sessions whose path ENDED at the split — has a median of **exactly 0.000 everywhere**, with a
session-weighted mean of 0.000 on the whole exgentic family and at most 0.060 on `ragbench`.
Sessions do not retire on the shared trunk. They retire in their **private tails**, which are
95%+ of all nodes, so a session's path ends at a node no other session reaches.

So shared width does not fall by refusal (the refuted design) *or* by retirement (my later
description of it, which this corrects). It falls by **exhaustion through subdivision**: a
cohort splits into smaller cohorts until a branch is down to one session, and that session is
then in its private tail. A session leaves the shared region exactly when its own cohort has
subdivided to one — which is a property of how many sessions chose its branch, and therefore
correlated with depth in the way random shedding is not.

A negative weighted leak (`qwen_code`, -0.008 to -0.013) is the opposite case: sessions whose
invocations FORK below a node, so the child fan-ins sum to more than the parent's. Small, real,
and outside FR-019a's one-path-per-session assumption.

At each split we also record the child fan-in vector, from which `n_eff = (Sum c)^2 / Sum c^2`
follows. `n_eff` is the only functional of the child-choice law that `corpus::occupancy`,
validation rule 16 and `branching: auto` depend on, so it is the honest scalar to fit per
segment even where no rank law describes the shape.

Usage:
    /tmp/pqenv/bin/python segment_census.py TRACE_DIR [TRACE_DIR ...] [--block-size N]
        [--json out.json] [--min-sessions 2]
"""

import argparse
import json
import os
import statistics
import sys
from collections import defaultdict

from trie_profile import Trie, build
from width_profile import load_manifest, pick_block_size, read_invocations, reconstruct

# Depth bands. Geometric, because out-degree plainly varies with depth — 4739-way at depth 0
# against 2-way at depth 210 on `qwen_code` — and a linear banding would put all the
# structure in one bucket.
BANDS = [(0, 0), (1, 7), (8, 31), (32, 127), (128, 511), (512, 1 << 30)]


def band_of(depth):
    for lo, hi in BANDS:
        if lo <= depth <= hi:
            return (lo, hi)
    return BANDS[-1]


def band_label(b):
    lo, hi = b
    if lo == hi:
        return str(lo)
    return f"{lo}-{hi}" if hi < (1 << 30) else f"{lo}+"


def census(trie, first, nxt, degree, min_sessions):
    """Every segment, as dicts. One pass, each node in exactly one segment."""
    n = len(trie)
    out = []
    for node in range(n):
        p = trie.parent[node]
        if p >= 0 and degree[p] == 1 and trie.sessions[p] == trie.sessions[node]:
            continue  # continues its parent's segment
        length = 1
        cur = node
        while True:
            if degree[cur] == 0:
                why = "leaf"
                break
            if degree[cur] > 1:
                why = "fanout"
                break
            child = first[cur]
            if trie.sessions[child] != trie.sessions[cur]:
                why = "sessions"
                break
            cur = child
            length += 1
        if trie.sessions[node] < min_sessions:
            continue
        rec = {
            "start_depth": int(trie.depth[node]),
            "length": length,
            "sessions": int(trie.sessions[node]),
            "ends": why,
        }
        if why == "fanout":
            kids = []
            c = first[cur]
            while c >= 0:
                kids.append(int(trie.sessions[c]))
                c = nxt[c]
            kids.sort(reverse=True)
            rec["children"] = kids
            shared = [k for k in kids if k >= min_sessions]
            rec["out_degree"] = len(kids)
            rec["shared_children"] = len(shared)
            # RETIREMENT, measured where it actually appears. A session takes exactly one
            # child at a node, so the children's session sets are disjoint subsets of the
            # parent's and the shortfall is sessions whose path ENDED here. That is why no
            # shared segment ends in `sessions` attrition in practice: retirement is not a
            # separate boundary kind, it is leakage at a split.
            #
            # It is the number this design turns on. A model that reads a fall in width as
            # the trunk refusing sessions sheds them at random, which was measured to make
            # `sharing_depth` three times worse; the trace retires them, and which sessions
            # retire correlates with how deep they were going.
            rec["leak"] = 1.0 - (sum(kids) / rec["sessions"]) if rec["sessions"] else 0.0
            if shared:
                tot = sum(shared)
                sq = sum(k * k for k in shared)
                rec["n_eff"] = (tot * tot) / sq if sq else 1.0
                rec["top_share"] = shared[0] / tot
        out.append(rec)
    return out


def summarise(segs):
    """Per-band aggregates, plus the split/attrition split that the design turns on."""
    by_band = defaultdict(list)
    for s in segs:
        by_band[band_of(s["start_depth"])].append(s)
    rows = []
    for b in BANDS:
        v = by_band.get(b)
        if not v:
            continue
        lengths = [s["length"] for s in v]
        fanin = [s["sessions"] for s in v]
        splits = [s for s in v if s["ends"] == "fanout"]
        attrition = [s for s in v if s["ends"] == "sessions"]
        deg = [s["out_degree"] for s in splits if "out_degree" in s]
        leak = [s["leak"] for s in splits if "leak" in s]
        # Session-weighted too: a split holding 5000 sessions matters more than one holding 2.
        leak_w = sum(s["leak"] * s["sessions"] for s in splits if "leak" in s)
        leak_wd = sum(s["sessions"] for s in splits if "leak" in s)
        neff = [
            (s["n_eff"], s["shared_children"])
            for s in splits
            if s.get("n_eff") and s.get("shared_children", 0) >= 2
        ]
        # Descent-weighted n_eff/n: what uniform descent overstates effective branching by.
        ratio = None
        if neff:
            num = sum(e / k for e, k in neff)
            ratio = num / len(neff)
        rows.append(
            {
                "band": band_label(b),
                "segments": len(v),
                "len_median": statistics.median(lengths),
                "len_p90": sorted(lengths)[min(len(lengths) - 1, int(0.9 * len(lengths)))],
                "len_max": max(lengths),
                "fanin_median": statistics.median(fanin),
                "fanin_max": max(fanin),
                "ends_fanout": len(splits),
                "ends_sessions": len(attrition),
                "ends_leaf": len(v) - len(splits) - len(attrition),
                "deg_median": statistics.median(deg) if deg else None,
                "deg_max": max(deg) if deg else None,
                "leak_median": statistics.median(leak) if leak else None,
                "leak_weighted": (leak_w / leak_wd) if leak_wd else None,
                "n_eff_over_n": ratio,
            }
        )
    return rows


def spearman(xs, ys):
    """Rank correlation, with binned medians reported separately by the caller.

    A low coefficient does NOT mean no relationship — a non-monotonic effect is invisible
    to it, and that exact error has been made on this codebase before. It is reported only
    beside the per-band medians.
    """
    n = len(xs)
    if n < 3:
        return None

    def ranks(v):
        order = sorted(range(n), key=lambda i: v[i])
        r = [0.0] * n
        i = 0
        while i < n:
            j = i
            while j + 1 < n and v[order[j + 1]] == v[order[i]]:
                j += 1
            avg = (i + j) / 2.0 + 1.0
            for k in range(i, j + 1):
                r[order[k]] = avg
            i = j + 1
        return r

    rx, ry = ranks(xs), ranks(ys)
    mx, my = sum(rx) / n, sum(ry) / n
    num = sum((a - mx) * (b - my) for a, b in zip(rx, ry))
    dx = sum((a - mx) ** 2 for a in rx) ** 0.5
    dy = sum((b - my) ** 2 for b in ry) ** 0.5
    return num / (dx * dy) if dx and dy else None


def run(trace_dir, block_size, min_sessions):
    manifest = load_manifest(trace_dir)
    if (manifest.get("field_status") or {}).get("session_id") not in ("native", "reconstructed"):
        return {"trace": os.path.basename(trace_dir), "refused": "no session identity"}
    if manifest.get("id_semantics") != "rolling_prefix":
        return {"trace": os.path.basename(trace_dir), "refused": "not rolling_prefix"}
    bs = pick_block_size(trace_dir, manifest, block_size)
    if not bs:
        return {"trace": os.path.basename(trace_dir), "refused": "metadata_only"}
    rows = read_invocations(trace_dir, bs, 0)
    encoding = (
        "full"
        if (manifest.get("field_status") or {}).get("full_input_blocks") == "native"
        else "delta"
    )
    paths = reconstruct(rows, encoding)
    trie, used, skipped, sessions = build(rows, paths, [r.get("session_id") for r in rows])
    if trie.bad_depth or trie.bad_parent:
        return {
            "trace": os.path.basename(trace_dir),
            "refused": f"rolling-prefix violations: {trie.bad_depth} depth, "
            f"{trie.bad_parent} parent — {trie.bad_example}",
        }
    first, nxt, degree = trie.children_links()
    segs = census(trie, first, nxt, degree, min_sessions)
    return {
        "trace": os.path.basename(trace_dir),
        "block_size": bs,
        "sessions": sessions,
        "invocations": used,
        "nodes": len(trie),
        "shared_segments": len(segs),
        "bands": summarise(segs),
        "len_vs_depth_rho": spearman(
            [s["start_depth"] for s in segs], [s["length"] for s in segs]
        ),
        "deg_vs_depth_rho": spearman(
            [s["start_depth"] for s in segs if "out_degree" in s],
            [s["out_degree"] for s in segs if "out_degree" in s],
        ),
        "segments": segs,
    }


def render(r):
    w = sys.stdout.write
    if "refused" in r:
        w(f"\n{r['trace']}: REFUSED — {r['refused']}\n")
        return
    w(
        f"\n{r['trace']}  {r['invocations']} invocations / {r['sessions']} sessions / "
        f"{r['nodes']} nodes / {r['shared_segments']} shared segments\n"
    )
    w(
        "  band      segs  len_med  len_p90  len_max  fanin_med  fanin_max  "
        "->fanout  ->attr  deg_med  deg_max  n_eff/n  leak_med  leak_wt\n"
    )
    for b in r["bands"]:
        neff = "-" if b["n_eff_over_n"] is None else f"{b['n_eff_over_n']:.3f}"
        dm = "-" if b["deg_median"] is None else f"{b['deg_median']:.0f}"
        dx = "-" if b["deg_max"] is None else str(b["deg_max"])
        lk = "-" if b["leak_median"] is None else f"{b['leak_median']:.3f}"
        lw = "-" if b["leak_weighted"] is None else f"{b['leak_weighted']:.3f}"
        w(
            f"  {b['band']:8}  {b['segments']:5}  {b['len_median']:7.0f}  {b['len_p90']:7}  "
            f"{b['len_max']:7}  {b['fanin_median']:9.0f}  {b['fanin_max']:9}  "
            f"{b['ends_fanout']:8}  {b['ends_sessions']:5}  "
            f"{dm:>7}  {dx:>7}  {neff:>7}  {lk:>8}  {lw:>7}\n"
        )
    lr = r["len_vs_depth_rho"]
    dr = r["deg_vs_depth_rho"]
    w(
        f"  rho(length, depth) = {'n/a' if lr is None else f'{lr:+.3f}'}    "
        f"rho(out_degree, depth) = {'n/a' if dr is None else f'{dr:+.3f}'}"
        "   (read beside the per-band medians, never instead of them)\n"
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("trace_dirs", nargs="+")
    ap.add_argument("--block-size", type=int, default=None)
    ap.add_argument("--min-sessions", type=int, default=2)
    ap.add_argument("--json", default=None)
    args = ap.parse_args()
    out = []
    for d in args.trace_dirs:
        r = run(d, args.block_size, args.min_sessions)
        render(r)
        out.append(r)
    if args.json:
        with open(args.json, "w") as f:
            json.dump(out, f)


if __name__ == "__main__":
    main()
