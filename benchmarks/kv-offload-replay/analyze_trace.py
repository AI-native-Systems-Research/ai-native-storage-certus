#!/usr/bin/env python3
"""
analyze_trace.py — summarize kvconn_trace_*.jsonl after a benchmark run.

Merges all per-pid trace files and prints:
  - call counts per method (grouped by role)
  - total and mean elapsed time
  - wall-time span
  - any errors
"""

import json
import sys
from collections import defaultdict
from pathlib import Path

TRACE_DIR = Path(__file__).parent


def load_records(trace_dir: Path) -> list[dict]:
    files = sorted(trace_dir.glob("kvconn_trace_*.jsonl"))
    if not files:
        print(f"No trace files found in {trace_dir}")
        sys.exit(1)
    print(f"Trace files: {[f.name for f in files]}")
    records = []
    for f in files:
        with open(f) as fh:
            for line in fh:
                line = line.strip()
                if line:
                    records.append(json.loads(line))
    return records


def main():
    trace_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else TRACE_DIR
    records = load_records(trace_dir)

    if not records:
        print("No records found.")
        return

    counts: dict[str, int] = defaultdict(int)
    total_elapsed: dict[str, float] = defaultdict(float)
    errors = []
    ts_min = float("inf")
    ts_max = float("-inf")
    pids: set[int] = set()

    for r in records:
        key = f"{r['role']}.{r['method']}"
        counts[key] += 1
        total_elapsed[key] += r["elapsed"]
        ts_min = min(ts_min, r["ts"])
        ts_max = max(ts_max, r["ts"] + r["elapsed"])
        pids.add(r.get("pid", 0))
        if r.get("error"):
            errors.append(r)

    print(f"\nTotal records : {len(records)}")
    print(f"Processes     : {sorted(pids)}")
    print(f"Wall span     : {ts_max - ts_min:.3f}s")
    print(f"Errors        : {len(errors)}")
    print()

    col = 50
    header = f"{'Role.Method':<{col}} {'Calls':>6} {'Total(s)':>10} {'Mean(ms)':>10}"
    print(header)
    print("-" * len(header))
    for key in sorted(total_elapsed, key=lambda k: total_elapsed[k], reverse=True):
        n = counts[key]
        tot = total_elapsed[key]
        mean_ms = (tot / n) * 1000
        print(f"{key:<{col}} {n:>6} {tot:>10.4f} {mean_ms:>10.3f}")

    if errors:
        print(f"\n--- Errors ({len(errors)}) ---")
        for e in errors[:20]:
            print(f"  pid={e.get('pid')} {e['role']}.{e['method']}: {e['error']}")


if __name__ == "__main__":
    main()
