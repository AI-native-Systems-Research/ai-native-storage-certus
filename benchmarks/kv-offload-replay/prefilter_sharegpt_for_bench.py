#!/usr/bin/env python3
"""Pre-filter sharegpt_v3.json down to N conversations that pass vLLM's
ShareGPTDataset validity rules (is_valid_sequence defaults), then write a
subset file the bench can consume directly.

Reports scanned / accepted / rejected (with per-reason counts).
"""

import argparse
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Match vllm.benchmarks.datasets.datasets.is_valid_sequence defaults.
MIN_LEN = 4
MAX_PROMPT_LEN = 1024
MAX_TOTAL_LEN = 2048


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source", default=str(HERE / "sharegpt_v3.json"))
    ap.add_argument("--out", required=True,
                    help="output subset .json path")
    ap.add_argument("--target", type=int, default=5000,
                    help="number of accepted conversations to collect")
    ap.add_argument("--model", default="NousResearch/Meta-Llama-3-8B")
    args = ap.parse_args()

    src = Path(args.source)
    if not src.exists():
        print(f"missing {src}", file=sys.stderr)
        return 1

    print(f"[prefilter] loading {src} ...", file=sys.stderr)
    with open(src) as fh:
        data = json.load(fh)
    print(f"[prefilter] {len(data):,} conversations in source", file=sys.stderr)

    print(f"[prefilter] loading tokenizer for {args.model} ...",
          file=sys.stderr)
    from transformers import AutoTokenizer
    tok = AutoTokenizer.from_pretrained(args.model)

    accepted: list[dict] = []
    counts = {
        "scanned": 0,
        "no_pair": 0,
        "non_human_first": 0,
        "non_gpt_second": 0,
        "prompt_too_short": 0,
        "output_too_short": 0,
        "prompt_too_long": 0,
        "combined_too_long": 0,
        "accepted": 0,
    }

    for entry in data:
        if len(accepted) >= args.target:
            break
        counts["scanned"] += 1

        convs = entry.get("conversations") or []
        if len(convs) < 2:
            counts["no_pair"] += 1
            continue
        if convs[0].get("from") != "human":
            counts["non_human_first"] += 1
            continue
        if convs[1].get("from") != "gpt":
            counts["non_gpt_second"] += 1
            continue

        prompt = convs[0].get("value", "")
        completion = convs[1].get("value", "")
        p_ids = tok(prompt, add_special_tokens=False).input_ids
        c_ids = tok(completion, add_special_tokens=False).input_ids
        p_len, o_len = len(p_ids), len(c_ids)

        if p_len < MIN_LEN:
            counts["prompt_too_short"] += 1
            continue
        if o_len < MIN_LEN:
            counts["output_too_short"] += 1
            continue
        if p_len > MAX_PROMPT_LEN:
            counts["prompt_too_long"] += 1
            continue
        if (p_len + o_len) > MAX_TOTAL_LEN:
            counts["combined_too_long"] += 1
            continue

        accepted.append(entry)
        counts["accepted"] += 1

        if counts["scanned"] % 1000 == 0:
            print(f"[prefilter] scanned={counts['scanned']:,} "
                  f"accepted={counts['accepted']:,}", file=sys.stderr)

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w") as fh:
        json.dump(accepted, fh)
    print(f"\n[prefilter] wrote {len(accepted):,} conversations → {out}",
          file=sys.stderr)

    print("\n[prefilter] stats:", file=sys.stderr)
    for k, v in counts.items():
        print(f"  {k:<20} {v:>10,}", file=sys.stderr)
    rejected = counts["scanned"] - counts["accepted"]
    rate = (counts["accepted"] / counts["scanned"]) if counts["scanned"] else 0
    print(f"\n[prefilter] {rejected:,} rejected / {counts['scanned']:,} scanned"
          f" ({rate:.1%} acceptance)", file=sys.stderr)

    if counts["accepted"] < args.target:
        print(f"[prefilter] WARNING: only collected {counts['accepted']:,} of "
              f"{args.target:,} requested", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
