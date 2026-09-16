"""otel_corpus.py — load the `otel_trace_replay` corpus (benchmarks/kv-offload-otel-replay/trace-gen/trace_to_otel.py)
into the per-conversation turn streams the OTel replay drivers consume.

Shared by every OTel-replay backend driver (``run_otel_replay.py`` for the
OffloadingConnector family, ``certus-shmq-connector/run_otel_shmq_certus.py`` for
the shmq path) so the corpus-format parsing lives in exactly one place — the
connector-agnostic analogue of ``run_multiturn_common.load_convs`` for ShareGPT.

Each file in the corpus is one conversation (one replay session); its spans, in
trace order, are its turns. For in-process concatenation replay we keep ONLY the
new user turn text per span (``gen_ai.input.messages[-1]`` with role user) —
history is rebuilt from vLLM's own generations at replay — plus the per-turn
``max_tokens`` and the inter-turn gap ``start_time[k] - end_time[k-1]`` (the
recorded tool-call / think delay). Keeping just the last user message (not each
span's full accumulated history) makes the retained size LINEAR in turns rather
than quadratic, so loading the whole 1000-file corpus costs ~0.8 GB rather than
its ~18 GB on-disk footprint.
"""

import glob
import json
import os
import sys
from datetime import datetime


def _parse_iso(s):
    return datetime.fromisoformat(s)


def load_otel_convs(trace_dir, num_convs=None):
    """Load the OTel corpus at ``trace_dir`` into a list of conversations.

    Files are read in sorted name order; ``num_convs`` (falsy = all) caps how
    many files are read. Each returned conversation is a list of turn tuples
    ``(human_text, max_tokens, delay_before_sec)`` in trace order —
    ``delay_before_sec`` is 0.0 for the first turn and
    ``max(0, start[k] - end[k-1])`` seconds for later turns.

    Exits the process if ``trace_dir`` is not a directory (a misconfigured corpus
    path is a fatal setup error, not something to silently replay as empty).
    """
    if not os.path.isdir(trace_dir):
        print(f"[otel] missing corpus dir {trace_dir}", file=sys.stderr)
        sys.exit(1)
    paths = sorted(glob.glob(os.path.join(trace_dir, "*.json")))
    if num_convs:
        paths = paths[:num_convs]

    convs = []
    skipped = 0
    for path in paths:
        with open(path) as f:
            data = json.load(f)
        spans = sorted(data.get("spans", []),
                       key=lambda s: s.get("start_time", ""))
        turns = []
        prev_end = None
        for sp in spans:
            attrs = sp.get("attributes", {})
            raw = attrs.get("gen_ai.input.messages")
            if not raw:
                continue
            try:
                msgs = json.loads(raw)
            except (TypeError, json.JSONDecodeError):
                continue
            # The current user turn is the last user-role message.
            human = next((m.get("content", "") for m in reversed(msgs)
                          if m.get("role") == "user"), None)
            if human is None:
                continue
            max_tok = int(attrs.get("gen_ai.request.max_tokens")
                          or attrs.get("gen_ai.usage.output_tokens") or 0)
            start = _parse_iso(sp["start_time"])
            end = _parse_iso(sp.get("end_time", sp["start_time"]))
            delay = 0.0
            if prev_end is not None:
                delay = max(0.0, (start - prev_end).total_seconds())
            turns.append((human, max_tok, delay))
            prev_end = end
        if turns:
            convs.append(turns)
        else:
            skipped += 1
    if skipped:
        print(f"[otel] skipped {skipped} files with no usable spans",
              file=sys.stderr)
    return convs
