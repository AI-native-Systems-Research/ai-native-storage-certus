#!/usr/bin/env python3
"""Summarize a profile_all.sh GPU telemetry run.

Reads the two CSVs the sampler wrote:
  gpu-timeline.csv : epoch_s,gpu_idx,util_gpu_pct,util_mem_pct,mem_used_mib,
                     sm_clock_mhz,temp_c,power_w   (one row per GPU per tick)
  gpu-markers.csv  : epoch_s,phase,variant         (phase = start|end)

and prints, per variant window (and one "whole run" row):
  - avg / max / p95 GPU utilization
  - avg SM clock (confirms the clock lock held)
  - max memory used, avg power, sample count
  - an over-time utilization sparkline across the window

The sampler queries EVERY GPU on the host, but a run typically drives only the
one(s) passed via --gpu. Averaging util across all GPUs present at each tick
would divide the busy GPU's number by the host's GPU count (e.g. a GPU running
at 68% shows as 8.5% on an 8-GPU box). So we first detect which GPUs were
actually used — a GPU counts as ACTIVE if its peak util over the run clears
ACTIVE_UTIL_PCT (default 5%) — and aggregate only those, reporting the active
count/indices. Override the threshold with GPU_REPORT_ACTIVE_PCT.

Usage: gpu_report.py <timeline.csv> <markers.csv>
"""
import csv
import os
import sys
from collections import defaultdict

BLOCKS = "▁▂▃▄▅▆▇█"

# A GPU is "active" (part of the run) if its peak util over the whole timeline
# clears this percent; idle cards sit at ~0 and are excluded from aggregation.
ACTIVE_UTIL_PCT = float(os.environ.get("GPU_REPORT_ACTIVE_PCT", "5"))


def _active_gpus(rows_by_gpu):
    """GPUs whose peak util over the run clears ACTIVE_UTIL_PCT. Falls back to
    all GPUs present if none clear it (so a genuinely idle run still reports)."""
    active = sorted(g for g, utils in rows_by_gpu.items()
                    if utils and max(utils) >= ACTIVE_UTIL_PCT)
    return active or sorted(rows_by_gpu)


def read_timeline(path):
    """Return (ticks, active_gpus) where ticks is {epoch: {"util","clock","mem",
    "power"}} aggregated across ONLY the GPUs that actually ran the workload, and
    active_gpus is the sorted list of those GPU indices."""
    # First pass: parse every (tick, gpu) row eagerly (values captured now, not
    # via a closure over the loop variable), and track per-GPU util peaks so we
    # can tell which GPUs were actually driven vs. sitting idle on the host.
    cols = (("util", "util_gpu_pct"), ("clock", "sm_clock_mhz"),
            ("mem", "mem_used_mib"), ("power", "power_w"))
    rows = []
    util_by_gpu = defaultdict(list)
    with open(path, newline="") as fh:
        for row in csv.DictReader(fh):
            try:
                t = int(float(row["epoch_s"]))
            except (KeyError, ValueError):
                continue

            def num(k):
                try:
                    return float(row[k])
                except (KeyError, ValueError):
                    return None

            gpu = row.get("gpu_idx", "0")
            vals = {key: num(col) for key, col in cols}
            if vals["util"] is not None:
                util_by_gpu[gpu].append(vals["util"])
            rows.append((t, gpu, vals))

    active = set(_active_gpus(util_by_gpu))

    per_tick = defaultdict(lambda: {"util": [], "clock": [], "mem": [], "power": []})
    for t, gpu, vals in rows:
        if gpu not in active:
            continue
        d = per_tick[t]
        for key, _col in cols:
            if vals[key] is not None:
                d[key].append(vals[key])
    ticks = {}
    for t, d in per_tick.items():
        ticks[t] = {
            "util": sum(d["util"]) / len(d["util"]) if d["util"] else 0.0,
            "clock": sum(d["clock"]) / len(d["clock"]) if d["clock"] else 0.0,
            "mem": max(d["mem"]) if d["mem"] else 0.0,
            "power": sum(d["power"]) if d["power"] else 0.0,
        }
    return ticks, sorted(active, key=lambda g: (len(g), g))


def read_windows(path):
    """Return [(variant, start_epoch, end_epoch)] from start/end marker pairs."""
    starts = {}
    windows = []
    try:
        with open(path, newline="") as fh:
            for row in csv.DictReader(fh):
                try:
                    t = int(float(row["epoch_s"]))
                except (KeyError, ValueError):
                    continue
                phase, variant = row.get("phase", ""), row.get("variant", "")
                if phase == "start":
                    starts[variant] = t
                elif phase == "end" and variant in starts:
                    windows.append((variant, starts.pop(variant), t))
    except FileNotFoundError:
        pass
    return windows


def pct(vals, p):
    if not vals:
        return 0.0
    s = sorted(vals)
    i = min(len(s) - 1, int(round((p / 100.0) * (len(s) - 1))))
    return s[i]


def sparkline(series, buckets=48):
    """Bucket a list of utilization values into a fixed-width block sparkline."""
    if not series:
        return ""
    n = len(series)
    if n <= buckets:
        vals = series
    else:
        vals = []
        for b in range(buckets):
            lo = b * n // buckets
            hi = max(lo + 1, (b + 1) * n // buckets)
            chunk = series[lo:hi]
            vals.append(sum(chunk) / len(chunk))
    out = []
    for v in vals:
        idx = int(round((max(0.0, min(100.0, v)) / 100.0) * (len(BLOCKS) - 1)))
        out.append(BLOCKS[idx])
    return "".join(out)


def summarize(name, ticks_in_window):
    epochs = sorted(ticks_in_window)
    util = [ticks_in_window[t]["util"] for t in epochs]
    clock = [ticks_in_window[t]["clock"] for t in epochs]
    mem = [ticks_in_window[t]["mem"] for t in epochs]
    power = [ticks_in_window[t]["power"] for t in epochs]
    dur = (epochs[-1] - epochs[0]) if len(epochs) > 1 else 0
    return {
        "name": name,
        "n": len(epochs),
        "dur_s": dur,
        "util_avg": sum(util) / len(util) if util else 0.0,
        "util_max": max(util) if util else 0.0,
        "util_p95": pct(util, 95),
        "clock_avg": sum(clock) / len(clock) if clock else 0.0,
        "mem_max_gib": (max(mem) if mem else 0.0) / 1024.0,
        "power_avg": sum(power) / len(power) if power else 0.0,
        "spark": sparkline(util),
    }


def main(argv):
    if len(argv) < 2:
        print("usage: gpu_report.py <timeline.csv> [markers.csv]", file=sys.stderr)
        return 2
    ticks, active_gpus = read_timeline(argv[1])
    if not ticks:
        print("[gpu-report] no samples in timeline", file=sys.stderr)
        return 0
    windows = read_windows(argv[2]) if len(argv) > 2 else []

    rows = []
    for variant, start, end in windows:
        win = {t: v for t, v in ticks.items() if start <= t <= end}
        if win:
            rows.append(summarize(variant, win))
    rows.append(summarize("── whole run", ticks))

    print("")
    print("================================ GPU Utilization ================================")
    gpus_str = ",".join(active_gpus)
    print(f"samples={len(ticks)}  active GPU(s)={gpus_str} (n={len(active_gpus)}, "
          f"idle cards excluded; util aggregated over active only)")
    print("")
    hdr = f"{'Window':<16} {'dur(s)':>6} {'util avg':>8} {'max':>4} {'p95':>4} " \
          f"{'clk MHz':>7} {'mem GiB':>7} {'pwr W':>6} {'n':>4}"
    print(hdr)
    print("-" * len(hdr))
    for r in rows:
        print(f"{r['name']:<16} {r['dur_s']:>6d} {r['util_avg']:>7.1f}% "
              f"{r['util_max']:>3.0f}% {r['util_p95']:>3.0f}% "
              f"{r['clock_avg']:>7.0f} {r['mem_max_gib']:>7.2f} "
              f"{r['power_avg']:>6.1f} {r['n']:>4d}")
    print("")
    print("GPU utilization over time (each row is one window, left→right = start→end):")
    for r in rows:
        if r["spark"]:
            print(f"  {r['name']:<16} {r['spark']}")
    print("=================================================================================")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
