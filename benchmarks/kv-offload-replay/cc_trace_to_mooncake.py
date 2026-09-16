#!/usr/bin/env python3
"""cc_trace_to_mooncake.py — convert a Certus "cc" conversation trace into a
flat guidellm Mooncake trace.

The Certus cc-trace (e.g. /mnt/certus1/cc-traces-weka-062126.jsonl) is NESTED:
one JSON object per line = one conversation
    {"id","models":[...],"block_size":64,"hash_id_scope":"local",
     "requests":[{"t","model","in","out","hash_ids":[...],"api_time","type","ttft"},...]}

guidellm's `mooncake` deserializer (guidellm/data/deserializers/trace_mooncake.py)
instead wants a FLAT trace — one request per line — with columns
`timestamp,input_length,output_length,hash_ids`, treats hash_ids as a GLOBAL id
space, and requires ceil(input_length/hash_id_block_size)==len(hash_ids).

This converter flattens requests[] to one row per request, renames t/in/out, and
— because the source hash scope is "local" (block N of conversation A is NOT the
same block as block N of conversation B) — remaps every conversation's ids into
a disjoint global range. That preserves *intra*-conversation prefix reuse (a
later turn reusing an earlier turn's blocks) while removing the *false*
cross-conversation sharing guidellm would otherwise invent by treating the local
ids as global.

Fidelity caveats (inherent to guidellm mooncake, not this converter):
  * Prompts are SYNTHETIC (Faker text sized to the token counts) — the real cc
    trace carries no prompt text anyway.
  * The per-request `model` (opus vs haiku), `api_time`, `ttft`, conversation
    grouping and turn ordering are dropped; every row is an independent request
    sent to the single --model you configure.
  * Arrival timing is only reproduced with `--profile kind=replay`; sweep/
    throughput/constant drive their own rate and ignore `timestamp`.
"""
from __future__ import annotations

import argparse
import json
import math
import sys


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("infile", help="nested cc-trace .jsonl (one conversation per line)")
    ap.add_argument("outfile", help="flat mooncake .jsonl (one request per line)")
    ap.add_argument("--model", default=None,
                    help="keep only requests whose 'model' matches (default: all)")
    ap.add_argument("--max-context", type=int, default=0,
                    help="drop rows where input_length+output_length exceeds this "
                         "(0 = no limit). Set to the serving --max-model-len so the "
                         "replay does not 400 on context-length-exceeded. The cc "
                         "traces reach ~1M tokens; Qwen2.5-7B allows 32768 (131072 "
                         "with YaRN).")
    args = ap.parse_args()

    next_base = 0          # running global offset for the disjoint id ranges
    n_conv = n_req = n_out = 0
    bad = 0
    incomplete = 0
    over_ctx = 0
    block_size = None

    with open(args.infile) as fin, open(args.outfile, "w") as fout:
        for line in fin:
            line = line.strip()
            if not line:
                continue
            conv = json.loads(line)
            n_conv += 1
            bs = conv.get("block_size")
            if block_size is None:
                block_size = bs
            elif bs != block_size:
                print(f"warning: mixed block_size {bs} != {block_size} "
                      f"(conv {conv.get('id')})", file=sys.stderr)

            # Disjoint global range for this conversation's local ids.
            max_local = -1
            for r in conv.get("requests", []):
                for h in r.get("hash_ids", []):
                    if h > max_local:
                        max_local = h
            base = next_base
            next_base += max_local + 1  # advance past this conv's id space

            for r in conv.get("requests", []):
                n_req += 1
                if args.model is not None and r.get("model") != args.model:
                    continue
                # Some records (e.g. non-'s' turns) omit in/out/hash_ids — skip them.
                if "in" not in r or "out" not in r or "hash_ids" not in r:
                    incomplete += 1
                    continue
                nin = r["in"]
                if args.max_context and nin + r["out"] > args.max_context:
                    over_ctx += 1
                    continue
                hids = [base + h for h in r["hash_ids"]]
                if bs and math.ceil(nin / bs) != len(hids):
                    bad += 1
                    continue
                fout.write(json.dumps({
                    "timestamp": r["t"],
                    "input_length": nin,
                    "output_length": r["out"],
                    "hash_ids": hids,
                }) + "\n")
                n_out += 1

    print(f"conversations={n_conv} requests_in={n_req} rows_out={n_out} "
          f"skipped_incomplete={incomplete} skipped_over_context={over_ctx} "
          f"skipped_bad_invariant={bad} block_size={block_size}", file=sys.stderr)
    print(f"hint: run guidellm with "
          f"--data 'kind=mooncake,path={args.outfile},hash_id_block_size={block_size}'",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
