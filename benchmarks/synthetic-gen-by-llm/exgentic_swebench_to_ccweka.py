#!/usr/bin/env python3
"""Convert the Exgentic **swebench** OTel trace into the cc-weka JSONL schema.

The Exgentic ``agent-llm-traces-v2`` dataset ships OpenTelemetry-shaped execution
traces for six agent benchmarks; this tool extracts the ``swebench`` benchmark
(a SWE-agent-style coding agent on GitHub issues — long, deep, session-local
sessions) and re-emits each session in the cc-weka JSONL schema so the existing
``extract_interval_distribution.py`` / ``extract_turn_distribution.py`` run on it
unchanged.

INVOCATION MODEL (mirrors Inference-Traces-KV-Analysis/data/exgentic.py)
    One parquet row = one agent session; its ``spans`` list holds the chat LLM
    calls. A span is a model invocation iff it carries ``gen_ai.input.messages``
    (spans without a prefill are dropped). The append-only chain is strictly
    increasing in input-message count, and OTel timestamps are occasionally
    batched, so spans are ordered by ``(len(input_messages), start_time)`` — not
    by time alone. Each ordered span's ``start_time`` (ISO8601) becomes one
    cc-weka ``requests[].t`` (seconds from the session's first request).

So the emitted line is::

    {"id": <session_id>, "models": [...],
     "requests": [{"t": 0.0}, {"t": 46.27}, ...]}   # one entry per invocation

turn count  = len(requests)            (requests/turns per conversation)
interval    = t[i+1] - t[i] after sort (intra-session request inter-arrival)

DOWNLOAD
    hf download Exgentic/agent-llm-traces-v2 --repo-type dataset \\
        --local-dir <raw> --include 'data/train/*.parquet'

RUN
    ./exgentic_swebench_to_ccweka.py '<raw>/data/train/*.parquet' -o swebench.jsonl
    ./extract_interval_distribution.py swebench.jsonl -o exgentic-swebench-intervals.yaml
    ./extract_turn_distribution.py     swebench.jsonl -o exgentic-swebench-turns.yaml

Cross-check: swebench yields 1959 sessions / 91768 invocations (== the paper's
temporal_summary_v2.json observed.sessions / observed.invocations), max 259 turns.
"""

from __future__ import annotations

import argparse
import glob
import json
import sys
from datetime import datetime, timezone

import pyarrow.parquet as pq


def _epoch(ts):
    """Epoch seconds for an ISO8601 OTel timestamp (UTC if naive)."""
    dt = datetime.fromisoformat(str(ts))
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.timestamp()


def iter_sessions(files, benchmark):
    """Yield (session_id, models, [(n_input_msgs, start_time), ...]) per session."""
    for fp in files:
        t = pq.read_table(
            fp, columns=["session_id", "benchmark", "models", "spans"])
        d = t.to_pydict()
        for sid, bench, models, spans in zip(
                d["session_id"], d["benchmark"], d["models"], d["spans"]):
            if bench != benchmark:
                continue
            parsed = []
            for s in spans or []:
                im = s["attributes"].get("gen_ai.input.messages")
                if not im:
                    continue                       # no prefill -> not an invocation
                try:
                    n = len(json.loads(im))
                except (TypeError, ValueError):
                    continue
                parsed.append((n, s["start_time"]))
            if parsed:
                yield sid, list(models or []), parsed


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("parquet_glob",
                    help="glob for the dataset shards, e.g. '<raw>/data/train/*.parquet'")
    ap.add_argument("-o", "--output", default="exgentic-swebench-ccweka.jsonl",
                    help="output JSONL (default: exgentic-swebench-ccweka.jsonl)")
    ap.add_argument("--benchmark", default="swebench",
                    help="Exgentic benchmark column to extract (default: swebench)")
    args = ap.parse_args()

    files = sorted(glob.glob(args.parquet_glob))
    if not files:
        print(f"error: no files match {args.parquet_glob!r}", file=sys.stderr)
        return 1

    n_sess = n_inv = 0
    with open(args.output, "w") as out:
        for sid, models, parsed in iter_sessions(files, args.benchmark):
            # order exactly as exgentic.py: (input-message count, start_time)
            parsed.sort(key=lambda p: (p[0], p[1]))
            starts = [_epoch(st) for _, st in parsed]
            t0 = min(starts)
            reqs = [{"t": round(s - t0, 6)} for s in starts]
            out.write(json.dumps(
                {"id": sid, "models": models, "requests": reqs}) + "\n")
            n_sess += 1
            n_inv += len(reqs)

    if not n_sess:
        print(f"error: no '{args.benchmark}' sessions found", file=sys.stderr)
        return 1
    print(f"wrote {args.output}: {n_sess} sessions, {n_inv} invocations "
          f"(benchmark={args.benchmark})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
