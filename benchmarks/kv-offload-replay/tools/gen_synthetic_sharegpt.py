#!/usr/bin/env python3
"""Synthetically generate a ShareGPT-format conversation dataset with the Claude API.

Produces N conversations whose *human-turn* counts are Poisson-distributed around a
mean of M, written in the exact schema the KV-offload bench consumes (see
``data/sharegpt/README.md`` and ``run_multiturn_common.load_convs``):

    [ {"id": "<str>", "conversations": [ {"from": "human"|"gpt", "value": "..."}, ... ]}, ... ]

Every conversation strictly alternates ``human`` -> ``gpt`` starting with ``human``
(so ``human turns == len(conversations) // 2``), and has >= 2 human turns, matching
what ``load_convs`` keeps.

Short conversations are authored in a single API call. Longer ones are *stitched*
across several calls (``--chunk-turns`` human turns each): the model is shown a
tail window of the most recent turns and asked to continue, so the conversation
actually reaches the requested length instead of the model winding it down early —
which single-call generation does well before ~30 turns. Human-turn counts are drawn
with a seeded RNG so a given ``--seed`` reproduces the same turn-count distribution
(the model's text still varies unless the API itself is deterministic).

Examples
--------
    # 450 conversations, mean 12 human turns (mirrors data/sharegpt_12turn_450.json)
    export ANTHROPIC_API_KEY=...   # or: ant auth login
    python tools/gen_synthetic_sharegpt.py -n 450 -m 12 -o data/synth_12turn_450.json

    # Cheaper/faster for bulk generation:
    python tools/gen_synthetic_sharegpt.py -n 2000 -m 8 --model claude-sonnet-5 \
        -o data/synth.json --concurrency 12

    # Preview the turn-count distribution + rough cost, no API calls:
    python tools/gen_synthetic_sharegpt.py -n 450 -m 12 -o /tmp/x.json --dry-run
"""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import random
import re
import signal
import string
import sys
import time
from pathlib import Path

# ─── ShareGPT schema constants ──────────────────────────────────────────────
HUMAN = "human"
GPT = "gpt"

# Output is requested as a raw JSON object rather than via the API's structured-
# output feature (output_config.format), because that feature isn't supported by
# older SDKs (0.x) or by LiteLLM-style proxy gateways. The exact turn count is
# requested in the prompt and re-asserted in post-processing (normalize_turns),
# and parse_turns tolerates markdown fences, surrounding prose, and truncation.
SYSTEM_PROMPT = (
    "You author realistic, self-contained multi-turn chat conversations between a "
    "human user and an AI assistant, for use as a benchmark dataset. Conversations "
    "must read like genuine ShareGPT logs: the human asks, follows up, refines, and "
    "changes direction naturally; the assistant answers helpfully and at realistic "
    "length (a mix of short clarifications and longer explanations, code, or lists). "
    "Do not include any meta-commentary about being synthetic.\n\n"
    'Respond with ONLY a single JSON object of the form '
    '{"turns": [{"from": "human", "value": "..."}, {"from": "gpt", "value": "..."}, ...]}. '
    "No prose before or after, and no markdown code fences."
)

# Built-in topic pool for diversity. Override with --topics-file (one topic/line).
DEFAULT_TOPICS = [
    "debugging a Python script that leaks memory",
    "planning a two-week trip to Japan on a budget",
    "explaining how TCP congestion control works",
    "writing a cover letter for a data-science role",
    "designing a REST API for a todo app",
    "understanding the causes of the French Revolution",
    "optimizing a slow SQL query with joins",
    "learning the basics of options trading",
    "refactoring a React component to use hooks",
    "meal-prep ideas for a high-protein vegetarian diet",
    "setting up CI/CD with GitHub Actions",
    "summarizing and critiquing a research paper on transformers",
    "troubleshooting a Wi-Fi router that keeps dropping",
    "drafting a fantasy short story opening",
    "comparing Rust and Go for a networking service",
    "explaining compound interest to a teenager",
    "building a personal budget spreadsheet",
    "diagnosing why a Docker container exits immediately",
    "learning conversational Spanish for travel",
    "designing a database schema for an e-commerce site",
    "improving the performance of a matplotlib plot",
    "understanding how vaccines train the immune system",
    "writing unit tests for a payment module",
    "choosing between AWS, GCP, and Azure for a startup",
    "explaining the Monty Hall problem intuitively",
    "creating a workout plan for a beginner runner",
    "parsing and validating CSV data in pandas",
    "outlining a business plan for a coffee cart",
    "explaining git rebase versus merge",
    "translating and explaining a Latin proverb",
    "setting up a home Kubernetes cluster on Raspberry Pis",
    "writing a polite email to decline a meeting",
    "understanding quantum entanglement without math",
    "tuning hyperparameters for a gradient-boosted model",
    "planning a wedding toast for a sibling",
    "securing an SSH server against brute-force attacks",
    "explaining the difference between HTTP/1.1, /2, and /3",
    "learning to read basic sheet music",
    "designing a rate limiter for a public API",
    "interpreting the results of an A/B test",
]

# Approx first-party API prices ($/1M tokens), input/output. Used only for the
# post-run cost report; unknown models are reported without a dollar figure.
PRICING = {
    "claude-opus-5": (5.0, 25.0),
    "claude-opus-4-8": (5.0, 25.0),
    "claude-opus-4-7": (5.0, 25.0),
    "claude-fable-5": (10.0, 50.0),
    "claude-sonnet-5": (2.0, 10.0),
    "claude-sonnet-4-6": (3.0, 15.0),
    "claude-haiku-4-5": (1.0, 5.0),
}


def poisson(rng: random.Random, lam: float) -> int:
    """Draw one Poisson(lam) sample using Knuth's algorithm (no numpy dependency).

    Fine for the small means (turn counts) this tool uses.
    """
    if lam <= 0:
        return 0
    l = math.exp(-lam)
    k = 0
    p = 1.0
    while True:
        k += 1
        p *= rng.random()
        if p <= l:
            return k - 1


def draw_turn_counts(n: int, mean: float, lo: int, hi: int, seed: int) -> list[int]:
    """N human-turn counts ~ Poisson(mean), each clamped to [lo, hi]."""
    rng = random.Random(seed)
    return [max(lo, min(hi, poisson(rng, mean))) for _ in range(n)]


def gen_id(rng: random.Random) -> str:
    """A ShareGPT-style id, e.g. 'xd92L6L_0' (7-char base62 + numeric suffix)."""
    alphabet = string.ascii_letters + string.digits
    base = "".join(rng.choice(alphabet) for _ in range(7))
    return f"{base}_{rng.randint(0, 30)}"


def build_user_prompt(topic: str, human_turns: int, nonce: str) -> str:
    return (
        f"Write one complete conversation about: {topic}.\n\n"
        f"It must contain exactly {human_turns} human turns, each followed by exactly "
        f"one assistant turn (so {2 * human_turns} turns total), strictly alternating "
        f"and starting with the human. The human's later turns should build on the "
        f"conversation — follow-ups, corrections, tangents, or requests for more "
        f"detail — not restart the topic. Vary assistant reply length naturally.\n\n"
        f'Respond with ONLY the JSON object {{"turns": [...]}} described in the system '
        f"prompt — no prose, no markdown fences. Diversity token: {nonce} "
        f"(use it only to vary the conversation; do not mention it)."
    )


def build_continuation_prompt(topic: str, recent_turns: list[dict],
                              want: int, nonce: str) -> str:
    """Prompt to extend an in-progress conversation by `want` more human turns.

    Only a tail window of the transcript is shown (see --context-turns), which keeps
    per-call input cost roughly constant regardless of how long the conversation has
    already grown. The model is told not to repeat earlier turns.
    """
    lines = []
    for t in recent_turns:
        who = "USER" if t["from"] == HUMAN else "ASSISTANT"
        lines.append(f"{who}: {t['value']}")
    transcript = "\n\n".join(lines)
    return (
        f"Below are the most recent turns of an ongoing conversation about {topic}:\n\n"
        f"---\n{transcript}\n---\n\n"
        f"Continue the SAME conversation naturally from where it left off. Produce the "
        f"next {want} human turns, each followed by exactly one assistant turn "
        f"({2 * want} turns total), strictly alternating and starting with the human. "
        f"The human should build on what was already said — follow-ups, corrections, "
        f"deeper questions, or related tangents — and must NOT repeat or restate any "
        f"earlier turn. Vary assistant reply length naturally.\n\n"
        f'Respond with ONLY the JSON object {{"turns": [...]}} described in the system '
        f"prompt — no prose, no markdown fences. Diversity token: {nonce} "
        f"(use it only to vary the conversation; do not mention it)."
    )


def parse_turns(text: str) -> list[dict]:
    """Extract the list of turn dicts from a model response, tolerantly.

    Handles: clean JSON, JSON wrapped in ```json fences or surrounding prose, and
    output truncated mid-array (e.g. hit max_tokens) — in which case every complete
    ``{"from": ..., "value": ...}`` object up to the break is salvaged.
    """
    s = text.strip()
    # Strip a leading/trailing markdown code fence if present.
    if s.startswith("```"):
        s = re.sub(r"^```[a-zA-Z0-9]*\s*\n?", "", s)
        s = re.sub(r"\n?```\s*$", "", s).strip()

    # strict=False lets literal newlines/tabs inside string values through — LLMs
    # routinely emit multi-line code blocks and lists that strict JSON rejects.
    dec = json.JSONDecoder(strict=False)

    # Fast path: a complete JSON object with a "turns" array.
    lb, rb = s.find("{"), s.rfind("}")
    if lb != -1 and rb > lb:
        try:
            obj = dec.decode(s[lb:rb + 1])
            if isinstance(obj, dict) and isinstance(obj.get("turns"), list):
                return obj["turns"]
        except json.JSONDecodeError:
            pass

    # Salvage path: scan the turns array and decode complete objects one at a time,
    # stopping at the first incomplete one (the truncation point).
    key = s.find('"turns"')
    start = s.find("[", key if key != -1 else 0)
    if start == -1:
        return []
    turns, idx, n = [], start + 1, len(s)
    while idx < n:
        while idx < n and s[idx] in " \t\r\n,":
            idx += 1
        if idx >= n or s[idx] == "]":
            break
        try:
            obj, idx = dec.raw_decode(s, idx)
        except json.JSONDecodeError:
            break  # incomplete trailing object → stop
        if isinstance(obj, dict) and "value" in obj:
            turns.append(obj)
    return turns


def normalize_turns(raw_turns: list[dict], target_human: int) -> list[dict]:
    """Coerce model output into clean, strictly-alternating human/gpt turns.

    Roles are re-asserted by position (even -> human, odd -> gpt) so the result is
    always valid regardless of how the model labeled them. Empty-valued turns are
    dropped first. The list is trimmed to an even length (ends on a gpt turn) and,
    if longer than requested, capped at the target.
    """
    values = [str(t.get("value", "")).strip() for t in raw_turns]
    values = [v for v in values if v]
    # Cap at the requested number of turns if the model over-produced.
    max_turns = 2 * target_human
    if len(values) > max_turns:
        values = values[:max_turns]
    # Trim to even length so it ends on an assistant turn.
    if len(values) % 2 == 1:
        values = values[:-1]
    return [
        {"from": HUMAN if i % 2 == 0 else GPT, "value": v}
        for i, v in enumerate(values)
    ]


# Rough per-message output-token budget, used to size max_tokens from turn count
# so long conversations don't get truncated mid-array.
TOKENS_PER_MSG = 350
MAX_TOKENS_CAP = 64000


def conv_max_tokens(human_turns: int, floor: int) -> int:
    """max_tokens for a conversation: at least `floor`, scaled up for many turns."""
    need = human_turns * 2 * TOKENS_PER_MSG + 1500
    return min(MAX_TOKENS_CAP, max(floor, need))


async def _stream_message(client, model, max_tokens, user_content):
    """One streamed messages call → the final Message.

    Streaming is required: at high per-call max_tokens the SDK refuses a
    non-streaming request (>10-min risk), and it avoids request timeouts on long
    generations.
    """
    async with client.messages.stream(
        model=model,
        max_tokens=max_tokens,
        system=SYSTEM_PROMPT,
        messages=[{"role": "user", "content": user_content}],
    ) as stream:
        return await stream.get_final_message()


async def gen_one(client, sem, model, max_tokens_floor, topic, human_turns,
                  conv_id, nonce, counts, chunk_turns, context_turns):
    """Generate a single conversation, stitching across calls to reach the target.

    A conversation needing more than `chunk_turns` human turns is built in chunks:
    the first call seeds it, each later call is shown a tail window of the last
    `context_turns` human turns and asked to continue. Returns a ShareGPT record, or
    None if nothing usable was produced.
    """
    async with sem:
        turns: list[dict] = []
        stalls = 0
        # Bound total calls so an under-producing model can't loop forever.
        max_calls = math.ceil(human_turns / max(1, chunk_turns)) + 3
        calls = 0

        while (len(turns) // 2) < human_turns and calls < max_calls:
            calls += 1
            want = min(chunk_turns, human_turns - len(turns) // 2)
            if not turns:
                user_content = build_user_prompt(topic, want, nonce)
            else:
                recent = turns[-2 * context_turns:]
                user_content = build_continuation_prompt(
                    topic, recent, want, f"{nonce}-{calls}")

            try:
                resp = await _stream_message(
                    client, model, conv_max_tokens(want, max_tokens_floor),
                    user_content)
            except Exception as e:  # noqa: BLE001 - one bad call shouldn't kill the run
                if len(turns) >= 4:  # keep the partial conversation we already have
                    print(f"  [gen] partial id={conv_id}: {type(e).__name__}: {e} "
                          f"— keeping {len(turns) // 2} h-turns", file=sys.stderr)
                    break
                counts["failed"] += 1
                print(f"  [gen] FAILED id={conv_id}: {type(e).__name__}: {e}",
                      file=sys.stderr)
                return None

            if resp.stop_reason == "refusal":
                if not turns:
                    counts["refused"] += 1
                    print(f"  [gen] refused id={conv_id} topic={topic!r}",
                          file=sys.stderr)
                    return None
                break  # refused a continuation → keep what we have

            u = resp.usage
            counts["in_tokens"] += getattr(u, "input_tokens", 0) or 0
            counts["out_tokens"] += getattr(u, "output_tokens", 0) or 0
            counts["api_calls"] += 1

            text = next((b.text for b in resp.content if b.type == "text"), None)
            # parse_turns salvages complete turns even if the response was truncated.
            new = normalize_turns(parse_turns(text), want) if text else []
            if resp.stop_reason == "max_tokens":
                counts["truncated"] += 1

            if not new:
                stalls += 1
                if stalls >= 2:  # two barren calls in a row → give up on this conv
                    break
                continue
            stalls = 0
            turns.extend(new)
            # A short-but-nonempty chunk (the model commonly returns ~10 for a
            # requested 12) is normal, not a wind-down signal: keep going until the
            # target is reached, max_calls is hit, or two consecutive barren calls.

        if len(turns) < 4:  # < 2 human turns => load_convs would drop it
            counts["too_short"] += 1
            print(f"  [gen] too short id={conv_id} ({len(turns)} turns)",
                  file=sys.stderr)
            return None

        counts["ok"] += 1
        counts["realized_human_turns"] += len(turns) // 2
        if counts["ok"] % 25 == 0:
            print(f"  [gen] ok={counts['ok']} failed={counts['failed']} "
                  f"refused={counts['refused']} truncated={counts['truncated']}",
                  file=sys.stderr)
        return {"id": conv_id, "conversations": turns}


def write_out(path: Path, records: list, indent):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        if indent is None:
            json.dump(records, fh, ensure_ascii=False, separators=(",", ":"))
        else:
            json.dump(records, fh, ensure_ascii=False, indent=indent)


def print_stats(counts, turn_counts, model, elapsed):
    realized_mean = (counts["realized_human_turns"] / counts["ok"]) if counts["ok"] else 0
    requested_mean = sum(turn_counts) / len(turn_counts) if turn_counts else 0
    print("\n[gen] stats:", file=sys.stderr)
    for k in ("ok", "failed", "refused", "too_short", "truncated"):
        print(f"  {k:<22} {counts[k]:>8,}", file=sys.stderr)
    if counts["truncated"]:
        print("  (truncated = hit max_tokens; kept but shorter than requested — "
              "raise --max-tokens or lower -m)", file=sys.stderr)
    print(f"  requested mean h-turns {requested_mean:>8.2f}", file=sys.stderr)
    print(f"  realized  mean h-turns {realized_mean:>8.2f}", file=sys.stderr)
    if counts.get("api_calls"):
        print(f"  api calls             {counts['api_calls']:>8,}", file=sys.stderr)
    print(f"  input tokens          {counts['in_tokens']:>8,}", file=sys.stderr)
    print(f"  output tokens         {counts['out_tokens']:>8,}", file=sys.stderr)
    if model in PRICING:
        pi, po = PRICING[model]
        cost = counts["in_tokens"] / 1e6 * pi + counts["out_tokens"] / 1e6 * po
        print(f"  est. cost ({model})   ${cost:>7.2f}", file=sys.stderr)
    print(f"  wall time             {elapsed:>7.1f}s", file=sys.stderr)


async def run(args) -> int:
    turn_counts = draw_turn_counts(
        args.num_convs, args.mean_turns, args.min_turns, args.max_turns, args.seed)

    # Load topic pool.
    if args.topics_file:
        topics = [ln.strip() for ln in Path(args.topics_file).read_text(
            encoding="utf-8").splitlines() if ln.strip()]
        if not topics:
            print(f"[gen] no topics in {args.topics_file}", file=sys.stderr)
            return 1
    else:
        topics = DEFAULT_TOPICS

    id_rng = random.Random(args.seed ^ 0x5EED)
    topic_rng = random.Random(args.seed ^ 0x709C)

    if args.dry_run:
        from collections import Counter
        dist = Counter(turn_counts)
        print(f"[gen] DRY RUN — no API calls. model={args.model}", file=sys.stderr)
        print(f"[gen] {args.num_convs} convs, requested mean h-turns="
              f"{sum(turn_counts)/len(turn_counts):.2f} "
              f"(range {min(turn_counts)}–{max(turn_counts)})", file=sys.stderr)
        print("[gen] human-turn distribution:", file=sys.stderr)
        for k in sorted(dist):
            bar = "#" * min(60, dist[k])
            print(f"  {k:>3} turns: {dist[k]:>5}  {bar}", file=sys.stderr)
        total_turns = sum(turn_counts)
        # Account for chunked stitching: a conv of H human turns takes
        # ceil(H / chunk_turns) calls; every call after the first re-sends a
        # tail window of ~context_turns human turns as input context.
        calls = sum(math.ceil(ht / max(1, args.chunk_turns)) for ht in turn_counts)
        cont_calls = max(0, calls - args.num_convs)
        # Very rough: ~250 output tokens/turn; ~500 input tokens of prompt/system
        # per call, plus ~300 tokens per context message on continuation calls.
        est_out = total_turns * 250
        est_in = calls * 500 + cont_calls * args.context_turns * 2 * 300
        note = ""
        if args.model in PRICING:
            pi, po = PRICING[args.model]
            note = f" → est. ${est_in/1e6*pi + est_out/1e6*po:.2f}"
        print(f"[gen] ~{calls:,} API calls ({cont_calls:,} continuations), "
              f"chunk={args.chunk_turns} h-turns/call", file=sys.stderr)
        print(f"[gen] rough token estimate: ~{est_in:,} in / ~{est_out:,} out{note}",
              file=sys.stderr)
        print("[gen] NOTE: output-token estimate is rough; a small validation run "
              "gives the real per-conv cost before a large batch.", file=sys.stderr)
        return 0

    try:
        from anthropic import AsyncAnthropic
    except ImportError:
        print("[gen] `pip install anthropic` required", file=sys.stderr)
        return 1

    client = AsyncAnthropic()
    sem = asyncio.Semaphore(args.concurrency)
    counts = {"ok": 0, "failed": 0, "refused": 0, "too_short": 0, "truncated": 0,
              "api_calls": 0, "in_tokens": 0, "out_tokens": 0,
              "realized_human_turns": 0}

    tasks = []
    for i, ht in enumerate(turn_counts):
        topic = topic_rng.choice(topics)
        nonce = f"{args.seed}-{i}"
        tasks.append(gen_one(
            client, sem, args.model, args.max_tokens,
            topic, ht, gen_id(id_rng), nonce, counts,
            args.chunk_turns, args.context_turns))

    print(f"[gen] generating {args.num_convs} conversations with {args.model} "
          f"(concurrency={args.concurrency}, mean h-turns={args.mean_turns}) …",
          file=sys.stderr)
    if args.model.startswith("claude-opus") or args.model.startswith("claude-fable"):
        print("[gen] note: for large bulk runs, --model claude-sonnet-5 or "
              "claude-haiku-4-5 is substantially cheaper/faster.", file=sys.stderr)

    start = time.time()
    records = []
    try:
        for coro in asyncio.as_completed(tasks):
            rec = await coro
            if rec is not None:
                records.append(rec)
    finally:
        # Always write whatever succeeded, even on Ctrl-C or a mid-run error.
        write_out(args.out, records, args.indent)
        print(f"\n[gen] wrote {len(records):,} conversations → {args.out}",
              file=sys.stderr)
        print_stats(counts, turn_counts, args.model, time.time() - start)

    return 0 if records else 2


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-n", "--num-convs", type=int, required=True,
                    help="number of conversations to generate (N)")
    ap.add_argument("-m", "--mean-turns", type=float, required=True,
                    help="mean HUMAN-turn count per conversation (M); "
                         "actual counts are Poisson-distributed around this")
    ap.add_argument("-o", "--out", type=Path, required=True,
                    help="output ShareGPT-format .json path")
    ap.add_argument("--model", default="claude-opus-5",
                    help="Claude model id (default: claude-opus-5; "
                         "claude-sonnet-5 / claude-haiku-4-5 are cheaper for bulk)")
    ap.add_argument("--concurrency", type=int, default=8,
                    help="max concurrent API requests (default: 8)")
    ap.add_argument("--max-tokens", type=int, default=16000,
                    help="floor for max output tokens per conversation (default: "
                         "16000); automatically raised for high turn counts, capped "
                         f"at {MAX_TOKENS_CAP}")
    ap.add_argument("--min-turns", type=int, default=2,
                    help="floor for human-turn count (default: 2 — the bench "
                         "minimum; conversations with fewer are dropped by load_convs)")
    ap.add_argument("--max-turns", type=int, default=40,
                    help="ceiling for human-turn count (default: 40)")
    ap.add_argument("--chunk-turns", type=int, default=12,
                    help="max human turns generated per API call (default: 12); "
                         "conversations longer than this are stitched across "
                         "multiple calls so they actually reach the target length")
    ap.add_argument("--context-turns", type=int, default=3,
                    help="how many recent human turns to re-send as context on each "
                         "continuation call (default: 3); larger = more coherent but "
                         "more input tokens")
    ap.add_argument("--seed", type=int, default=1234,
                    help="RNG seed for the turn-count distribution, ids, and topic "
                         "selection (default: 1234)")
    ap.add_argument("--topics-file", type=str, default=None,
                    help="optional file of topics, one per line (default: built-in pool)")
    ap.add_argument("--indent", type=int, default=None,
                    help="pretty-print JSON with this indent (default: compact, "
                         "matching the repo's sharegpt files)")
    ap.add_argument("--dry-run", action="store_true",
                    help="print the turn-count distribution and a rough cost "
                         "estimate without calling the API")
    args = ap.parse_args()

    if args.num_convs < 1:
        print("[gen] --num-convs must be >= 1", file=sys.stderr)
        return 1
    if args.mean_turns <= 0:
        print("[gen] --mean-turns must be > 0", file=sys.stderr)
        return 1
    if args.concurrency < 1:
        print("[gen] --concurrency must be >= 1", file=sys.stderr)
        return 1
    if args.max_tokens < 1:
        print("[gen] --max-tokens must be >= 1", file=sys.stderr)
        return 1
    if args.chunk_turns < 1:
        print("[gen] --chunk-turns must be >= 1", file=sys.stderr)
        return 1
    if args.context_turns < 0:
        print("[gen] --context-turns must be >= 0", file=sys.stderr)
        return 1
    if args.indent is not None and args.indent < 0:
        print("[gen] --indent must be >= 0", file=sys.stderr)
        return 1

    if args.min_turns < 2:
        print("[gen] --min-turns clamped to 2 (bench requires >= 2 human turns)",
              file=sys.stderr)
        args.min_turns = 2
    if args.max_turns < args.min_turns:
        print("[gen] --max-turns must be >= --min-turns", file=sys.stderr)
        return 1

    # Make Ctrl-C raise so the `finally` block flushes partial output cleanly.
    signal.signal(signal.SIGINT, signal.default_int_handler)
    try:
        return asyncio.run(run(args))
    except KeyboardInterrupt:
        print("\n[gen] interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
