#!/usr/bin/env python3
"""plot-gpu-vs-io.py — plot GPU compute time vs KV-IO time over a run, with wall
time on the X-axis, from a CSV written by poll-kv-metrics.sh.

The poller records CUMULATIVE counters; this diffs consecutive samples to get, per
wall interval:
  * KV load  IO  = d(kv_offload_time CPU_to_GPU)   — reloading KV onto the GPU
  * KV store IO  = d(kv_offload_time GPU_to_CPU)    — evicting KV off the GPU
  * GPU prefill-compute ~= d(request_prefill_time_sum) - d(load_time)
        (the reload happens inside the prefill window, so subtract it out)

All three are summed over concurrently-running requests, so each is plotted as
"device-seconds per wall-second" = d(metric)/d(wall): a value of 2.0 means that, on
average over the interval, ~2 requests' worth of that activity was in flight. Each
series gets its own stacked subplot (shared wall-time x-axis, independent y-scale)
so a small band stays visible next to a large one; the relative heights tell you
whether the run is compute- or IO-bound. A fourth subplot shows the measured KV
transfer bandwidth (GiB/s).

NOTE (estimate, not exact): with continuous batching one request's reload overlaps
another's compute, so the compute = prefill - load split is approximate; the KV-IO
bands and the bandwidth panel are measured directly.

  ./plot-gpu-vs-io.py /mnt/certus1/vllm-serve-matrix/kvio/kvio_YYYYMMDD_HHMMSS.csv
  ./plot-gpu-vs-io.py run.csv -o out.png --title "MHA iobound, 8 streams"
"""
import argparse
import csv
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

GIB = 1 << 30


def load(path):
    with open(path) as fh:
        rows = list(csv.DictReader(fh))
    if len(rows) < 2:
        sys.exit(f"{path}: need at least 2 samples to diff, got {len(rows)}")
    return rows


def diffs(rows):
    """Yield per-interval derived series from cumulative rows."""
    out = {k: [] for k in ("t", "load_io", "store_io", "compute", "load_bw", "store_bw")}
    for a, b in zip(rows, rows[1:]):
        dt = float(b["wall_s"]) - float(a["wall_s"])
        if dt <= 0:
            continue
        dload = float(b["load_time_s"]) - float(a["load_time_s"])
        dstore = float(b["store_time_s"]) - float(a["store_time_s"])
        dpref = float(b["prefill_sum_s"]) - float(a["prefill_sum_s"])
        dlb = float(b["load_bytes"]) - float(a["load_bytes"])
        dsb = float(b["store_bytes"]) - float(a["store_bytes"])
        # Prefill compute = prefill window minus the reload that happens inside it.
        compute = max(dpref - dload, 0.0)
        out["t"].append(float(b["wall_s"]))
        out["load_io"].append(dload / dt)
        out["store_io"].append(dstore / dt)
        out["compute"].append(compute / dt)
        out["load_bw"].append(dlb / dt / GIB)
        out["store_bw"].append(dsb / dt / GIB)
    if not out["t"]:
        sys.exit("no positive-dt intervals; check the CSV")
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("csv")
    ap.add_argument("-o", "--out")
    ap.add_argument("--title", default="")
    args = ap.parse_args()

    d = diffs(load(args.csv))
    out = args.out or (args.csv.rsplit(".", 1)[0] + ".png")

    # Three stacked subplots, one series each (GPU compute / KV load / KV store),
    # sharing the wall-time x-axis, each with its own y-scale so a small band stays
    # visible next to a large one. Values are device-seconds per wall-second (≈ the
    # number of concurrent streams busy in that activity). A fourth panel keeps the
    # measured KV transfer bandwidth (GiB/s).
    panels = [
        ("compute", "GPU prefill-compute", "#4C78A8"),
        ("load_io", "KV load IO (CPU→GPU)", "#F58518"),
        ("store_io", "KV store IO (GPU→CPU)", "#E45756"),
    ]
    fig, axes = plt.subplots(4, 1, figsize=(11, 10.5), sharex=True)
    for ax, (key, label, color) in zip(axes, panels):
        ax.fill_between(d["t"], d[key], color=color, alpha=0.85)
        ax.plot(d["t"], d[key], color=color, linewidth=1)
        ax.set_ylabel("device-s / wall-s")
        ax.set_title(label, fontsize=11, loc="left")
        ax.grid(True, alpha=0.3)
        ax.set_ylim(bottom=0)
    axes[0].set_title(f"{args.title or 'GPU compute vs KV-IO over time'}\n{panels[0][1]}",
                      fontsize=11, loc="left")

    # Panel 4: measured KV transfer bandwidth.
    ax_bw = axes[3]
    ax_bw.plot(d["t"], d["load_bw"], color="#F58518", label="KV load (CPU→GPU)")
    ax_bw.plot(d["t"], d["store_bw"], color="#E45756", label="KV store (GPU→CPU)")
    ax_bw.set_ylabel("bandwidth (GiB/s)")
    ax_bw.set_title("KV transfer bandwidth", fontsize=11, loc="left")
    ax_bw.set_ylim(bottom=0)
    ax_bw.legend(loc="upper right", fontsize=9)
    ax_bw.grid(True, alpha=0.3)
    ax_bw.set_xlabel("wall time (s)")

    fig.tight_layout()
    fig.savefig(out, dpi=120)
    print(f"wrote {out}")

    # Totals, for a one-line compute-vs-IO verdict.
    rows = load(args.csv)
    tot_load = float(rows[-1]["load_time_s"]) - float(rows[0]["load_time_s"])
    tot_store = float(rows[-1]["store_time_s"]) - float(rows[0]["store_time_s"])
    tot_pref = float(rows[-1]["prefill_sum_s"]) - float(rows[0]["prefill_sum_s"])
    tot_compute = max(tot_pref - tot_load, 0.0)
    io = tot_load + tot_store
    denom = tot_compute + io
    if denom > 0:
        print(f"totals (device-seconds): GPU-compute={tot_compute:.1f}  "
              f"KV-load={tot_load:.1f}  KV-store={tot_store:.1f}  "
              f"=> IO is {100*io/denom:.0f}% of (compute+IO)")


if __name__ == "__main__":
    main()
