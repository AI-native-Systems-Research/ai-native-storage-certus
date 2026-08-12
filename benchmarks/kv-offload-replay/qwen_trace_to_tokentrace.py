#!/usr/bin/env python3
"""qwen_trace_to_tokentrace.py — convert a Qwen Bailian usage trace into a
compact, self-contained "token trace" for the KV-offload-replay drivers.

The Qwen Bailian anonymous dataset
(https://github.com/alibaba-edu/qwen-bailian-usagetraces-anon) ships
metadata-only JSONL: no request text, only block hashes and lengths. Each
record is one chat *turn*:

    {"chat_id", "parent_chat_id"(-1=root), "timestamp",
     "input_length", "output_length", "type", "turn",
     "hash_ids": [ int, ... ]}

`hash_ids` is the FULL cumulative input for that turn, expressed as a list of
salted-SipHash identities of consecutive `block_size`-token blocks (the trace
files are named ..._blksz_16, i.e. 16 tokens per block). Identical `hash_id`
means byte-identical block content anywhere in the corpus — so block 0 is the
shared system prompt across every session, and a turn's leading full blocks are
shared with its parent turn up to the last block boundary of the parent's
cumulative content (verified against the coder trace: parent had 112 blocks,
child's LCP was 111 — diverging exactly at the parent's last, partial block).

This converter reconstructs multi-turn sessions from the parent_chat_id chains
and emits a compact JSON the drivers can replay by deterministically expanding
each hash_id into `block_size` synthetic token-ids (see the drivers'
`expand_hashes`). Because the expansion is a pure function of the hash_id, the
prefix-cache / offload hit pattern of the original trace is reproduced exactly
at block granularity, without ever needing the (unavailable) real tokens.

Output schema:

    {
      "meta": {"trace": <name>, "scenario": <type>, "block_size": 16,
               "num_sessions": N, "max_model_len": M, ...},
      "sessions": [
        {"session_id": <int>,
         "turns": [{"hash_ids": [int, ...], "max_tokens": <output_length>}, ...]}
      ]
    }

Turns whose cumulative prompt would exceed --max-model-len are dropped (and,
since the prefix only grows, so is the rest of that session); max_tokens is
capped so prompt+output fits the context window.

Usage:
    qwen_trace_to_tokentrace.py \
        --trace /mnt/certus1/qwen-traces/qwen_coder_blksz_16.jsonl \
        --num-sessions 64 --min-turns 2 --max-model-len 32768 \
        --out token_trace_coder_64s.json
"""

import argparse
import json
import os
import random
import sys
from collections import Counter, defaultdict


def parse_args(argv=None):
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--trace", default="/mnt/certus1/qwen-traces/qwen_coder_blksz_16.jsonl",
                   help="Qwen Bailian JSONL trace (default: coder scenario)")
    p.add_argument("--out", default="",
                   help="output JSON path (default: token_trace_<scenario>_<N>s.json "
                        "beside this script)")
    p.add_argument("--num-sessions", type=int, default=64,
                   help="number of sessions to emit (default 64)")
    p.add_argument("--min-turns", type=int, default=1,
                   help="only sample sessions with at least this many turns (default 1)")
    p.add_argument("--max-turns", type=int, default=0,
                   help="cap turns per session (0 = no cap)")
    p.add_argument("--max-model-len", type=int, default=32768,
                   help="drop/cap turns so prompt (+output) fits this context window "
                        "(default 32768)")
    p.add_argument("--block-size", type=int, default=16,
                   help="tokens per hash block (matches the trace's blksz_N; default 16)")
    p.add_argument("--seed", type=int, default=0,
                   help="RNG seed for session sampling (default 0)")
    p.add_argument("--first-n", action="store_true",
                   help="take the first N eligible sessions by root chat_id instead of "
                        "a seeded random sample (fully deterministic ordering)")
    return p.parse_args(argv)


def build_sessions(recs):
    """Reconstruct one session per root as its longest parent->child chain.

    Branches (a turn with multiple children = regenerations / alternate
    continuations) are collapsed to the single deepest path so each session is
    a clean increasing-turn sequence. Returns (sessions, n_branch_records)
    where each session is a list of records ordered by turn.
    """
    byid = {r["chat_id"]: r for r in recs}
    children = defaultdict(list)
    for r in recs:
        children[r["parent_chat_id"]].append(r)
    # Sort each node's children by (turn, timestamp) for stable longest-path.
    for k in children:
        children[k].sort(key=lambda x: (x["turn"], x["timestamp"]))

    # Roots: parent is the -1 sentinel, or points outside this (possibly
    # sampled) file.
    roots = [r for r in recs
             if r["parent_chat_id"] == -1 or r["parent_chat_id"] not in byid]

    def longest_path(cid):
        best = []
        for c in children.get(cid, []):
            sub = longest_path(c["chat_id"])
            if len(sub) > len(best):
                best = sub
        return [cid] + best

    sys.setrecursionlimit(10000)
    sessions = []
    n_branch = 0
    for root in sorted(roots, key=lambda r: r["chat_id"]):
        path = longest_path(root["chat_id"])
        # Count records skipped because they were on a non-longest branch.
        n_branch += _subtree_size(root["chat_id"], children) - len(path)
        sessions.append([byid[c] for c in path])
    return sessions, n_branch


def _subtree_size(cid, children):
    n = 1
    for c in children.get(cid, []):
        n += _subtree_size(c["chat_id"], children)
    return n


def main(argv=None):
    args = parse_args(argv)
    if not os.path.exists(args.trace):
        print(f"[convert] missing trace {args.trace}", file=sys.stderr)
        return 1

    block = args.block_size
    scenario = None
    recs = []
    with open(args.trace) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            r = json.loads(line)
            recs.append(r)
            if scenario is None:
                scenario = r.get("type", "unknown")
    print(f"[convert] loaded {len(recs)} records from {os.path.basename(args.trace)} "
          f"(scenario={scenario})", file=sys.stderr)

    sessions, n_branch = build_sessions(recs)
    print(f"[convert] reconstructed {len(sessions)} sessions "
          f"({n_branch} branch records collapsed)", file=sys.stderr)

    # Eligibility: at least --min-turns turns.
    eligible = [s for s in sessions if len(s) >= args.min_turns]
    print(f"[convert] {len(eligible)} sessions have >= {args.min_turns} turn(s)",
          file=sys.stderr)
    if not eligible:
        print("[convert] no eligible sessions", file=sys.stderr)
        return 1

    if args.first_n:
        chosen = eligible[:args.num_sessions]
    else:
        rng = random.Random(args.seed)
        chosen = rng.sample(eligible, min(args.num_sessions, len(eligible)))
        # Stable, reproducible ordering in the output.
        chosen.sort(key=lambda s: s[0]["chat_id"])

    # Build compact turns, applying the context-window budget.
    out_sessions = []
    dropped_turns = 0
    capped_turns = 0
    for sid, sess in enumerate(chosen):
        turns = []
        for rec in sess:
            if args.max_turns and len(turns) >= args.max_turns:
                break
            prompt_tokens = len(rec["hash_ids"]) * block
            if prompt_tokens >= args.max_model_len:
                # Prefix only grows from here — truncate the session.
                dropped_turns += (len(sess) - len(turns))
                break
            max_tokens = int(rec["output_length"])
            room = args.max_model_len - prompt_tokens
            if max_tokens > room:
                max_tokens = room
                capped_turns += 1
            max_tokens = max(1, max_tokens)
            turns.append({"hash_ids": rec["hash_ids"], "max_tokens": max_tokens})
        if turns:
            out_sessions.append({"session_id": sid + 1, "turns": turns})

    turn_counts = Counter(len(s["turns"]) for s in out_sessions)
    total_turns = sum(len(s["turns"]) for s in out_sessions)
    total_prompt_tokens = sum(len(t["hash_ids"]) * block
                              for s in out_sessions for t in s["turns"])
    print(f"[convert] emitting {len(out_sessions)} sessions, {total_turns} turns "
          f"({dropped_turns} turns dropped over budget, {capped_turns} max_tokens capped)",
          file=sys.stderr)
    print(f"[convert] turn-count distribution: "
          f"{dict(sorted(turn_counts.items()))}", file=sys.stderr)
    print(f"[convert] total prompt tokens across all turns: {total_prompt_tokens} "
          f"(~{total_prompt_tokens // block} blocks)", file=sys.stderr)

    out_path = args.out
    if not out_path:
        here = os.path.dirname(os.path.abspath(__file__))
        out_path = os.path.join(here, f"token_trace_{scenario}_{len(out_sessions)}s.json")

    payload = {
        "meta": {
            "trace": os.path.basename(args.trace),
            "scenario": scenario,
            "block_size": block,
            "num_sessions": len(out_sessions),
            "total_turns": total_turns,
            "max_model_len": args.max_model_len,
            "min_turns": args.min_turns,
            "max_turns": args.max_turns or None,
            "seed": None if args.first_n else args.seed,
            "selection": "first-n" if args.first_n else "seeded-sample",
        },
        "sessions": out_sessions,
    }
    with open(out_path, "w") as f:
        json.dump(payload, f)
    print(f"[convert] wrote {out_path} "
          f"({os.path.getsize(out_path) / (1 << 20):.1f} MiB)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
