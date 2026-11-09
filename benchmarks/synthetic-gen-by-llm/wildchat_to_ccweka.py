#!/usr/bin/env python3
"""Convert the **WildChat-1M** corpus into the cc-weka JSONL schema.

``allenai/WildChat-1M`` is ~838K real human<->ChatGPT conversations collected
over months, each a list of alternating ``user``/``assistant`` messages. It is
the only large, public, *human*, timestamped multi-turn chat trace, so it is the
natural source for a **real human inter-turn think-time** distribution to drive
``tools/kv-cache-dynamics``. This tool re-emits each conversation in the cc-weka
JSONL schema so the existing ``extract_interval_distribution.py`` /
``extract_turn_distribution.py`` run on it unchanged.

TIMESTAMP MODEL (the one subtlety)
    In WildChat only **assistant** messages carry a ``timestamp`` (the moment the
    backend received the full response); **user** messages have ``timestamp ==
    None``. So we key each turn off its assistant message: one cc-weka
    ``requests[]`` entry per assistant message, with ``t`` = that message's
    timestamp relative to the conversation's first assistant message.

    An interval is therefore the gap between two consecutive assistant
    receipts == (user reads turn i) + (user types turn i+1) + (model generates
    turn i+1). It bundles human think-time with the next turn's generation
    latency; in the recency-relevant long tail the think-time term dominates, but
    it is **not** pure think-time, so treat short intervals (< a few seconds) as
    noise. This mirrors cc-weka's ``t`` (per-turn submission time) closely enough
    to sample the same recency axis.

So the emitted line is::

    {"id": <conversation_hash>, "models": [<model>],
     "requests": [{"t": 0.0}, {"t": 131.0}, {"t": 208.0}, ...]}   # per assistant turn

turn count  = len(requests)       (== WildChat's own ``turn`` field)
interval    = t[i+1] - t[i]       (intra-conversation assistant-receipt gap)

With ``--approx-tokens`` each request also carries ``in``/``out`` *estimated* as
``chars // 4`` (prior user+assistant context chars for ``in``, this turn's
assistant chars for ``out``). These are rough — WildChat ships no token counts
and this adapter deliberately avoids a tokenizer dependency — so they are opt-in
and the distribution extractors do not use them.

DOWNLOAD (no HF token required; ~3.2 GB, 14 shards)
    hf download allenai/WildChat-1M --repo-type dataset \\
        --local-dir <raw> --include 'data/*.parquet'

RUN
    ./wildchat_to_ccweka.py '<raw>/data/*.parquet' -o wildchat.jsonl
    ./extract_interval_distribution.py wildchat.jsonl -o wildchat-intervals.yaml
    ./extract_turn_distribution.py     wildchat.jsonl -o wildchat-turns.yaml

WildChat is shallow (mean ~2.8 turns/conv, ~47% single-turn, ~4% >=10 turns), so
single-turn conversations contribute to the turn-count distribution but yield no
interval (a lone turn has no gap) — exactly as the extractors expect.
"""

from __future__ import annotations

import argparse
import glob
import json
import sys

import pyarrow.parquet as pq

# Only the columns we need; avoids deserialising the large moderation structs.
_COLUMNS = ["conversation_hash", "model", "conversation"]


def iter_conversations(files):
    """Yield (conv_id, model, [(t_epoch, in_chars, out_chars), ...]) per conversation.

    Streams row-group by row-group so a full 838K-conversation corpus never has
    to be materialised at once. Only assistant messages (the ones that carry a
    timestamp) become turns; their running prior-context char count is tracked
    for the optional token estimate.
    """
    for fp in files:
        pf = pq.ParquetFile(fp)
        for rg in range(pf.num_row_groups):
            d = pf.read_row_group(rg, columns=_COLUMNS).to_pydict()
            for cid, model, conv in zip(
                    d["conversation_hash"], d["model"], d["conversation"]):
                turns = []
                prior_chars = 0          # user+assistant chars seen before this turn
                for m in conv or []:
                    content = m.get("content") or ""
                    role = m.get("role")
                    if role == "assistant":
                        ts = m.get("timestamp")
                        if ts is not None:
                            turns.append((ts.timestamp(), prior_chars, len(content)))
                    prior_chars += len(content)
                if turns:
                    yield cid, model, turns


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("parquet_glob",
                    help="glob for the WildChat shards, e.g. '<raw>/data/*.parquet'")
    ap.add_argument("-o", "--output", default="wildchat-ccweka.jsonl",
                    help="output JSONL (default: wildchat-ccweka.jsonl)")
    ap.add_argument("--approx-tokens", action="store_true",
                    help="also emit estimated per-turn in/out token counts "
                         "(chars // 4); rough, extractors ignore them")
    args = ap.parse_args()

    files = sorted(glob.glob(args.parquet_glob))
    if not files:
        print(f"error: no files match {args.parquet_glob!r}", file=sys.stderr)
        return 1

    n_conv = n_turn = 0
    with open(args.output, "w") as out:
        for cid, model, turns in iter_conversations(files):
            t0 = turns[0][0]
            reqs = []
            for ts, in_chars, out_chars in turns:
                r = {"t": round(ts - t0, 6)}
                if args.approx_tokens:
                    r["in"] = in_chars // 4
                    r["out"] = out_chars // 4
                reqs.append(r)
            out.write(json.dumps(
                {"id": cid, "models": [model] if model else [],
                 "requests": reqs}) + "\n")
            n_conv += 1
            n_turn += len(reqs)

    if not n_conv:
        print("error: no conversations with assistant timestamps found",
              file=sys.stderr)
        return 1
    print(f"wrote {args.output}: {n_conv} conversations, {n_turn} turns "
          f"(mean {n_turn / n_conv:.2f} turns/conv)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
