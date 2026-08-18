#!/usr/bin/env python3
"""Fit every trace in a corpus and tabulate the outcome — the anti-overfitting check.

Run this before and after any change to the fit or the generator, and judge the change
on the whole table rather than on whichever traces you were debugging.

# Why this exists

Every model change during this work was measured on `tau2_airline`, `tau2_retail` and
`tau2_telecom`, because those were the traces that fitted end to end. They are three task
domains of **one** benchmark, produced by the same harness with the same agent scaffold —
near-siblings, not three independent workloads. Reporting "three traces, three seeds, nine
cells" made that sound like nine observations; the seeds only quantify generator noise, so
the effective sample was one workload family.

Two things went wrong because of it, both recorded in the spec:

- **FR-054g was written wider than its evidence.** "Reuse distance is minimised at about
  1.18x the reference count" held on `tau2` and was stated as a property of the model. Two
  agentic traces from other families sit at 4.5x the reuse tolerance with their reference
  counts already at parity, so the claim does not transfer, and nothing in a `tau2`-only
  workflow would have revealed that.
- **Seven of the corpus's traces were misattributed** and it went unnoticed. Every
  `metadata_only` source reported `CALLER INPUT: this trace carries several blockings — []
  tokens`, a sentence that contradicts itself, and following its advice produced a bare
  `No such file or directory`. They carry no block data at all, which FR-054b classifies as
  a limit of the model. A wrong label there is worse than a wrong number, because the label
  is what a reader uses to decide what to build next.

The deeper exposure is not in any parameter — every parameter is fitted — but in
**prioritisation**. Which residual gets chased comes from whichever traces are in front of
you, and on a differently-shaped workload the dominant term may be a different one
entirely.

# Usage

    python3 research/corpus_matrix.py --corpus traces/ --bin target/release/certus-trace

`--json` emits the same table as machine-readable rows, so two runs can be diffed. Traces
that refuse are reported with their FR-054b classification rather than dropped: a change
that turns a MODEL LIMITATION into a fit is progress, and a change that turns a fit into a
refusal is a regression, and neither is visible in a table of only the traces that fitted.

Build the binary with `--features parquet`; most real traces are parquet directories.
"""

import argparse
import json
import pathlib
import re
import subprocess
import sys

# The gated statistics, with the FR-056 default tolerances at a stated 50k requests.
# Reported beside each divergence so the table says pass or fail without a second lookup.
TOLERANCES = {
    "request_length": 0.02,
    "sharing_depth": 0.05,
    "reuse_distance_objects": 0.02,
    "unique_keys": 0.15,
}


def classify(out: str, rc: int) -> str:
    """The FR-054b outcome, from the report rather than from the exit code.

    Order matters: a trace can carry a MODEL LIMITATION caveat *and* still fit and be
    compared, which is the normal case for a real trace, so the compared-ness is reported
    separately rather than overwriting the classification.
    """
    if rc not in (0, 1):
        return f"CRASH/TIMEOUT rc={rc}"
    for marker in ("CORRUPT TRACE", "CALLER INPUT", "MODEL LIMITATION"):
        if marker in out:
            base = marker
            break
    else:
        base = "OK"
    fitted = bool(re.search(r"^  statistic\s", out, re.M))
    if fitted:
        return f"{base}+FIT"
    # A trace that neither fitted nor named an FR-054b outcome must NOT read as `OK`.
    # That combination is not a success, it is a refusal whose classification is missing,
    # and reporting it as OK is the same class of error this whole check exists to catch —
    # the first run of this script did exactly that for four traces, whose refusal messages
    # predated the taxonomy. Kept as a distinct label rather than folded into one of the
    # three, because inventing a classification here would hide the gap in the tool.
    return "UNCLASSIFIED — refused, named no FR-054b outcome" if base == "OK" else base


def measure(binary: str, trace: pathlib.Path, block_bytes: int, window: int, seed: int,
            timeout: int, fit_args: list = ()) -> dict:
    cmd = [binary, "fit", "-t", str(trace), "--block-bytes", str(block_bytes),
           "--wss-window", str(window), "--seed", str(seed), *fit_args]
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        out, rc = p.stdout + p.stderr, p.returncode
    except subprocess.TimeoutExpired:
        out, rc = "", 124
    row = {"trace": trace.name, "verdict": classify(out, rc)}
    m = re.search(r"^  measured\s+(\d+) requests, (\d+) references, (\d+) sessions", out, re.M)
    if m:
        row["requests"], row["references"], row["sessions"] = (int(g) for g in m.groups())
    syn = re.search(r"^  synthetic (\d+) requests, (\d+)", out, re.M)
    if syn and m:
        # The reference ratio is not gated, and it is the first thing to look at anyway:
        # it is a mean, so it moves for reasons every distributional check is blind to.
        row["refs_ratio"] = int(syn.group(2)) / int(m.group(2))
    for stat in TOLERANCES:
        s = re.search(rf"^  {stat}\s+([\d.]+)\s+([\d.]+)\s+\d+\s+(\w+)", out, re.M)
        if s:
            row[stat] = float(s.group(1))
            row[stat + "_verdict"] = s.group(3)
    # One line of why, for a trace that did not fit. The classification says which kind of
    # problem it is; this says which instance.
    why = re.search(r"(MODEL LIMITATION[^\n]{0,150}|CALLER INPUT: [^\n]{0,150}|CORRUPT TRACE[^\n]{0,150})", out)
    if "+FIT" not in row["verdict"]:
        if why:
            row["why"] = why.group(1)
        else:
            # Falling back to the tool's own error line matters most in exactly the case the
            # marker search missed: without it an UNCLASSIFIED row would report no reason
            # either, and the one row needing a human to look would be the least legible.
            err = re.search(r"^certus-trace: (.{0,150})", out, re.M)
            if err:
                row["why"] = err.group(1)
    return row


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--corpus", default="traces", type=pathlib.Path)
    ap.add_argument("--bin", default="target/release/certus-trace")
    ap.add_argument("--block-bytes", type=int, default=131072)
    ap.add_argument("--wss-window", type=int, default=5000)
    ap.add_argument("--seed", type=int, default=4242)
    ap.add_argument("--timeout", type=int, default=900, help="per trace, seconds")
    ap.add_argument("--json", action="store_true")
    # Pass-through so an arm of an experiment can be measured across the whole corpus without
    # editing this script. FR-055f asks for the whole table per change, and a change reachable
    # only behind a flag (`--branching-segments`) was otherwise unmeasurable here.
    ap.add_argument("--fit-arg", action="append", default=[], metavar="ARG",
                    help="extra argument passed to `certus-trace fit`; repeatable")
    a = ap.parse_args()

    traces = sorted(p for p in a.corpus.iterdir() if p.is_dir() or p.suffix == ".jsonl")
    if not traces:
        print(f"no traces under {a.corpus}", file=sys.stderr)
        return 2
    rows = []
    for t in traces:
        rows.append(measure(a.bin, t, a.block_bytes, a.wss_window, a.seed, a.timeout,
                            a.fit_arg))
        if not a.json:
            print(".", end="", flush=True, file=sys.stderr)
    if not a.json:
        print(file=sys.stderr)

    if a.json:
        print(json.dumps(rows, indent=2, sort_keys=True))
        return 0

    fitted = [r for r in rows if "+FIT" in r["verdict"]]
    cols = ["request_length", "sharing_depth", "reuse_distance_objects", "unique_keys"]
    short = {"request_length": "req_len", "sharing_depth": "share",
             "reuse_distance_objects": "reuse", "unique_keys": "uniq"}
    print(f"\n{len(fitted)} of {len(rows)} traces fit and compare\n")
    head = f"{'trace':<26}{'sessions':>9}{'refs':>8}" + "".join(f"{short[c]:>9}" for c in cols)
    print(head)
    print("-" * len(head))
    for r in sorted(fitted, key=lambda r: r["trace"]):
        line = f"{r['trace']:<26}{r.get('sessions', 0):>9}{r.get('refs_ratio', float('nan')):>8.3f}"
        for c in cols:
            v = r.get(c)
            # A trailing marker rather than colour: this output lands in commit messages
            # and terminals that strip escapes.
            line += f"{v:>8.3f}{'!' if v is not None and v > TOLERANCES[c] else ' '}" if v is not None else f"{'—':>9}"
        print(line)
    print("\n  ! = outside the FR-056 tolerance " +
          ", ".join(f"{short[c]} {TOLERANCES[c]}" for c in cols))
    n = len(fitted)
    if n:
        within = lambda c: sum(1 for r in fitted if r.get(c) is not None and r[c] <= TOLERANCES[c])
        print("  within tolerance: " +
              ", ".join(f"{short[c]} {within(c)}/{n}" for c in cols))
        near = sum(1 for r in fitted if abs(r.get("refs_ratio", 9) - 1) <= 0.035)
        print(f"  reference count within +/-3.5%: {near}/{n}")

    refused = [r for r in rows if "+FIT" not in r["verdict"]]
    if refused:
        print(f"\n{len(refused)} refused — by FR-054b classification, not dropped:\n")
        for r in sorted(refused, key=lambda r: (r["verdict"], r["trace"])):
            print(f"  {r['trace']:<26} {r['verdict']}")
            if r.get("why"):
                print(f"  {'':<26}   {r['why'][:110]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
