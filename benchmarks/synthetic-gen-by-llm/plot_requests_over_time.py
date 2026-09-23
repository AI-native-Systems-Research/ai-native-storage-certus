#!/usr/bin/env python3
"""Plot vLLM in-flight/queued request counts over wall-clock time.

Parses the periodic `INFO HH:MM:SS [loggers.py] Engine 000: ... Running: N reqs,
Waiting: M reqs[, Deferred: K reqs]` snapshots out of one or more run logs and
draws requests-vs-seconds, one colour per variant (Running solid, Waiting
dashed). Time is normalised so each variant starts at t=0 (its first snapshot),
so the curves overlay for shape comparison regardless of wall-clock offset.

Usage:
  plot_requests_over_time.py TAG=path/to/variant.log [TAG=... ...] -o out.png
"""
import re
import sys
import argparse
from datetime import datetime, timedelta

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

# Fixed variant colours, matching render_kvprofile.py.
COLORS = {
    "certus": "#D4A017", "certus-spdk": "#D4A017",
    "tiered": "#2E8B57", "tiered-cpu-fs": "#2E8B57",
    "nooffload": "#3A6EA5", "cpuoffload": "#E07B39", "sharedstorage": "#3AAFA9",
}

LINE = re.compile(
    r"(\d{2}):(\d{2}):(\d{2}) \[loggers\.py.*?"
    r"Running:\s*(\d+)\s*reqs,\s*Waiting:\s*(\d+)\s*reqs"
    r"(?:,\s*Deferred:\s*(\d+)\s*reqs)?"
)


def parse(path):
    """Return (t_seconds[], running[], waiting[], deferred[]) from a run log."""
    ts, run, wait, defer = [], [], [], []
    t0 = None
    with open(path, errors="replace") as f:
        for raw in f:
            for m in LINE.finditer(raw):
                h, mi, s = int(m.group(1)), int(m.group(2)), int(m.group(3))
                t = datetime(2000, 1, 1, h, mi, s)
                if t0 is None:
                    t0 = t
                # Handle a midnight wrap (rare) by adding a day if we went backwards.
                if t < t0:
                    t += timedelta(days=1)
                ts.append((t - t0).total_seconds())
                run.append(int(m.group(4)))
                wait.append(int(m.group(5)))
                defer.append(int(m.group(6)) if m.group(6) else 0)
    return ts, run, wait, defer


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+", help="TAG=log.path entries")
    ap.add_argument("-o", "--out", default="requests-over-time.png")
    ap.add_argument("--title", default="Requests over time")
    ap.add_argument("--subtitle", default="")
    ap.add_argument("--dpi", type=int, default=200)
    args = ap.parse_args()

    fig, ax = plt.subplots(figsize=(11, 6))
    for entry in args.runs:
        tag, _, path = entry.partition("=")
        if not path:
            path, tag = tag, tag
        color = COLORS.get(tag.lower(), "#666666")
        ts, run, wait, defer = parse(path)
        if not ts:
            print(f"warning: no engine snapshots in {path}", file=sys.stderr)
            continue
        ax.plot(ts, run, color=color, lw=2.0, label=f"{tag} — running")
        ax.plot(ts, wait, color=color, lw=1.5, ls="--", alpha=0.8,
                label=f"{tag} — waiting")
        if any(defer):
            ax.plot(ts, defer, color=color, lw=1.2, ls=":", alpha=0.7,
                    label=f"{tag} — deferred")
        print(f"{tag}: {len(ts)} snapshots, span {ts[-1]:.0f}s, "
              f"peak running {max(run)}, peak waiting {max(wait)}")

    ax.set_xlabel("wall-clock time since first engine step (s)")
    ax.set_ylabel("requests")
    ax.grid(True, alpha=0.25, lw=0.6)
    ax.spines[["top", "right"]].set_visible(False)
    ax.set_ylim(bottom=0)
    ax.set_xlim(left=0)
    ax.legend(frameon=False, ncol=2, fontsize=9, loc="upper right")

    title = args.title
    fig.suptitle(title, fontsize=15, fontweight="bold", x=0.02, ha="left")
    if args.subtitle:
        ax.set_title(args.subtitle, fontsize=10, color="#555", loc="left", pad=8)
    fig.tight_layout(rect=(0, 0, 1, 0.97))
    fig.savefig(args.out, dpi=args.dpi)
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
