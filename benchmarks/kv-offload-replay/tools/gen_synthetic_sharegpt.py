#!/usr/bin/env python3
"""Synthetically generate a ShareGPT-format conversation dataset with the Claude API.

Produces N conversations whose *human-turn* counts are Poisson-distributed around a
mean of M, written in the exact schema the KV-offload bench consumes (see
``data/sharegpt/README.md`` and ``run_multiturn_common.load_convs``):

    [ {"id": "<str>", "conversations": [ {"from": "human"|"gpt", "value": "..."}, ... ]}, ... ]

Every conversation strictly alternates ``human`` -> ``gpt`` starting with ``human``
(so ``human turns == len(conversations) // 2``), and has >= 2 human turns, matching
what ``load_convs`` keeps.

Each conversation is authored in a *single* structured-output API call (not one call
per turn), which is ~M times cheaper and faster. Human-turn counts are drawn with a
seeded RNG so a given ``--seed`` reproduces the same turn-count distribution (the
model's text still varies unless the API itself is deterministic).

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
import signal
import string
import sys
import time
from pathlib import Path

# ─── ShareGPT schema constants ──────────────────────────────────────────────
HUMAN = "human"
GPT = "gpt"

# json_schema for one conversation's turn list. Exact count is requested in the
# prompt and re-asserted in post-processing; the schema just guarantees shape.
TURN_SCHEMA = {
    "type": "object",
    "properties": {
        "turns": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "from": {"type": "string", "enum": [HUMAN, GPT]},
                    "value": {"type": "string"},
                },
                "required": ["from", "value"],
                "additionalProperties": False,
            },
        }
    },
    "required": ["turns"],
    "additionalProperties": False,
}

SYSTEM_PROMPT = (
    "You author realistic, self-contained multi-turn chat conversations between a "
    "human user and an AI assistant, for use as a benchmark dataset. Conversations "
    "must read like genuine ShareGPT logs: the human asks, follows up, refines, and "
    "changes direction naturally; the assistant answers helpfully and at realistic "
    "length (a mix of short clarifications and longer explanations, code, or lists). "
    "Do not include any meta-commentary about being synthetic. Return only the "
    "structured turns."
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
        f"Return the turns via the structured schema. Diversity token: {nonce} "
        f"(use it only to vary the conversation; do not mention it)."
    )


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


async def gen_one(client, sem, model, effort, max_tokens, topic, human_turns,
                  conv_id, nonce, counts):
    """Generate a single conversation; returns a ShareGPT record or None on failure."""
    async with sem:
        try:
            resp = await client.messages.create(
                model=model,
                max_tokens=max_tokens,
                system=SYSTEM_PROMPT,
                output_config={
                    "effort": effort,
                    "format": {"type": "json_schema", "schema": TURN_SCHEMA},
                },
                messages=[{
                    "role": "user",
                    "content": build_user_prompt(topic, human_turns, nonce),
                }],
            )
        except Exception as e:  # noqa: BLE001 - one bad conv shouldn't kill the run
            counts["failed"] += 1
            print(f"  [gen] FAILED id={conv_id}: {type(e).__name__}: {e}",
                  file=sys.stderr)
            return None

        if resp.stop_reason == "refusal":
            counts["refused"] += 1
            print(f"  [gen] refused id={conv_id} topic={topic!r}", file=sys.stderr)
            return None

        # usage accounting
        u = resp.usage
        counts["in_tokens"] += getattr(u, "input_tokens", 0) or 0
        counts["out_tokens"] += getattr(u, "output_tokens", 0) or 0

        text = next((b.text for b in resp.content if b.type == "text"), None)
        if not text:
            counts["failed"] += 1
            print(f"  [gen] empty id={conv_id}", file=sys.stderr)
            return None
        try:
            turns = normalize_turns(json.loads(text)["turns"], human_turns)
        except (json.JSONDecodeError, KeyError, TypeError) as e:
            counts["failed"] += 1
            print(f"  [gen] unparseable id={conv_id}: {e}", file=sys.stderr)
            return None

        if len(turns) < 4:  # < 2 human turns => load_convs would drop it
            counts["too_short"] += 1
            print(f"  [gen] too short id={conv_id} ({len(turns)} turns)",
                  file=sys.stderr)
            return None

        counts["ok"] += 1
        counts["realized_human_turns"] += len(turns) // 2
        if counts["ok"] % 25 == 0:
            print(f"  [gen] ok={counts['ok']} failed={counts['failed']} "
                  f"refused={counts['refused']}", file=sys.stderr)
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
    for k in ("ok", "failed", "refused", "too_short"):
        print(f"  {k:<22} {counts[k]:>8,}", file=sys.stderr)
    print(f"  requested mean h-turns {requested_mean:>8.2f}", file=sys.stderr)
    print(f"  realized  mean h-turns {realized_mean:>8.2f}", file=sys.stderr)
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
        # Very rough: ~250 output tokens/turn, ~600 input tokens/call (prompt+schema).
        est_out = total_turns * 250
        est_in = args.num_convs * 600
        note = ""
        if args.model in PRICING:
            pi, po = PRICING[args.model]
            note = f" → est. ${est_in/1e6*pi + est_out/1e6*po:.2f}"
        print(f"[gen] rough token estimate: ~{est_in:,} in / ~{est_out:,} out{note}",
              file=sys.stderr)
        return 0

    try:
        from anthropic import AsyncAnthropic
    except ImportError:
        print("[gen] `pip install anthropic` required", file=sys.stderr)
        return 1

    client = AsyncAnthropic()
    sem = asyncio.Semaphore(args.concurrency)
    counts = {"ok": 0, "failed": 0, "refused": 0, "too_short": 0,
              "in_tokens": 0, "out_tokens": 0, "realized_human_turns": 0}

    tasks = []
    for i, ht in enumerate(turn_counts):
        topic = topic_rng.choice(topics)
        nonce = f"{args.seed}-{i}"
        tasks.append(gen_one(
            client, sem, args.model, args.effort, args.max_tokens,
            topic, ht, gen_id(id_rng), nonce, counts))

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
    ap.add_argument("--effort", default="low",
                    choices=["low", "medium", "high", "xhigh", "max"],
                    help="reasoning effort (default: low — generation isn't "
                         "reasoning-heavy; higher = better prose, more cost)")
    ap.add_argument("--concurrency", type=int, default=8,
                    help="max concurrent API requests (default: 8)")
    ap.add_argument("--max-tokens", type=int, default=16000,
                    help="max output tokens per conversation (default: 16000)")
    ap.add_argument("--min-turns", type=int, default=2,
                    help="floor for human-turn count (default: 2 — the bench "
                         "minimum; conversations with fewer are dropped by load_convs)")
    ap.add_argument("--max-turns", type=int, default=40,
                    help="ceiling for human-turn count (default: 40)")
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
