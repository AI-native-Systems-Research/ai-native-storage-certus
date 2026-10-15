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
average over the interval, ~2 requests' worth of that activity was in flight. The
compute:IO ratio of the two bands is what tells you whether the run is compute- or
IO-bound. A second panel shows raw KV transfer bandwidth (GB/s).

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

    fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(11, 7.5), sharex=True)

    # Panel 1: GPU compute vs KV IO, as concurrent device-seconds per wall-second.
    ax1.stackplot(
        d["t"], d["compute"], d["load_io"], d["store_io"],
        labels=["GPU prefill-compute", "KV load IO (CPU→GPU)", "KV store IO (GPU→CPU)"],
        colors=["#4C78A8", "#F58518", "#E45756"], alpha=0.85,
    )
    ax1.set_ylabel("device-seconds per wall-second\n(≈ concurrent streams active)")
    ax1.set_title(args.title or "GPU compute vs KV-IO over time", fontsize=12)
    ax1.legend(loc="upper right", fontsize=9)
    ax1.grid(True, alpha=0.3)

    # Panel 2: measured KV transfer bandwidth.
    ax2.plot(d["t"], d["load_bw"], color="#F58518", label="KV load (CPU→GPU)")
    ax2.plot(d["t"], d["store_bw"], color="#E45756", label="KV store (GPU→CPU)")
    ax2.set_ylabel("KV transfer bandwidth (GiB/s)")
    ax2.set_xlabel("wall time (s)")
    ax2.legend(loc="upper right", fontsize=9)
    ax2.grid(True, alpha=0.3)

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
