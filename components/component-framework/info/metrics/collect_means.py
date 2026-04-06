#!/usr/bin/env python3
"""Collect mean values for all numeric metrics from rust-code-analysis-cli output.

Usage: run from the `component-framework` directory (script handles paths):
    ./scripts/collect_means.py

Outputs:
 - metrics_means.json  (in component-framework root)
 - prints a compact summary to stdout
"""

import json
import os
import subprocess
import sys
import shutil


def find_cli():
    cli = shutil.which("rust-code-analysis-cli")
    if cli:
        return cli
    # fallback to cargo bin path
    home = os.path.expanduser("~")
    fallback = os.path.join(home, ".cargo", "bin", "rust-code-analysis-cli")
    if os.path.exists(fallback):
        return fallback
    return None


def parse_many_json(s):
    decoder = json.JSONDecoder()
    idx = 0
    n = len(s)
    while True:
        # skip whitespace
        while idx < n and s[idx].isspace():
            idx += 1
        if idx >= n:
            break
        obj, j = decoder.raw_decode(s, idx)
        yield obj
        idx = j


def flatten_metrics(prefix, obj):
    if isinstance(obj, dict):
        for k, v in obj.items():
            yield from flatten_metrics(prefix + [k], v)
    elif isinstance(obj, (int, float)):
        yield (".".join(prefix), float(obj))


def flatten_keys(prefix, obj, keys):
    """Collect metric key paths (as dotted strings) from a nested dict."""
    if isinstance(obj, dict):
        for k, v in obj.items():
            flatten_keys(prefix + [k], v, keys)
    else:
        keys.add(".".join(prefix))


def main():
    cli = find_cli()
    if not cli:
        print(
            "rust-code-analysis-cli not found in PATH or ~/.cargo/bin", file=sys.stderr
        )
        sys.exit(2)

    # script is in component-framework/scripts; base_dir is parent
    script_dir = os.path.dirname(os.path.abspath(__file__))
    base_dir = os.path.dirname(script_dir)

    # collect all .rs files under base_dir
    rs_files = []
    for root, dirs, files in os.walk(base_dir):
        # skip target and hidden dirs
        dirs[:] = [d for d in dirs if not d.startswith(".") and d != "target"]
        for f in files:
            if f.endswith(".rs"):
                rs_files.append(os.path.join(root, f))

    if not rs_files:
        print("no Rust files found", file=sys.stderr)
        sys.exit(1)

    args = [cli]
    for p in rs_files:
        args += ["--paths", p]
    args += ["--metrics", "--output-format", "json"]

    try:
        out = subprocess.check_output(args, cwd=base_dir, stderr=subprocess.PIPE)
    except subprocess.CalledProcessError as e:
        print("cli failed:", e.stderr.decode(), file=sys.stderr)
        sys.exit(3)

    text = out.decode("utf-8", errors="replace")

    sums = {}
    counts = {}
    keys = set()
    per_file = []

    # parse each JSON object (each corresponds to a file/unit)
    for obj in parse_many_json(text):
        name = obj.get("name") or obj.get("file") or "<unknown>"
        metrics = obj.get("metrics", {})
        per_file.append({"name": name, "metrics": metrics})

        # collect numeric metrics for means
        for key, value in flatten_metrics([], metrics):
            sums[key] = sums.get(key, 0.0) + value
            counts[key] = counts.get(key, 0) + 1

        # collect all keys (including non-numeric) for discovery
        flatten_keys([], metrics, keys)

    means = {k: (sums[k] / counts[k]) for k in sums}

    # write outputs
    out_means = os.path.join(base_dir, "metrics_means.json")
    with open(out_means, "w") as f:
        json.dump({"means": means, "files_analyzed": len(per_file)}, f, indent=2)

    out_all = os.path.join(base_dir, "metrics_all.json")
    with open(out_all, "w") as f:
        json.dump(per_file, f, indent=2)

    out_keys = os.path.join(base_dir, "metrics_keys.json")
    with open(out_keys, "w") as f:
        json.dump({"keys": sorted(keys)}, f, indent=2)

    # print a neat summary (sorted by metric name)
    print(f"analyzed {len(per_file)} files; means written to {out_means}")
    print(f"all per-file metrics written to {out_all}")
    print(f"discovered {len(keys)} metric keys; saved to {out_keys}")
    for k in sorted(means.keys()):
        print(f"{k}: {means[k]:.6g}")


if __name__ == "__main__":
    main()
