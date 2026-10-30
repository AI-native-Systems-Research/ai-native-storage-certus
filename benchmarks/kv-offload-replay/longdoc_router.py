#!/usr/bin/env python3
"""The long-doc-qa workload, sent to an OpenAI-compatible router.

``profile_all.sh --workload long-doc-qa`` starts its own certus server and its
own vLLM engine, and drives the engine in-process. This sends the same
conversations to a serving stack that is already running -- llm-d's router,
say -- over HTTP, and starts nothing.

The conversations are the ones ``run_multiturn_common`` generates for
long-doc-qa (same ``LONGDOC_*`` knobs, same seed, so the same documents), and a
conversation advances as in the async driver: turn 0's prompt is the document
and the first question; every later turn's prompt is the previous prompt, the
model's answer, and the next question. Each turn is one streamed
``/v1/completions`` request, so a later turn's prompt starts with everything the
previous turn computed.

A turn whose prompt plus answer exceeds the served context window is refused by
the server; that conversation ends there (the in-process drivers retire it the
same way, but check before sending).

Output: one JSON line per turn (conversation, turn, prompt and answer tokens,
time to first token, latency), and a summary on stdout. ``cached_tokens`` in a
turn's line is what the answering server reported; under P/D that is the decode
pod, which reports 0 for KV it received from prefill, so it is no measure of
cache reuse there. Read the pods' Prometheus counters for that.

``--order interleaved`` sends turn k of every conversation before turn k+1 of
any, so with many conversations a working set larger than a cache tier is
evicted and read back between one conversation's turns; the default sends each
conversation's turns back to back.

Example (llm-d's router on any node's NodePort):
  LONGDOC_DOC_TOKENS=8000 LONGDOC_QUESTIONS=8 \\
      benchmarks/kv-offload-replay/longdoc_router.py --url http://node2:30080 \\
      --num-convs 8 --output-tokens 1 --out longdoc.jsonl
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import threading
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_multiturn_common as common  # noqa: E402  (stdlib-only; generates the corpus)


def get_json(url: str, timeout: float):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.load(r)


def stream(url: str, body: dict, timeout: float):
    """POST a streaming completion; return (text, usage, ttft_s, total_s)."""
    body = {**body, "stream": True, "stream_options": {"include_usage": True}}
    req = urllib.request.Request(url, json.dumps(body).encode(),
                                 {"Content-Type": "application/json"})
    t0 = time.monotonic()
    ttft = None
    text: list[str] = []
    usage = None
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
                if ttft is None:
                    ttft = time.monotonic() - t0
                text.append(choice.get("text") or "")
            if chunk.get("usage"):
                usage = chunk["usage"]
    return "".join(text), usage or {}, ttft, time.monotonic() - t0


def run_turn(i, k, human, context, a, out, lock, results):
    """Send turn ``k`` of conversation ``i`` and record it. Returns the context
    the next turn builds on, or None when the conversation ends here."""
    prompt = human if k == 0 else context + "\n\n" + human
    body = {"model": a.model, "prompt": prompt, "max_tokens": a.output_tokens,
            "temperature": a.temperature, "top_p": a.top_p}
    if a.ignore_eos:
        body["ignore_eos"] = True
    rec = {"conv": i, "turn": k, "sent": round(time.time(), 4)}
    try:
        answer, usage, ttft, total = stream(a.url.rstrip("/") + "/v1/completions",
                                            body, a.timeout)
    except urllib.error.HTTPError as e:
        # 400 is the server refusing the prompt (too long for its window):
        # the conversation is retired, the run goes on.
        rec["error"] = f"HTTP {e.code}: {e.read().decode(errors='replace')[:300]}"
        rec["retired"] = e.code == 400
    except Exception as e:  # noqa: BLE001 - a failed turn ends its conversation, not the run
        rec["error"] = str(e)
    if "error" in rec:
        with lock:
            out.write(json.dumps(rec) + "\n")
            out.flush()
            results.append(rec)
        return None
    details = usage.get("prompt_tokens_details") or {}
    rec.update({
        "prompt_tokens": usage.get("prompt_tokens"),
        "cached_tokens": details.get("cached_tokens"),
        "answer_tokens": usage.get("completion_tokens"),
        "ttft_s": round(ttft if ttft is not None else total, 4),
        "latency_s": round(total, 4),
    })
    with lock:
        out.write(json.dumps(rec) + "\n")
        out.flush()
        results.append(rec)
    return prompt + answer


def turns_of(conv, a):
    return conv[: a.turns] if a.turns else conv


def run_conv(i, conv, a, out, lock, results):
    """Back-to-back order: one conversation's turns, each after the last."""
    context = ""
    for k, human in enumerate(turns_of(conv, a)):
        context = run_turn(i, k, human, context, a, out, lock, results)
        if context is None:
            return


def run_interleaved(convs, a, concurrency, out, lock, results):
    """Round-robin order: turn k of every conversation before turn k+1 of any.

    ``concurrency`` worker threads take the next ready turn from a queue in
    conversation order; a conversation's next turn is queued only when its last
    one has answered, and goes behind every turn already waiting. So between two
    turns of one conversation, every other live conversation takes a turn, and
    the KV they store passes through the tiers in between -- which is how a
    working set larger than a tier gets its entries evicted and read back.
    """
    import collections

    ready = collections.deque((i, 0, "") for i in range(len(convs)))
    cv = threading.Condition()
    live = [len(convs)]

    def worker():
        while True:
            with cv:
                while not ready and live[0] > 0:
                    cv.wait()
                if not ready:
                    return
                i, k, context = ready.popleft()
            turns = turns_of(convs[i], a)
            nxt = run_turn(i, k, turns[k], context, a, out, lock, results)
            with cv:
                if nxt is not None and k + 1 < len(turns):
                    ready.append((i, k + 1, nxt))
                else:
                    live[0] -= 1
                cv.notify_all()

    ths = [threading.Thread(target=worker) for _ in range(concurrency)]
    for t in ths:
        t.start()
    for t in ths:
        t.join()


def pct(xs, q):
    xs = sorted(x for x in xs if x is not None)
    if not xs:
        return None
    return round(xs[min(len(xs) - 1, int(q * len(xs)))], 4)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", required=True,
                    help="router base URL, e.g. http://node2:30080")
    ap.add_argument("--model", default=None,
                    help="served model name (default: the first one /v1/models lists)")
    ap.add_argument("--num-convs", type=int, default=None,
                    help="conversations (documents) to run (default: LONGDOC_NUM_DOCS)")
    ap.add_argument("--concurrency", type=int, default=None,
                    help="conversations in flight at once (default: all of them)")
    ap.add_argument("--order", choices=("conversation", "interleaved"), default="conversation",
                    help="conversation: each conversation's turns back to back (a turn's "
                         "prefix is minutes old at most); interleaved: turn k of every "
                         "conversation before turn k+1 of any, so a large working set is "
                         "evicted from a tier and read back between a conversation's turns")
    ap.add_argument("--turns", type=int, default=0,
                    help="turns per conversation, at most (0 = all LONGDOC_QUESTIONS)")
    ap.add_argument("--output-tokens", type=int, default=150,
                    help="max_tokens per turn (the drivers' OUTPUT_TOKENS default)")
    ap.add_argument("--temperature", type=float, default=0.7)
    ap.add_argument("--top-p", type=float, default=0.95)
    ap.add_argument("--ignore-eos", action="store_true",
                    help="generate exactly --output-tokens every turn (vLLM's ignore_eos), so "
                         "runs compared against each other carry the same answer lengths")
    ap.add_argument("--timeout", type=float, default=1800,
                    help="per-request timeout, seconds")
    ap.add_argument("--out", required=True, help="JSON lines, one per turn")
    a = ap.parse_args()

    served = get_json(a.url.rstrip("/") + "/v1/models", 30)["data"]
    if a.model is None:
        a.model = served[0]["id"]
    window = next((m.get("max_model_len") for m in served if m["id"] == a.model), None)

    doc_tokens, questions, num_docs, seed = common._longdoc_params()
    n = a.num_convs if a.num_convs is not None else num_docs
    if n > num_docs:
        sys.exit(f"--num-convs {n} exceeds LONGDOC_NUM_DOCS={num_docs}")
    convs = common.load_convs(common._longdoc_generate(), n)
    concurrency = a.concurrency or len(convs)
    print(f"[longdoc] {len(convs)} conversations x {questions} turns, "
          f"~{doc_tokens}-token documents, {concurrency} at a time, {a.order} order, "
          f"model {a.model} (window {window}), {a.output_tokens} output tokens",
          file=sys.stderr, flush=True)
    if window is not None and doc_tokens + a.output_tokens > window:
        print(f"[longdoc] WARNING: a ~{doc_tokens}-token document does not fit the "
              f"served window of {window}; every conversation will be refused at turn 0",
              file=sys.stderr, flush=True)

    lock = threading.Lock()
    results: list[dict] = []
    t0 = time.monotonic()
    with open(a.out, "w") as out:
        if a.order == "interleaved":
            run_interleaved(convs, a, concurrency, out, lock, results)
        else:
            pending = list(enumerate(convs))
            threads: list[threading.Thread] = []
            while pending or threads:
                threads = [t for t in threads if t.is_alive()]
                while pending and len(threads) < concurrency:
                    i, c = pending.pop(0)
                    t = threading.Thread(target=run_conv, args=(i, c, a, out, lock, results))
                    t.start()
                    threads.append(t)
                time.sleep(0.05)
    wall = time.monotonic() - t0

    ok = [r for r in results if "error" not in r]
    later = [r for r in ok if r["turn"] > 0]
    summary = {
        "wall_s": round(wall, 1),
        "conversations": len(convs),
        "turns_ok": len(ok),
        "retired": sum(1 for r in results if r.get("retired")),
        "errors": sum(1 for r in results if "error" in r and not r.get("retired")),
        "turn0_ttft_p50_s": pct([r["ttft_s"] for r in ok if r["turn"] == 0], 0.5),
        "later_ttft_p50_s": pct([r["ttft_s"] for r in later], 0.5),
        "later_ttft_p90_s": pct([r["ttft_s"] for r in later], 0.9),
        "latency_p50_s": pct([r["latency_s"] for r in ok], 0.5),
        "per_turn": {},
    }
    for k in sorted({r["turn"] for r in ok}):
        rs = [r for r in ok if r["turn"] == k]
        summary["per_turn"][k] = {
            "n": len(rs),
            "prompt_tokens_mean": round(statistics.mean(r["prompt_tokens"] or 0 for r in rs)),
            "ttft_p50_s": pct([r["ttft_s"] for r in rs], 0.5),
        }
    print(json.dumps(summary, indent=1))
    return 1 if summary["errors"] else 0


if __name__ == "__main__":
    sys.exit(main())
