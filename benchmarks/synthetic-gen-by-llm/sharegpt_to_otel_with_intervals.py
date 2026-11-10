#!/usr/bin/env python3.12
"""Build an OTel `otel_trace_replay` corpus from a ShareGPT dataset, embedding
inter-turn gaps sampled from an interval-distribution YAML.

This is a content-from-ShareGPT / timing-from-distribution variant of
``benchmarks/kv-offload-otel-replay/trace-gen/trace_to_otel.py``. It emits the
*same* span schema (so the same replay drivers consume it), but:

  * message CONTENT comes from the ShareGPT file — each ``human`` turn is sent as
    the user message; the ``gpt`` turn's real token count drives
    ``gen_ai.usage.output_tokens`` (the assistant text is a marker, substituted at
    replay like the original pipeline);
  * inter-turn GAPS are not recorded latencies but samples drawn from the passed
    interval distribution (e.g. cc-trace-weka-062126-intervals.yaml), so the
    replay's wall-clock turn spacing follows that distribution.

Output is a directory, one ``conv_NNNNN.json`` per conversation (the
``otel_trace_replay`` loader maps one file -> one replay session), matching the
shipped ``otel_1k`` layout.

ShareGPT schema (input):
    [ {"id": "...", "conversations": [ {"from":"human","value":"..."},
                                       {"from":"gpt","value":"..."}, ... ]}, ... ]

Usage:
    ./sharegpt_to_otel_with_intervals.py SHAREGPT.json DIST.yaml OUT_DIR \
        [--num N] [--cap 120000] [--model Qwen/Qwen2.5-7B-Instruct] \
        [--system-tokens 0] [--seed 7]

Example:
    ./sharegpt_to_otel_with_intervals.py /mnt/certus1/synth-1K-M50.json \
        cc-trace-weka-062126-intervals.yaml /mnt/certus1/otel-synth-1K-M50-weka
"""
from __future__ import annotations

import argparse
import json
import math
import os
import sys
import time
import warnings
from collections import deque
from datetime import datetime, timedelta, timezone

warnings.filterwarnings("ignore")
import numpy as np
import yaml

BASE = datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc)


# ─── interval sampling ───────────────────────────────────────────────────────
class IntervalSampler:
    """Sample inter-turn gaps (seconds) from a bucketed distribution YAML.

    A bucket is chosen with probability proportional to its ``count`` (falling
    back to ``fraction``); a value is then drawn *within* the bucket — log-uniform
    when the histogram's ``scale`` is log, uniform when linear — so the sampled
    gaps reproduce both the bucket weights and the within-bucket spread.
    """

    def __init__(self, dist, rng):
        self.rng = rng
        self.scale = dist.get("scale", "log")
        buckets = dist.get("buckets") or []
        if not buckets:
            raise ValueError("distribution YAML has no buckets")
        self.lowers = np.array([b["lower_s"] for b in buckets], dtype=float)
        self.uppers = np.array([b["upper_s"] for b in buckets], dtype=float)
        weights = np.array(
            [b.get("count", b.get("fraction", 0.0)) for b in buckets], dtype=float)
        if weights.sum() <= 0:
            raise ValueError("distribution buckets have zero total weight")
        self.probs = weights / weights.sum()
        # Log sampling needs a strictly positive floor.
        self.log_floor = 1e-9

    def sample(self) -> float:
        i = self.rng.choice(len(self.probs), p=self.probs)
        lo, hi = self.lowers[i], self.uppers[i]
        if hi <= lo:
            return max(0.0, float(lo))
        if self.scale == "log":
            lo = max(lo, self.log_floor)
            hi = max(hi, lo * (1 + 1e-9))
            return float(10 ** self.rng.uniform(math.log10(lo), math.log10(hi)))
        return float(self.rng.uniform(lo, hi))


# ─── helpers ─────────────────────────────────────────────────────────────────
def iso(dt: datetime) -> str:
    return dt.isoformat()


def gen_text(tok, rng, n_tokens: int) -> str:
    """Random text that tokenizes to ~n_tokens (for the shared system prompt)."""
    if n_tokens <= 0:
        return ""
    ids = rng.integers(0, tok.vocab_size, size=int(n_tokens)).tolist()
    return tok.decode(ids, skip_special_tokens=True)


def sharegpt_turns(conv):
    """Yield (human_text, gpt_text) pairs from a ShareGPT conversation.

    Pairs each ``human`` message with the next ``gpt`` message; unpaired trailing
    messages are dropped (matches the load_convs convention).
    """
    msgs = conv.get("conversations") or []
    i = 0
    while i < len(msgs):
        if msgs[i].get("from") == "human":
            # find the next gpt reply
            j = i + 1
            while j < len(msgs) and msgs[j].get("from") != "gpt":
                j += 1
            if j < len(msgs):
                yield msgs[i].get("value", ""), msgs[j].get("value", "")
                i = j + 1
                continue
        i += 1


def build_conversation(idx, conv, turns, sys_text, sys_tok, tok, sampler,
                       cap, model):
    """Return (trace_obj, stats) for one conversation.

    ``turns`` is a list of (human_text, gpt_text, in_tok, out_tok) with token
    counts precomputed. Inter-turn gaps are sampled from ``sampler``.
    """
    trace_id = f"conv_{idx:05d}"
    history = deque()  # dict(user_dict, user_tok, asst_dict, out_tok)
    spans = []
    t = BASE
    max_span_tok = 0
    gaps = []

    for i, (user_text, _gpt_text, in_tok, out_tok) in enumerate(turns):
        user_dict = {"role": "user", "content": user_text}

        def accounted():
            return sys_tok + sum(h["user_tok"] + h["out_tok"] for h in history) + in_tok

        # Sliding window: drop oldest completed pairs until the accounted prompt
        # fits under the cap (system + kept pairs + current user).
        while history and accounted() > cap:
            history.popleft()

        messages = []
        if sys_tok > 0:
            messages.append({"role": "system", "content": sys_text})
        for h in history:
            messages.append(h["user_dict"])
            messages.append(h["asst_dict"])
        messages.append(user_dict)

        input_tokens = accounted()
        max_span_tok = max(max_span_tok, input_tokens + out_tok)

        asst_marker = f"[ASST {trace_id} t{i:03d}]"  # substituted at replay
        out_messages = [{"role": "assistant", "content": asst_marker}]

        start = t
        dur = timedelta(seconds=max(0.05, out_tok * 0.01))
        end = start + dur

        spans.append({
            "span_id": f"{trace_id}-s{i:03d}",
            "trace_id": trace_id,
            "name": f"chat {model}",
            "start_time": iso(start),
            "end_time": iso(end),
            "status": {"code": 1},
            "attributes": {
                "gen_ai.request.model": model,
                "gen_ai.usage.input_tokens": int(input_tokens),
                "gen_ai.usage.output_tokens": int(out_tok),
                "gen_ai.request.max_tokens": int(out_tok),
                "gen_ai.input.messages": json.dumps(messages, ensure_ascii=False),
                "gen_ai.output.messages": json.dumps(out_messages, ensure_ascii=False),
            },
        })

        # Gap before the NEXT turn is sampled from the interval distribution.
        if i < len(turns) - 1:
            gap = sampler.sample()
            gaps.append(gap)
            t = end + timedelta(seconds=gap)

        history.append({
            "user_dict": user_dict, "user_tok": int(in_tok),
            "asst_dict": {"role": "assistant", "content": asst_marker},
            "out_tok": int(out_tok),
        })

    trace = {
        "trace_id": trace_id,
        "source_id": conv.get("id"),
        "span_count": len(spans),
        "collected_at": iso(BASE),
        "spans": spans,
    }
    return trace, max_span_tok, gaps


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("sharegpt", help="ShareGPT dataset JSON")
    ap.add_argument("distribution", help="interval-distribution YAML")
    ap.add_argument("out_dir", help="output corpus directory (one file per conv)")
    ap.add_argument("--num", type=int, default=0,
                    help="max conversations (0 = all)")
    ap.add_argument("--cap", type=int, default=120000,
                    help="sliding-window context cap in tokens (default: 120000)")
    ap.add_argument("--model", default="Qwen/Qwen2.5-7B-Instruct",
                    help="tokenizer / span model id (default: Qwen/Qwen2.5-7B-Instruct)")
    ap.add_argument("--system-tokens", type=int, default=0,
                    help="shared system-prompt size in tokens (0 = none)")
    ap.add_argument("--seed", type=int, default=7,
                    help="RNG seed for interval sampling + system filler")
    args = ap.parse_args()

    with open(args.sharegpt) as f:
        data = json.load(f)
    if not isinstance(data, list):
        print("error: ShareGPT file must be a JSON list of conversations",
              file=sys.stderr)
        return 1
    if args.num > 0:
        data = data[: args.num]

    with open(args.distribution) as f:
        dist = yaml.safe_load(f)

    rng = np.random.default_rng(args.seed)
    sampler = IntervalSampler(dist, rng)

    from transformers import AutoTokenizer
    tok = AutoTokenizer.from_pretrained(args.model)

    def ntok(text):
        return len(tok.encode(text or "", add_special_tokens=False))

    sys_text = gen_text(tok, rng, args.system_tokens)
    sys_tok = ntok(sys_text) if args.system_tokens > 0 else 0

    os.makedirs(args.out_dir, exist_ok=True)

    t0 = time.time()
    total_spans = 0
    total_bytes = 0
    global_max_tok = 0
    all_gaps = []
    written = 0
    skipped = 0

    for k, conv in enumerate(data):
        pairs = list(sharegpt_turns(conv))
        if not pairs:
            skipped += 1
            continue
        turns = [(h, g, ntok(h), ntok(g)) for h, g in pairs]
        trace_obj, max_tok, gaps = build_conversation(
            written, conv, turns, sys_text, sys_tok, tok, sampler, args.cap, args.model)
        global_max_tok = max(global_max_tok, max_tok)
        total_spans += trace_obj["span_count"]
        all_gaps.extend(gaps)

        path = os.path.join(args.out_dir, f"{trace_obj['trace_id']}.json")
        with open(path, "w", encoding="utf-8") as f:
            s = json.dumps(trace_obj, ensure_ascii=False)
            f.write(s)
            total_bytes += len(s)
        written += 1
        if written % 100 == 0:
            el = time.time() - t0
            print(f"  {written}/{len(data)} convs  {total_spans} spans  "
                  f"{total_bytes/1e9:.2f} GB  {el:.0f}s", flush=True)

    g = np.array(all_gaps) if all_gaps else np.array([0.0])
    print(f"\nwrote {written} files -> {args.out_dir}")
    if skipped:
        print(f"skipped (no human/gpt turns): {skipped}")
    print(f"total spans          : {total_spans:,}")
    print(f"on-disk size         : {total_bytes/1e9:.2f} GB")
    print(f"max accounted span   : {global_max_tok:,} tokens (cap {args.cap})")
    print(f"sampled gaps         : {len(all_gaps):,}  "
          f"median={np.median(g):.2f}s p90={np.percentile(g,90):.2f}s "
          f"p99={np.percentile(g,99):.2f}s max={g.max():.2f}s")
    print(f"elapsed              : {time.time()-t0:.0f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
