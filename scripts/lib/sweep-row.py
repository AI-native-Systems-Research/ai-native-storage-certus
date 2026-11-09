#!/usr/bin/env python3
"""Flatten one `remote-lookup-bench lookup` JSON result into a TSV sweep row.

Shared by the local and multi-node sweep drivers so both legs produce byte-identical
column layouts — the whole point of the sweep is comparing them, and two
independently-written formatters would eventually disagree about a unit.

Usage: sweep-row.py <json-path> <batch> <workers> <inflight> <replicate> <label>
                    <object-size>

Writes one tab-separated line to stdout. An unparsable input is reported on stderr
and produces no row, so one bad round cannot abort a long sweep.
"""

import json
import sys

FIELDS = (
    "batch", "workers", "inflight", "replicate", "label", "object_size",
    "gbps", "p50_us", "p99_us", "elapsed_s", "keys_ok", "keys_failed",
    "local_read_ops", "verify_failures",
)


def main() -> int:
    if len(sys.argv) != 8:
        print(__doc__, file=sys.stderr)
        return 2
    path, batch, workers, inflight, replicate, label, objsize = sys.argv[1:8]
    try:
        with open(path) as fh:
            d = json.load(fh)
    except (OSError, ValueError) as e:
        print(f"# unparsed {path}: {e}", file=sys.stderr)
        return 0

    row = (
        batch, workers, inflight, replicate, label, objsize,
        f"{d['gbps']:.4f}",
        f"{d['rpc_latency_us']['p50']:.1f}",
        f"{d['rpc_latency_us']['p99']:.1f}",
        f"{d['elapsed_s']:.4f}",
        d["keys_ok"], d["keys_failed"],
        d.get("local_read_ops_delta", 0),
        d.get("verify_failures", 0),
    )
    print("\t".join(str(x) for x in row))
    return 0


if __name__ == "__main__":
    sys.exit(main())
