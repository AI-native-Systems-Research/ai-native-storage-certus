#!/usr/bin/env python3
"""Extract turn- and interval-distribution YAMLs from the uw-syfi/TraceLab
``syfi_coding_trace`` (v0.0.2 release).

The trace is one JSON object per *agent step* (a ``round``). Each record carries a
``session_id`` (the conversation), and a ``timing_events`` list whose earliest
timestamp marks when that step/request arrived. We treat each step as one cc-weka
request, keyed by ``session_id`` and ordered by that earliest timestamp, so:

  * turns per conversation = number of steps sharing a ``session_id``;
  * an interval = gap (seconds) between consecutive steps within one conversation
    (``num_intervals == total_steps - num_sessions``).

Output YAMLs match the format consumed by ``kv_cache_dynamics.py`` /
``dram_dashboard.py``: 20 geometric buckets with ``round(x, 6)`` edges and
fractions. Turn buckets span ``[min, max]``; interval buckets span ``[1e-6, max]``
(intervals can be exactly 0, so sub-microsecond gaps clamp into bucket 0).

Usage:
    ./syfi_coding_dist.py syfi_coding_trace.jsonl.gz \
        --turns-out syfi-coding-turns.yaml \
        --intervals-out syfi-coding-intervals.yaml
"""
import argparse
import collections
import gzip
import io
import json
from datetime import datetime, timezone

import numpy as np
import yaml

NUM_BUCKETS = 20
SRC = ("uw-syfi/TraceLab v0.0.2 release (syfi_coding_trace.jsonl.gz, "
       "{sessions} sessions / {steps} agent steps / {users} users); one agent "
       "step (round) per request, ordered by earliest timing_events timestamp "
       "-> cc-weka requests[].t")


def parse_ts(s):
    s = s.strip()
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        dt = datetime.strptime(s, "%Y-%m-%dT%H:%M:%S%z")
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.timestamp()


def _open(path):
    if path.endswith(".gz"):
        return io.TextIOWrapper(gzip.open(path, "rb"), encoding="utf-8")
    return open(path, "r", encoding="utf-8")


def load(path):
    sessions = collections.defaultdict(list)  # session_id -> [start_ts, ...]
    users = set()
    with _open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            rec = json.loads(line)
            sid = rec.get("session_id")
            if rec.get("user") is not None:
                users.add(rec["user"])
            ts = None
            for ev in rec.get("timing_events") or []:
                t = ev.get("timestamp")
                if t:
                    tt = parse_ts(t)
                    if ts is None or tt < ts:
                        ts = tt
            sessions[sid].append(ts if ts is not None else float("nan"))
    return sessions, users


def r6(x):
    return round(float(x), 6)


def make_buckets(values, start, stop):
    edges = np.geomspace(start, stop, NUM_BUCKETS + 1)
    counts, _ = np.histogram(np.clip(values, start, stop), bins=edges)
    total = len(values)
    return edges, counts, total


class _ODumper(yaml.SafeDumper):
    pass


_ODumper.add_representer(
    dict, lambda d, data: d.represent_mapping("tag:yaml.org,2002:map", data.items()))


def dump(doc, path):
    with open(path, "w") as fh:
        yaml.dump(doc, fh, Dumper=_ODumper, default_flow_style=False,
                  sort_keys=False, width=4096)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("trace", help="syfi_coding_trace.jsonl or .jsonl.gz")
    ap.add_argument("--turns-out", default="syfi-coding-turns.yaml")
    ap.add_argument("--intervals-out", default="syfi-coding-intervals.yaml")
    args = ap.parse_args()

    sessions, users = load(args.trace)
    num_sessions = len(sessions)
    total_steps = sum(len(v) for v in sessions.values())
    src = SRC.format(sessions=num_sessions, steps=total_steps, users=len(users))

    turns = np.array([len(v) for v in sessions.values()], dtype=float)
    intervals = []
    for v in sessions.values():
        good = sorted(t for t in v if t == t)  # chronological, drop NaN
        intervals.extend(good[i] - good[i - 1]
                         for i in range(1, len(good)) if good[i] >= good[i - 1])
    intervals = np.array(intervals, dtype=float)

    # turns
    edges, counts, total = make_buckets(turns, float(turns.min()), float(turns.max()))
    dump({
        "source": src,
        "units": "turns",
        "metric": "requests (turns) per conversation",
        "scale": "log",
        "num_conversations": num_sessions,
        "total_turns": total_steps,
        "summary": {
            "min": int(turns.min()), "max": int(turns.max()),
            "mean": r6(turns.mean()), "median": r6(np.median(turns)),
            "p90": r6(np.percentile(turns, 90)), "p99": r6(np.percentile(turns, 99)),
        },
        "buckets": [{"index": i, "lower": r6(edges[i]), "upper": r6(edges[i + 1]),
                     "count": int(counts[i]), "fraction": r6(counts[i] / total)}
                    for i in range(NUM_BUCKETS)],
    }, args.turns_out)

    # intervals
    edges, counts, total = make_buckets(intervals, 1e-6, float(intervals.max()))
    dump({
        "source": src,
        "units": "seconds",
        "format": "cc-weka",
        "metric": "intra-conversation request inter-arrival interval",
        "scale": "log",
        "num_intervals": int(len(intervals)),
        "summary": {
            "min_s": r6(intervals.min()), "max_s": r6(intervals.max()),
            "mean_s": r6(intervals.mean()), "median_s": r6(np.median(intervals)),
            "p90_s": r6(np.percentile(intervals, 90)),
            "p99_s": r6(np.percentile(intervals, 99)),
        },
        "buckets": [{"index": i, "lower_s": r6(edges[i]), "upper_s": r6(edges[i + 1]),
                     "count": int(counts[i]), "fraction": r6(counts[i] / total)}
                    for i in range(NUM_BUCKETS)],
    }, args.intervals_out)

    print(f"{num_sessions} sessions, {total_steps} steps, {len(users)} users -> "
          f"{args.turns_out}, {args.intervals_out} ({len(intervals)} intervals)")


if __name__ == "__main__":
    main()
