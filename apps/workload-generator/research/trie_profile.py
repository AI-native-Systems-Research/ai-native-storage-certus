#!/usr/bin/env python3
"""Recover the prefix TRIE of an LLM trace: global prefixes, shared subtrees, private chains.

`width_profile.py` measures `w(d)` — distinct keys at each depth — and every trunk
statistic in `stats/trunk.rs` is pooled the same way, per depth. That pooling is what
makes the load-bearing structure invisible: a global preamble every session shares,
then a fanout into per-branch commonality (a tool definition, a retrieved document),
then private tails, all arrive as one scalar width per depth with a lower-bound
occupancy. This script measures the structure itself.

# Why the structure is exact rather than inferred

The corpus declares `id_semantics: rolling_prefix`, so a block id is a hash over the
whole prefix chain ending at it. Two requests carrying the same id at depth `d`
therefore have *identical* paths `[0..d]`, which makes

    parent(path[d]) = path[d-1]

well defined and consistent across every request and session. The set of paths is a
trie, and recovering it needs no thresholds and no clustering. It also *checks* the
manifest's declaration for free: a key seen at two depths, or under two parents,
means the ids are not rolling-prefix, which is a trace contradicting its own manifest.
That is reported, never repaired.

# The three quantities, and one rule about counting

Per node: `depth`, `parent`, out-degree, and `sessions` — **distinct sessions**, never
references. A session re-walks its whole path on every turn, so counting references
would measure conversation length; `stats/trunk.rs` and `KeyTable` already follow this
rule and this script matches them.

Then the decomposition is definitional:

* **root** — a node at depth 0.
* **segment** — a maximal chain in which each node has exactly one child *and* the
  child is reached by the same session set size. Both conditions matter: out-degree
  above one is a fanout, and a drop in `sessions` along a unary chain is sessions
  ending, which is a different event and is reported as such.
* **global prefix** — the segment that starts at a root, when its `sessions` is the
  root's whole population. Its length is how much preamble every session on that root
  shares.
* **shared subtree** — the induced subtrie on `sessions >= 2`. A segment inside it and
  below a branch point is one indivisible unit of shared content.
* **private chain** — the maximal suffix with `sessions == 1`. Its frontier per request
  is the realised LCP that `stats::sharing::last_prefix_len` already measures.

# What the numbers here can and cannot say

Session counts at depth are **lower bounds** under right censoring: a node whose
sessions are still live can only gain occupancy. Nothing here can see a tool loaded
*after* two sessions diverged — under rolling-prefix hashing that content has
different ids in each session, so it is not shared, and it would not be shared in a KV
cache either. And a trace with no block data (`metadata_only`) or no session identity
cannot be profiled at all; both refuse rather than reporting a shape.

Usage:
    python3 -m venv /tmp/pqenv && /tmp/pqenv/bin/pip install pyarrow
    /tmp/pqenv/bin/python trie_profile.py TRACE_DIR [--block-size N]
        [--max-invocations N] [--top N] [--print-depth N] [--json out.json]
"""

import argparse
import json
import os
import sys
from array import array
from collections import defaultdict

from width_profile import load_manifest, pick_block_size, read_invocations, reconstruct


class Trie:
    """The prefix trie, in parallel arrays indexed by a dense node id.

    Arrays rather than objects because a real trace has millions of distinct keys:
    `qwen_code` at block size 16 has 5.2M, and one Python object per node costs two
    orders of magnitude more than the four integers a node actually needs.
    """

    def __init__(self):
        self.index = {}          # block id -> node
        self.depth = array("i")
        self.parent = array("i")  # -1 at a root
        self.sessions = array("i")
        self.last_session = array("i")
        self.bad_depth = 0
        self.bad_parent = 0
        self.bad_example = ""

    def __len__(self):
        return len(self.depth)

    def add(self, block, depth, parent, session):
        node = self.index.get(block)
        if node is None:
            node = len(self.depth)
            self.index[block] = node
            self.depth.append(depth)
            self.parent.append(parent)
            self.sessions.append(1)
            self.last_session.append(session)
            return node
        # The rolling-prefix declaration, checked rather than trusted.
        if self.depth[node] != depth:
            self.bad_depth += 1
            if not self.bad_example:
                self.bad_example = (
                    f"block {block} seen at depth {self.depth[node]} and {depth}"
                )
        elif self.parent[node] != parent:
            self.bad_parent += 1
            if not self.bad_example:
                self.bad_example = (
                    f"block {block} at depth {depth} seen under parents "
                    f"{self.parent[node]} and {parent}"
                )
        # Distinct sessions, so a session re-walking its own path counts once. A
        # single last-seen marker is exact because each session's references to a
        # node are handed over contiguously (all of one session's paths at once).
        if self.last_session[node] != session:
            self.last_session[node] = session
            self.sessions[node] += 1
        return node

    def children_links(self):
        """Child lists as a first-child / next-sibling pair, plus out-degrees."""
        n = len(self.depth)
        first = array("i", bytes(4 * n))
        nxt = array("i", bytes(4 * n))
        for i in range(n):
            first[i] = -1
            nxt[i] = -1
        degree = array("i", bytes(4 * n))
        # Reverse order so each child list comes out in ascending node id, which is
        # first-seen order and makes the printed tree stable across runs.
        for node in range(n - 1, -1, -1):
            p = self.parent[node]
            if p >= 0:
                nxt[node] = first[p]
                first[p] = node
                degree[p] += 1
        return first, nxt, degree


def build(rows, paths, sessions_of):
    """One node per distinct block, sessions counted per session not per reference.

    Rows are grouped by session first, so `Trie.add`'s last-seen marker is an exact
    distinct-session count and no per-node set is needed.
    """
    by_session = defaultdict(list)
    skipped = 0
    for i, blocks in enumerate(paths):
        # A truncated read cuts a session's head off, so the row's blocks would sit
        # at the wrong depths and under the wrong parents. Dropped, not misplaced —
        # the same rule `read::normalise` follows.
        if blocks is None:
            skipped += 1
            continue
        by_session[sessions_of[i]].append(i)

    trie = Trie()
    used = 0
    for sid, (session, indices) in enumerate(sorted(by_session.items(), key=lambda kv: str(kv[0]))):
        for i in indices:
            blocks = paths[i]
            if not blocks:
                skipped += 1
                continue
            used += 1
            parent = -1
            for depth, block in enumerate(blocks):
                parent = trie.add(block, depth, parent, sid)
    return trie, used, skipped, len(by_session)


def segments(trie, first, degree):
    """Every maximal unary constant-session chain, as (head, length, sessions, why).

    `why` is what ended the segment — `fanout`, `sessions` (some ended), or `leaf` —
    because the three are different events and a segmentation that conflates them
    would read a retiring population as a narrowing trunk.
    """
    n = len(trie)
    out = []
    head_of = array("i", bytes(4 * n))
    for i in range(n):
        head_of[i] = -1
    for node in range(n):
        p = trie.parent[node]
        if p >= 0 and degree[p] == 1 and trie.sessions[p] == trie.sessions[node]:
            continue  # not a head: it continues its parent's segment
        # Walk the chain down from this head.
        length = 1
        cur = node
        head_of[node] = node
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
            head_of[cur] = node
            length += 1
        out.append((node, length, trie.sessions[node], why, cur))
    return out, head_of


def descend(trie, first, nxt, degree, root, max_depth, top, min_sessions, frac, budget):
    """The load-bearing spine below `root`, unary chains collapsed, as printable rows.

    Pruned two ways, because an unpruned descent is unreadable and says nothing: a
    trace of 26k sessions has hundreds of thousands of two-session branches, and
    printing them buries the structure that carries the references. A child is
    followed only if it holds at least `frac` of the root's sessions, and what was
    dropped is counted on the parent's row rather than silently omitted.
    """
    floor = max(min_sessions, int(frac * trie.sessions[root]))
    rows = []
    stack = [(root, 0)]
    while stack and len(rows) < budget:
        node, indent = stack.pop()
        # Collapse the unary constant-session chain starting here.
        length = 1
        cur = node
        while degree[cur] == 1 and trie.sessions[first[cur]] == trie.sessions[cur]:
            cur = first[cur]
            length += 1
        kids = []
        c = first[cur]
        while c >= 0:
            kids.append(c)
            c = nxt[c]
        shared = [k for k in kids if trie.sessions[k] >= min_sessions]
        shared.sort(key=lambda k: trie.sessions[k], reverse=True)
        followed = [k for k in shared if trie.sessions[k] >= floor][:top]
        rows.append(
            {
                "indent": indent,
                "start_depth": trie.depth[node],
                "end_depth": trie.depth[cur],
                "length": length,
                "sessions": trie.sessions[node],
                "children": len(kids),
                "shared_children": len(shared),
                "followed": len(followed),
                "child_sessions": [trie.sessions[k] for k in shared[:top]],
            }
        )
        if trie.depth[cur] >= max_depth:
            continue
        for k in reversed(followed):
            stack.append((k, indent + 1))
    return rows


def report(trie, used, skipped, session_count, meta, top, print_depth, min_sessions,
           spine_roots, spine_frac, spine_budget):
    first, nxt, degree = trie.children_links()
    n = len(trie)
    shared = sum(1 for i in range(n) if trie.sessions[i] >= 2)
    roots = [i for i in range(n) if trie.parent[i] < 0]
    roots.sort(key=lambda i: trie.sessions[i], reverse=True)

    segs, head_of = segments(trie, first, degree)
    shared_segs = [s for s in segs if s[2] >= min_sessions]
    seg_of_head = {s[0]: s for s in segs}

    # Per-depth occupancy two ways. The pooled figure is what `stats/trunk.rs`
    # publishes and what `fit::branching`'s near-root fold judges; the shared-only
    # figure is the same quantity with private descents left out of the denominator,
    # which is exactly the lower-bound bias that module documents.
    pooled = defaultdict(lambda: [0, 0, 0])
    sharedonly = defaultdict(lambda: [0, 0])
    for i in range(n):
        d = trie.depth[i]
        s = trie.sessions[i]
        pooled[d][0] += s
        pooled[d][1] += 1
        # The most-occupied single node at this depth. The model's trunk has no
        # popularity skew (`branch_skew` is never fitted), so what it can realise is
        # the *mean*; this is what the trace actually concentrates on one key.
        if s > pooled[d][2]:
            pooled[d][2] = s
        if s >= 2:
            sharedonly[d][0] += s
            sharedonly[d][1] += 1

    out = {
        "trace": meta["trace"],
        "block_size": meta["block_size"],
        "encoding": meta["encoding"],
        "invocations_used": used,
        "invocations_skipped": skipped,
        "sessions": session_count,
        "nodes": n,
        "shared_nodes": shared,
        "private_nodes": n - shared,
        "roots": len(roots),
        "rolling_prefix_violations": {
            "depth": trie.bad_depth,
            "parent": trie.bad_parent,
            "example": trie.bad_example,
        },
        "root_table": [
            {
                "root": r,
                "sessions": trie.sessions[r],
                "segment_length": seg_of_head[r][1],
                "segment_ends": seg_of_head[r][3],
                "branch_depth": trie.depth[seg_of_head[r][4]],
                "children_at_branch": degree[seg_of_head[r][4]],
            }
            for r in roots[:top]
        ],
        "segments": {
            "total": len(segs),
            "shared": len(shared_segs),
            "top_by_value": [
                {
                    "start_depth": trie.depth[s[0]],
                    "length": s[1],
                    "sessions": s[2],
                    "ends": s[3],
                    "node_sessions": s[1] * s[2],
                }
                for s in sorted(shared_segs, key=lambda s: s[1] * s[2], reverse=True)[:top]
            ],
        },
        "spines": [
            {
                "root": r,
                "root_sessions": trie.sessions[r],
                "rows": descend(
                    trie, first, nxt, degree, r, print_depth, top, min_sessions,
                    spine_frac, spine_budget,
                ),
            }
            for r in roots[:spine_roots]
        ],
        # Sessions per root, so the concentration is visible rather than averaged.
        # Capped: a trace whose sessions never share a root has one root each, and
        # printing 826k of them says nothing that the tail sum does not.
        "root_sessions": [trie.sessions[r] for r in roots[:4096]],
        "root_sessions_tail": sum(trie.sessions[r] for r in roots[4096:]),
        "occupancy_by_depth": [
            {
                "depth": d,
                "width": pooled[d][1],
                "pooled": pooled[d][0] / pooled[d][1],
                "max_sessions": pooled[d][2],
                "shared_width": sharedonly[d][1],
                "shared_only": (sharedonly[d][0] / sharedonly[d][1]) if sharedonly[d][1] else None,
            }
            for d in sorted(pooled)
        ],
    }
    return out


def render(r):
    w = sys.stdout.write
    w(f"trie  {r['trace']}  block size {r['block_size']}, {r['encoding']} encoding\n")
    w(
        f"  {r['invocations_used']} invocations over {r['sessions']} sessions"
        f" ({r['invocations_skipped']} skipped), {r['nodes']} distinct keys\n"
    )
    w(
        f"  {r['shared_nodes']} shared nodes (>=2 sessions), {r['private_nodes']} private"
        f" ({100.0 * r['private_nodes'] / max(1, r['nodes']):.1f}%), {r['roots']} roots\n"
    )
    v = r["rolling_prefix_violations"]
    if v["depth"] or v["parent"]:
        w(
            f"  ROLLING-PREFIX VIOLATIONS: {v['depth']} depth, {v['parent']} parent"
            f" — {v['example']}\n"
        )
    else:
        w("  rolling-prefix identity holds: every key has one depth and one parent\n")

    w("\n  roots, by sessions — the global prefix is the segment starting at each\n")
    w("    sessions  preamble  ends       first branch at depth  children there\n")
    for row in r["root_table"]:
        w(
            f"    {row['sessions']:8}  {row['segment_length']:8}  {row['segment_ends']:9}"
            f"  {row['branch_depth']:21}  {row['children_at_branch']:14}\n"
        )

    s = r["segments"]
    w(
        f"\n  {s['shared']} shared segments of {s['total']} total"
        " — top by nodes x sessions (references onto shared content)\n"
    )
    w("    depth   length  sessions  ends       nodes x sessions\n")
    for row in s["top_by_value"]:
        w(
            f"    {row['start_depth']:6}  {row['length']:6}  {row['sessions']:8}"
            f"  {row['ends']:9}  {row['node_sessions']:16}\n"
        )

    for spine in r["spines"]:
        w(
            f"\n  spine below root {spine['root']} ({spine['root_sessions']} sessions),"
            " unary chains collapsed, thin branches pruned\n"
        )
        for row in spine["rows"]:
            pad = "    " + "  " * row["indent"]
            kids = ", ".join(str(x) for x in row["child_sessions"])
            w(
                f"{pad}depth {row['start_depth']}..{row['end_depth']}"
                f" ({row['length']} nodes) x {row['sessions']} sessions"
                f" -> {row['children']} children, {row['shared_children']} shared,"
                f" {row['followed']} followed [{kids}]\n"
            )

    rows = r["occupancy_by_depth"]
    w(
        f"\n  occupancy over {len(rows)} depths, runs of constant width collapsed:"
        " pooled (what stats/trunk.rs publishes) vs shared-only\n"
    )
    w("    depths            width  pooled  shared width  shared-only  max on one key\n")
    # Collapsed by (width, shared_width) rather than sampled, because a sampled
    # ladder hides exactly the thing being looked for: the depth at which the width
    # steps. A run boundary IS a fanout event.
    runs = []
    for row in rows:
        key = (row["width"], row["shared_width"])
        if runs and runs[-1]["key"] == key:
            runs[-1]["to"] = row["depth"]
            runs[-1]["n"] += 1
            runs[-1]["pooled"] += row["pooled"]
            runs[-1]["shared_only"] += row["shared_only"] or 0.0
            runs[-1]["max_sessions"] = max(runs[-1]["max_sessions"], row["max_sessions"])
        else:
            runs.append(
                {
                    "key": key,
                    "from": row["depth"],
                    "to": row["depth"],
                    "n": 1,
                    "pooled": row["pooled"],
                    "shared_only": row["shared_only"] or 0.0,
                    "max_sessions": row["max_sessions"],
                }
            )
    shown = runs if len(runs) <= 60 else runs[:40] + runs[-20:]
    for i, run in enumerate(shown):
        if i == 40 and len(runs) > 60:
            w(f"    ... {len(runs) - 60} runs elided ...\n")
        span = f"{run['from']}..{run['to']}" if run["to"] > run["from"] else f"{run['from']}"
        w(
            f"    {span:16}  {run['key'][0]:5}  {run['pooled'] / run['n']:6.2f}"
            f"  {run['key'][1]:12}  {run['shared_only'] / run['n']:11.1f}"
            f"  {run['max_sessions']:14}\n"
        )


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("trace_dir")
    ap.add_argument("--block-size", type=int, default=None)
    ap.add_argument("--max-invocations", type=int, default=0)
    ap.add_argument("--top", type=int, default=12)
    ap.add_argument("--print-depth", type=int, default=400)
    ap.add_argument("--min-sessions", type=int, default=2)
    ap.add_argument("--spine-roots", type=int, default=3)
    ap.add_argument(
        "--spine-frac",
        type=float,
        default=0.02,
        help="follow a child only if it holds this fraction of its root's sessions",
    )
    ap.add_argument("--spine-budget", type=int, default=40, help="max rows per spine")
    ap.add_argument("--json", default=None)
    args = ap.parse_args()

    manifest = load_manifest(args.trace_dir)
    if (manifest.get("field_status") or {}).get("session_id") not in ("native", "reconstructed"):
        sys.exit(f"{args.trace_dir}: no session identity, so occupancy is undefined")
    block_size = pick_block_size(args.trace_dir, manifest, args.block_size)
    if not block_size:
        sys.exit(f"{args.trace_dir}: metadata_only trace, no block data to profile")
    if manifest.get("id_semantics") != "rolling_prefix":
        sys.exit(
            f"{args.trace_dir}: id_semantics is {manifest.get('id_semantics')!r}, not"
            " rolling_prefix — a trie cannot be recovered from ids that do not encode"
            " their prefix"
        )

    rows = read_invocations(args.trace_dir, block_size, args.max_invocations)
    encoding = (
        "full"
        if (manifest.get("field_status") or {}).get("full_input_blocks") == "native"
        else "delta"
    )
    paths = reconstruct(rows, encoding)
    trie, used, skipped, session_count = build(
        rows, paths, [r.get("session_id") for r in rows]
    )
    meta = {
        "trace": os.path.basename(os.path.abspath(args.trace_dir)),
        "block_size": block_size,
        "encoding": encoding,
    }
    r = report(trie, used, skipped, session_count, meta, args.top, args.print_depth,
               args.min_sessions, args.spine_roots, args.spine_frac, args.spine_budget)
    render(r)
    if args.json:
        with open(args.json, "w") as f:
            json.dump(r, f)


if __name__ == "__main__":
    main()
