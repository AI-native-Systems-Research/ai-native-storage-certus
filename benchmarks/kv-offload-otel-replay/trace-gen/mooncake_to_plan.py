#!/usr/bin/env python3.12
"""Convert a mooncake request trace (cc131k) into a conversation_replay PLAN
trace, so the existing `trace_to_otel.py` (stage 2) can materialise an OTel
corpus from it. This is the mooncake-shaped counterpart of
`gen_conversation_trace.py` (which only knows inference-perf's generator).

WHY A RECONSTRUCTION STEP
-------------------------
A mooncake row is ONE independent request:
    {"timestamp", "input_length", "output_length", "hash_ids": [block_hash,...]}
There are no conversation ids and no turns. The ONLY structure is `hash_ids`:
each entry hashes a fixed-size block of the prompt (64 tokens in cc131k), and a
*shared leading prefix of hash_ids* means a shared KV prefix. A multi-turn chat
shows up as a chain of rows whose hash_ids each EXTEND the previous row's
hash_ids (turn k+1's prompt = turn k's prompt + turn k's output + new user
input). We rebuild those chains so within-conversation KV reuse survives into
the OTel corpus — the structure guidellm's flat replay destroys.

ALGORITHM
---------
1. block_size = median(input_length / len(hash_ids))  (64 for cc131k).
2. For each row find its PARENT = the row whose full hash_ids tuple is the
   longest proper prefix of this row's hash_ids. Rows with no parent are roots.
3. Each node continues into exactly ONE child (first in trace order) to bound
   corpus size to O(#rows) turns; a parent's other children start new chains
   (their turn 0 re-sends the full prompt — the shared-ancestor reuse for those
   extra branches is not representable in trace_to_otel's single-global-prefix
   model, and is reported as `branch_roots`).
4. A chain [r0..rk] becomes one conversation. Per-turn *incremental* user-input
   tokens (what trace_to_otel accumulates on top of history):
       turn 0 : r0.input_length - shared_system_prompt_len
       turn i : max(1, r_i.input_length - r_{i-1}.input_length
                        - r_{i-1}.output_length)
   output_tokens = row.output_length. Inter-turn latency = 0.0: mooncake
   timestamps are global arrival times, not within-conversation gaps, and the
   replay arms run TIME_SCALE=0 anyway.
5. shared_system_prompt_len = block_size * (number of leading hash_ids common to
   EVERY chain root). Often 0 for a heterogeneous trace; within-conversation
   reuse does not depend on it.

Usage:
    python3.12 mooncake_to_plan.py <mooncake.jsonl> <out_plan.json> \
        [--max-rows N] [--stats-only]
"""
from __future__ import annotations

import argparse
import json
import statistics
import sys
import time


def load_rows(path: str, max_rows: int | None) -> list[dict]:
    rows = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            rows.append(json.loads(line))
            if max_rows is not None and len(rows) >= max_rows:
                break
    return rows


def common_prefix_len(seqs: list[tuple]) -> int:
    """Length of the longest hash_id prefix shared by every sequence."""
    if not seqs:
        return 0
    shortest = min(len(s) for s in seqs)
    n = 0
    for i in range(shortest):
        v = seqs[0][i]
        if all(s[i] == v for s in seqs):
            n += 1
        else:
            break
    return n


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("trace")
    ap.add_argument("out_plan")
    ap.add_argument("--max-rows", type=int, default=None,
                    help="only read the first N rows (for quick tests)")
    ap.add_argument("--stats-only", action="store_true",
                    help="print reconstruction stats and exit; do not write")
    args = ap.parse_args()

    t0 = time.time()
    rows = load_rows(args.trace, args.max_rows)
    n = len(rows)
    if n == 0:
        sys.exit("no rows read")

    # 1. block size
    ratios = [r["input_length"] / len(r["hash_ids"]) for r in rows if r["hash_ids"]]
    block_size = int(round(statistics.median(ratios)))

    # 2. parent = longest proper-prefix row. Index every full hash_ids tuple.
    full = [tuple(r["hash_ids"]) for r in rows]
    present = set(full)
    parent = [-1]  # placeholder, rebuilt below
    parent = [None] * n
    # Map a full tuple -> an owning row index (first occurrence wins).
    owner: dict[tuple, int] = {}
    for i, t in enumerate(full):
        owner.setdefault(t, i)
    for i, t in enumerate(full):
        p = None
        for cut in range(len(t) - 1, 0, -1):
            pref = t[:cut]
            if pref in present:
                p = owner[pref]
                if p == i:  # identical duplicate row; treat as its own continuation
                    continue
                break
        parent[i] = p

    # 3. children (trace order), chain assignment: each node keeps its FIRST child.
    children: dict[int, list[int]] = {}
    roots: list[int] = []
    for i in range(n):
        p = parent[i]
        if p is None or p == i:
            roots.append(i)
        else:
            children.setdefault(p, []).append(i)

    continued: set[int] = set()  # nodes already consumed as a non-root chain link
    chains: list[list[int]] = []
    branch_roots = 0

    # Deterministic order: roots first (trace order), then any branch starts.
    def build_chain(start: int) -> list[int]:
        chain = [start]
        cur = start
        while True:
            kids = children.get(cur, [])
            nxt = None
            for k in kids:
                if k not in continued:
                    nxt = k
                    break
            if nxt is None:
                break
            continued.add(nxt)
            chain.append(nxt)
            cur = nxt
        return chain

    for r in roots:
        chains.append(build_chain(r))
    # Any node that had a parent but was never continued into a chain starts its
    # own chain (a shared-prefix branch we cannot fold into its ancestor).
    for i in range(n):
        if parent[i] is not None and parent[i] != i and i not in continued \
                and all(i != c[0] for c in chains):
            branch_roots += 1
            chains.append(build_chain(i))

    # 5. global shared system prompt = common hash_id prefix across chain roots.
    root_tuples = [full[c[0]] for c in chains]
    sys_blocks = common_prefix_len(root_tuples)
    shared_system_prompt_len = sys_blocks * block_size

    # 4. emit conversations
    conversations = []
    clamped = 0
    all_turns, all_in, all_out = [], [], []
    for cid, chain in enumerate(chains):
        turns = []
        for pos, ri in enumerate(chain):
            r = rows[ri]
            if pos == 0:
                in_tok = r["input_length"] - shared_system_prompt_len
            else:
                prev = rows[chain[pos - 1]]
                in_tok = r["input_length"] - prev["input_length"] - prev["output_length"]
            if in_tok < 1:
                in_tok = 1
                clamped += 1
            out_tok = int(r["output_length"])
            turns.append([int(in_tok), out_tok, 0.0])
            all_in.append(int(in_tok))
            all_out.append(out_tok)
        all_turns.append(len(turns))
        conversations.append({
            "id": cid,
            "num_turns": len(turns),
            "system_prompt_tokens": shared_system_prompt_len,
            "turns": turns,
        })

    multiturn = sum(1 for c in chains if len(c) > 1)
    meta_totals = {
        "total_turns": int(sum(all_turns)),
        "mean_turns_per_conversation": round(statistics.mean(all_turns), 2),
        "min_turns": int(min(all_turns)),
        "max_turns": int(max(all_turns)),
        "sum_input_tokens": int(sum(all_in)),
        "sum_output_tokens": int(sum(all_out)),
    }

    print(f"rows read                : {n}")
    print(f"block size (tokens)      : {block_size}")
    print(f"conversations (chains)   : {len(chains)}  (multi-turn: {multiturn}, "
          f"single-turn: {len(chains) - multiturn})")
    print(f"branch roots (lost reuse): {branch_roots}")
    print(f"shared system prefix     : {sys_blocks} blocks = "
          f"{shared_system_prompt_len} tokens")
    print(f"turns/conv mean/min/max  : {meta_totals['mean_turns_per_conversation']} / "
          f"{meta_totals['min_turns']} / {meta_totals['max_turns']}")
    print(f"clamped (<1 tok) turns   : {clamped}")
    print(f"sum input/output tokens  : {meta_totals['sum_input_tokens']} / "
          f"{meta_totals['sum_output_tokens']}")
    print(f"elapsed                  : {time.time() - t0:.1f}s")

    if args.stats_only:
        return

    trace = {
        "metadata": {
            "generator": "mooncake_to_plan",
            "source_trace": args.trace,
            "block_size": block_size,
            "shared_system_prompt_len": shared_system_prompt_len,
            "note": "Reconstructed from mooncake hash_ids prefix chains. Per-turn "
                    "input is INCREMENTAL user tokens; latency 0 (TIME_SCALE=0 replay).",
            "turn_fields": ["input_tokens", "output_tokens", "tool_call_latency_sec"],
            "totals": meta_totals,
        },
        "conversations": conversations,
    }
    with open(args.out_plan, "w", encoding="utf-8") as f:
        json.dump(trace, f, separators=(",", ":"))
    print(f"wrote                    : {args.out_plan}")


if __name__ == "__main__":
    main()
