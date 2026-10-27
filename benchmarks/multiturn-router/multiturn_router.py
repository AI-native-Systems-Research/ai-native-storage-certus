#!/usr/bin/env python3
"""Prompt-level multi-turn conversations through an OpenAI-compatible router.

Unlike apps/workload-generator (which synthesizes block keys), this drives real
conversations at the prompt level: each turn's prompt is the previous turn's
prompt, plus the model's answer *as generated* (its token ids, appended
verbatim), plus the next user message. That is what lets a turn reuse the KV a
previous turn generated, wherever it was cached -- on the pod that computed it,
or (under P/D) on the decode pod that generated the answer.

Token-exact by construction: prompts are token-id lists, tokenized once per user
message by a render endpoint (``/v1/completions/render``, the router's own
tokenizer service in the pd-certus profile), and answers come back as token ids
(``return_token_ids``). No chat template, so nothing re-renders a previous turn.

Conversations come from a ShareGPT-style JSON file (``[{"conversations":
[{"from": "human"|"gpt", "value": ...}]}]``); only the human messages are used,
the answers are the model's. Several conversations run concurrently, each turn
waiting for the previous one.

Output: one JSON line per turn (conversation, turn, prompt/answer tokens,
latency, time to first token), and a summary at the end. Pair it with
``benchmarks/synthetic-agentic/scrape_prom.py --k8s`` for the cache counters.

Example:
  multiturn_router.py --url http://10.97.175.172 --render http://10.244.2.142:8000 \\
      --conversations data/sharegpt_12turn_450.json --sessions 16 --concurrency 4 \\
      --turns 6 --answer-tokens 160 --out run.jsonl
"""

from __future__ import annotations

import argparse
import json
import random
import statistics
import sys
import threading
import time
import urllib.request


def post(url: str, body: dict, timeout: float, headers: dict | None = None):
    req = urllib.request.Request(url, json.dumps(body).encode(),
                                 {"Content-Type": "application/json", **(headers or {})})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def stream(url: str, body: dict, timeout: float, headers: dict | None = None):
    """POST a streaming completion; return (token_ids, ttft_s, total_s)."""
    body = {**body, "stream": True, "return_token_ids": True}
    req = urllib.request.Request(url, json.dumps(body).encode(),
                                 {"Content-Type": "application/json", **(headers or {})})
    t0 = time.monotonic()
    ttft = None
    ids: list[int] = []
    with urllib.request.urlopen(req, timeout=timeout) as r:
        for raw in r:
            line = raw.decode().strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            chunk = json.loads(data)
            for choice in chunk.get("choices", []):
                got = choice.get("token_ids") or []
                if got and ttft is None:
                    ttft = time.monotonic() - t0
                ids.extend(got)
    return ids, ttft, time.monotonic() - t0


class Tokenizer:
    def __init__(self, render_url: str, model: str, timeout: float):
        self.url = render_url.rstrip("/") + "/v1/completions/render"
        self.model = model
        self.timeout = timeout

    def __call__(self, text: str) -> list[int]:
        r = post(self.url, {"model": self.model, "prompt": text, "add_special_tokens": False},
                 self.timeout)
        return r[0]["token_ids"]


def human_turns(conv: dict) -> list[str]:
    return [t["value"] for t in conv["conversations"] if t.get("from") == "human" and t["value"].strip()]


def run_session(i, conv, a, tok, out, lock, results):
    sep = tok("\n\nUser: ")
    cue = tok("\n\nAssistant:")
    # The tag makes every block of this run distinct from other runs', so two
    # arms of a comparison never hit each other's cache.
    head = f"[{a.tag}] " if a.tag else ""
    prompt: list[int] = tok(head + a.system) if (a.system or head) else []
    headers = {"x-session-id": f"s{a.seed}-{i}"} if a.session_header else {}
    for turn, text in enumerate(human_turns(conv)[: a.turns]):
        prompt = prompt + sep + tok(text) + cue
        if len(prompt) + a.answer_tokens > a.max_model_len:
            break
        body = {"model": a.model, "prompt": prompt, "max_tokens": a.answer_tokens,
                "temperature": 0, "ignore_eos": True}
        # Send time (wall clock), so a turn can be matched to the router's log
        # of the pod it picked; the gateway replaces any client request id.
        sent = time.time()
        try:
            ans, ttft, total = stream(a.url.rstrip("/") + "/v1/completions", body, a.timeout, headers)
        except Exception as e:  # noqa: BLE001 - a failed turn ends its session, not the run
            rec = {"session": i, "turn": turn, "sent": round(sent, 4), "error": str(e)}
            with lock:
                out.write(json.dumps(rec) + "\n")
                out.flush()
            return
        rec = {"session": i, "turn": turn, "sent": round(sent, 4),
               "prompt_tokens": len(prompt), "answer_tokens": len(ans),
               "ttft_s": round(ttft or total, 4), "latency_s": round(total, 4)}
        with lock:
            out.write(json.dumps(rec) + "\n")
            out.flush()
            results.append(rec)
        prompt = prompt + ans


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", required=True, help="router base URL")
    ap.add_argument("--render", required=True, help="tokenizer render endpoint base URL")
    ap.add_argument("--model", default="Qwen/Qwen3-8B")
    ap.add_argument("--conversations", required=True)
    ap.add_argument("--sessions", type=int, default=16)
    ap.add_argument("--concurrency", type=int, default=4)
    ap.add_argument("--turns", type=int, default=6, help="user turns per session, at most")
    ap.add_argument("--answer-tokens", type=int, default=160, help="tokens generated per turn")
    ap.add_argument("--system", default="You are a helpful assistant. Answer carefully and completely.",
                    help="system text prepended once (empty for none)")
    ap.add_argument("--max-model-len", type=int, default=24576)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--tag", default="", help="prefix every session with this, so runs share no cache")
    ap.add_argument("--session-header", action="store_true",
                    help="send x-session-id (for session-affinity routing)")
    ap.add_argument("--timeout", type=float, default=600)
    ap.add_argument("--out", required=True, help="JSON lines, one per turn")
    a = ap.parse_args()

    convs = json.load(open(a.conversations))
    random.Random(a.seed).shuffle(convs)
    convs = [c for c in convs if len(human_turns(c)) >= 2][: a.sessions]
    tok = Tokenizer(a.render, a.model, a.timeout)
    lock = threading.Lock()
    results: list[dict] = []
    t0 = time.monotonic()
    with open(a.out, "w") as out:
        pending = list(enumerate(convs))
        threads: list[threading.Thread] = []
        while pending or threads:
            threads = [t for t in threads if t.is_alive()]
            while pending and len(threads) < a.concurrency:
                i, c = pending.pop(0)
                t = threading.Thread(target=run_session, args=(i, c, a, tok, out, lock, results))
                t.start()
                threads.append(t)
            time.sleep(0.05)
    wall = time.monotonic() - t0

    by_turn: dict[int, list[dict]] = {}
    for r in results:
        by_turn.setdefault(r["turn"], []).append(r)
    summary = {"wall_s": round(wall, 1), "turns": len(results), "sessions": len(convs),
               "per_turn": {}}
    for turn in sorted(by_turn):
        rs = by_turn[turn]
        summary["per_turn"][turn] = {
            "n": len(rs),
            "prompt_tokens_mean": round(statistics.mean(r["prompt_tokens"] for r in rs)),
            "ttft_p50_s": round(statistics.median(r["ttft_s"] for r in rs), 3),
            "latency_p50_s": round(statistics.median(r["latency_s"] for r in rs), 3),
        }
    print(json.dumps(summary, indent=1))


if __name__ == "__main__":
    sys.exit(main())
