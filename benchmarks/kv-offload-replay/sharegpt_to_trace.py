#!/usr/bin/env python3
"""sharegpt_to_trace.py — build a turn-level trace from ShareGPT_Vicuna_unfiltered.

Output JSONL, one record per user turn (the part a prefill scheduler sees):
  {
    "arrival_s": float,          # simulated arrival timestamp (seconds)
    "conv_id":   str,            # stable conversation id
    "turn_idx":  int,            # 0-based index within conversation
    "prompt_tokens": [int, ...], # concat of all prior turns + new user msg
    "output_len": int,           # assistant reply length in tokens
    "is_agent":  bool,           # heuristic: long/bursty multi-turn sessions
  }

Arrival model:
  - Conversations start times ~ Poisson(rate=--arrival-rate, seed=--seed).
  - Turn k (k>0) arrives after turn k-1 finishes: arrival_{k-1} + output_len / decode_tps
    + Exp(mean=think_time_s).
"""

import argparse
import json
import random
import sys
from pathlib import Path

from datasets import load_dataset
from transformers import AutoTokenizer


TOKENIZER_ID = "NousResearch/Meta-Llama-3-8B"
DATA_FILE = "ShareGPT_V3_unfiltered_cleaned_split_no_imsorry.json"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--num-conversations", type=int, default=2000)
    ap.add_argument("--min-turns", type=int, default=2)
    ap.add_argument("--max-prompt-tokens", type=int, default=8192,
                    help="drop turns whose cumulative prompt exceeds this")
    ap.add_argument("--arrival-rate", type=float, default=2.0,
                    help="new-conversation arrival rate (req/s)")
    ap.add_argument("--decode-tps", type=float, default=80.0,
                    help="simulated decode tokens/sec (for inter-turn spacing)")
    ap.add_argument("--think-time-s", type=float, default=4.0,
                    help="mean user think time between turns (seconds)")
    ap.add_argument("--agent-turns", type=int, default=5,
                    help="≥ this many turns ⇒ is_agent=True")
    ap.add_argument("--agent-ctx", type=int, default=4096,
                    help="≥ this many prompt tokens at any turn ⇒ is_agent=True")
    ap.add_argument("--seed", type=int, default=17)
    args = ap.parse_args()

    rng = random.Random(args.seed)

    print(f"[trace] loading tokenizer {TOKENIZER_ID}", file=sys.stderr)
    tok = AutoTokenizer.from_pretrained(TOKENIZER_ID, use_fast=True)

    print(f"[trace] streaming dataset {DATA_FILE}", file=sys.stderr)
    ds = load_dataset(
        "anon8231489123/ShareGPT_Vicuna_unfiltered",
        data_files=DATA_FILE,
        split="train",
        streaming=True,
    )

    # First pass: keep up to N conversations meeting min-turn requirement.
    kept: list[tuple[str, list[dict]]] = []
    for ex in ds:
        turns_raw = ex.get("conversations", [])
        # Each turn is a JSON-encoded string (streaming mode) or a dict.
        turns: list[dict] = []
        for t in turns_raw:
            if isinstance(t, str):
                try:
                    t = json.loads(t)
                except Exception:
                    continue
            if isinstance(t, dict) and "from" in t and "value" in t:
                turns.append(t)
        if len(turns) < args.min_turns:
            continue
        # Walk turns, keep only the human→gpt pairs.
        paired: list[dict] = []
        i = 0
        while i < len(turns) - 1:
            if turns[i]["from"] == "human" and turns[i + 1]["from"] == "gpt":
                paired.append(turns[i])
                paired.append(turns[i + 1])
                i += 2
            else:
                i += 1
        if len(paired) // 2 < args.min_turns:
            continue
        kept.append((ex.get("id", f"conv_{len(kept)}"), paired))
        if len(kept) >= args.num_conversations:
            break

    print(f"[trace] kept {len(kept)} conversations, tokenizing...", file=sys.stderr)

    # Tokenize each conversation and emit records.
    out_path: Path = args.out
    out_path.parent.mkdir(parents=True, exist_ok=True)

    records: list[dict] = []
    conv_start_s = 0.0
    n_dropped_long = 0
    n_records = 0

    for conv_idx, (conv_id, turns) in enumerate(kept):
        # Poisson inter-arrival between conversation starts
        gap = rng.expovariate(args.arrival_rate) if args.arrival_rate > 0 else 0.0
        conv_start_s += gap
        t_now = conv_start_s
        n_pairs = len(turns) // 2

        is_agent_conv = n_pairs >= args.agent_turns

        cumulative_ids: list[int] = []
        kept_in_conv: list[dict] = []

        for turn_idx in range(n_pairs):
            user_text = turns[2 * turn_idx]["value"]
            gpt_text = turns[2 * turn_idx + 1]["value"]

            user_ids = tok.encode(user_text, add_special_tokens=(turn_idx == 0))
            gpt_ids = tok.encode(gpt_text, add_special_tokens=False)

            cumulative_ids = cumulative_ids + user_ids
            prompt_len = len(cumulative_ids)
            output_len = len(gpt_ids)

            if prompt_len > args.max_prompt_tokens:
                n_dropped_long += 1
                break

            rec = {
                "arrival_s": round(t_now, 6),
                "conv_id": str(conv_id),
                "turn_idx": turn_idx,
                "prompt_tokens": cumulative_ids.copy(),
                "output_len": output_len,
                "is_agent": is_agent_conv or prompt_len >= args.agent_ctx,
            }
            kept_in_conv.append(rec)

            decode_s = output_len / max(1.0, args.decode_tps)
            think_s = rng.expovariate(1.0 / args.think_time_s) if args.think_time_s > 0 else 0.0
            t_now += decode_s + think_s

            cumulative_ids = cumulative_ids + gpt_ids

        records.extend(kept_in_conv)
        n_records += len(kept_in_conv)

        if (conv_idx + 1) % 250 == 0:
            print(f"[trace] tokenized {conv_idx + 1}/{len(kept)} convs, "
                  f"{n_records} records", file=sys.stderr)

    # Sort by arrival so the scheduler sees a time-ordered stream.
    records.sort(key=lambda r: r["arrival_s"])

    with out_path.open("w") as f:
        for r in records:
            f.write(json.dumps(r) + "\n")

    horizon = records[-1]["arrival_s"] if records else 0.0
    agent_frac = sum(1 for r in records if r["is_agent"]) / max(1, len(records))
    mean_prompt = sum(len(r["prompt_tokens"]) for r in records) / max(1, len(records))

    print(f"[trace] wrote {len(records)} records → {out_path}", file=sys.stderr)
    print(f"[trace] horizon={horizon:.1f}s  mean_prompt_tok={mean_prompt:.1f}  "
          f"agent_frac={agent_frac:.2%}  dropped_long={n_dropped_long}", file=sys.stderr)


if __name__ == "__main__":
    main()
