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

Multi-GPU hosts are aggregated by averaging util across the GPUs present at each
tick. Usage: gpu_report.py <timeline.csv> <markers.csv>
"""
import csv
import sys
from collections import defaultdict

BLOCKS = "▁▂▃▄▅▆▇█"


def read_timeline(path):
    """Return {epoch: {"util": mean_util, "clock": mean_clock, "mem": max_mem,
    "power": sum_power}} aggregated across GPUs sharing that tick."""
    per_tick = defaultdict(lambda: {"util": [], "clock": [], "mem": [], "power": []})
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

            d = per_tick[t]
            for key, col in (("util", "util_gpu_pct"), ("clock", "sm_clock_mhz"),
                             ("mem", "mem_used_mib"), ("power", "power_w")):
                v = num(col)
                if v is not None:
                    d[key].append(v)
    ticks = {}
    for t, d in per_tick.items():
        ticks[t] = {
            "util": sum(d["util"]) / len(d["util"]) if d["util"] else 0.0,
            "clock": sum(d["clock"]) / len(d["clock"]) if d["clock"] else 0.0,
            "mem": max(d["mem"]) if d["mem"] else 0.0,
            "power": sum(d["power"]) if d["power"] else 0.0,
        }
    return ticks


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
    ticks = read_timeline(argv[1])
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
    print(f"samples={len(ticks)}  (aggregated across GPUs per tick)")
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
