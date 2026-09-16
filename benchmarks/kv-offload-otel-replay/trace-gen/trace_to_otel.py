#!/usr/bin/env python3.12
"""Convert a conversation_replay trace (workload PLAN) into an OTel trace-file
corpus that inference-perf's `otel_trace_replay` datagen can replay from disk.

WHY THIS SHAPE
--------------
`otel_trace_replay` is *one conversation per file*: the loader maps each JSON
file -> one dataset row -> one replay session (spans in a file are one session's
call graph; it does NOT split by trace_id). So N conversations => N files in a
directory, replayed as N concurrent sessions via `load.type: trace_session_replay`.

Each turn becomes one LLM span. The replayer SENDS the recorded user/"shared"
message text on the wire (assistant "output" segments are substituted at replay
with the real predecessor generations), and takes token *sizes* from the
`gen_ai.usage.*` attributes. Therefore:
  * user messages carry real token-sized filler (Qwen-tokenizer random text),
  * assistant messages are short markers (substituted at replay; their real size
    is carried by gen_ai.usage.output_tokens),
  * inter-turn delay is encoded as the timestamp GAP between consecutive spans
    (build_graph: wait_ms = start_i - end_{i-1}).

CONTEXT CAP (option B)
----------------------
conversation_replay accumulates context every turn with no compaction, so deep
turns blow past any real context window (this workload peaks ~289k tokens). We
apply a sliding window: keep the system prompt + most-recent turns so the
ACCOUNTED prompt (system + kept user inputs + kept outputs + current user) never
exceeds --cap tokens. Default 120000 leaves headroom under Qwen2.5's 131072.

Usage:
    python3.12 trace_to_otel.py <trace.json> <out_dir> \
        [--num 1000] [--cap 120000] [--model Qwen/Qwen2.5-7B-Instruct] \
        [--seed 7] [--validate]
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
import warnings
from collections import deque
from datetime import datetime, timedelta, timezone

warnings.filterwarnings("ignore")
import numpy as np
from transformers import AutoTokenizer

BASE = datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc)


def gen_text(tok, rng, n_tokens: int) -> str:
    """Random text that Qwen tokenizes to ~n_tokens (within a few %)."""
    if n_tokens <= 0:
        return ""
    ids = rng.integers(0, tok.vocab_size, size=int(n_tokens)).tolist()
    return tok.decode(ids, skip_special_tokens=True)


def iso(dt: datetime) -> str:
    return dt.isoformat()


def build_conversation(conv, sys_text, sys_tok, tok, rng, cap, model):
    """Return (spans, stats) for one conversation with sliding-window context."""
    cid = conv["id"]
    trace_id = f"conv_{cid:05d}"
    turns = conv["turns"]  # [[input_tokens, output_tokens, latency_sec], ...]

    history = deque()  # entries: dict(user_dict, user_tok, asst_dict, out_tok)
    spans = []
    t = BASE
    max_span_tok = 0

    for i, (in_tok, out_tok, lat) in enumerate(turns):
        user_text = gen_text(tok, rng, in_tok)
        user_dict = {"role": "user", "content": user_text}

        # Sliding window: drop oldest completed pairs until the accounted prompt
        # (system + kept pairs' user+output + current user) fits under cap.
        def accounted():
            return sys_tok + sum(h["user_tok"] + h["out_tok"] for h in history) + in_tok

        while history and accounted() > cap:
            history.popleft()

        # Assemble the wire message list for this turn.
        messages = [{"role": "system", "content": sys_text}]
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

        spans.append(
            {
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
            }
        )

        # Advance clock: gap before next turn = this turn's tool-call latency.
        t = end + timedelta(seconds=float(lat))
        history.append({"user_dict": user_dict, "user_tok": int(in_tok), "asst_dict": {"role": "assistant", "content": asst_marker}, "out_tok": int(out_tok)})

    trace = {
        "trace_id": trace_id,
        "span_count": len(spans),
        "collected_at": iso(BASE),
        "spans": spans,
    }
    return trace, max_span_tok


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("trace")
    ap.add_argument("out_dir")
    ap.add_argument("--num", type=int, default=1000)
    ap.add_argument("--cap", type=int, default=120000)
    ap.add_argument("--model", default="Qwen/Qwen2.5-7B-Instruct")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--validate", action="store_true", help="load the first file back through inference-perf's OTel loader")
    args = ap.parse_args()

    with open(args.trace) as f:
        trace = json.load(f)
    convs = trace["conversations"][: args.num]
    sys_tok = trace["metadata"]["shared_system_prompt_len"]

    os.makedirs(args.out_dir, exist_ok=True)
    tok = AutoTokenizer.from_pretrained(args.model)
    rng = np.random.default_rng(args.seed)

    # One shared system prompt reused by every span of every conversation
    # (the shared prefix -> prefix-cache + offload pressure).
    sys_text = gen_text(tok, rng, sys_tok)

    t0 = time.time()
    total_spans = 0
    total_bytes = 0
    global_max_tok = 0
    for k, conv in enumerate(convs):
        trace_obj, max_tok = build_conversation(conv, sys_text, sys_tok, tok, rng, args.cap, args.model)
        global_max_tok = max(global_max_tok, max_tok)
        total_spans += trace_obj["span_count"]
        path = os.path.join(args.out_dir, f"{trace_obj['trace_id']}.json")
        with open(path, "w", encoding="utf-8") as f:
            s = json.dumps(trace_obj, ensure_ascii=False)
            f.write(s)
            total_bytes += len(s)
        if (k + 1) % 100 == 0:
            el = time.time() - t0
            print(f"  {k+1}/{len(convs)} convs  {total_spans} spans  {total_bytes/1e9:.2f} GB  {el:.0f}s", flush=True)

    print(f"\nwrote {len(convs)} files -> {args.out_dir}")
    print(f"total spans          : {total_spans:,}")
    print(f"on-disk size         : {total_bytes/1e9:.2f} GB")
    print(f"max accounted span   : {global_max_tok:,} tokens (cap {args.cap}, model ctx 131072)")
    print(f"elapsed              : {time.time()-t0:.0f}s")

    if args.validate:
        print("\n--- validating first file through inference-perf OTel loader ---")
        from inference_perf.datagen.otel_trace_to_replay_graph import build_raw_calls, build_graph

        p = os.path.join(args.out_dir, f"conv_{convs[0]['id']:05d}.json")
        data = json.load(open(p))
        raw_calls, _ = build_raw_calls(data["spans"], include_errors=False)
        graph = build_graph(raw_calls, source_file=p)
        print(f"file                 : {p}")
        print(f"spans -> raw_calls   : {len(data['spans'])} -> {len(raw_calls)}")
        print(f"graph events         : {len(graph.events)}  roots: {len(graph.root_event_ids)}")
        # Inspect a mid-conversation event's segments + wait
        eids = list(graph.events.keys())
        mid = graph.events[eids[len(eids) // 2]]
        seg_types = [(s.type, s.message_count, s.token_count) for s in mid.call.input_segments]
        print(f"mid event            : {mid.event_id}")
        print(f"  preds/deps         : {mid.predecessor_dependency_types}")
        print(f"  wait_ms            : {mid.wait_ms}")
        print(f"  total_input_tokens : {mid.call.total_input_tokens}")
        print(f"  segments           : {seg_types}")


if __name__ == "__main__":
    main()
