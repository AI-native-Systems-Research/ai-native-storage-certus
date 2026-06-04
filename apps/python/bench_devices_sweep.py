#!/usr/bin/env python3
"""Sweep number of NVMe devices (1..8) with fixed 16 clients and plot throughput.

Launches certus-server with increasing device counts, runs certus-api-bench.py
for each configuration, parses aggregate throughput, and produces a plot.

Usage:
    python bench_devices_sweep.py [--server-bin PATH] [--iterations 10]
"""

import argparse
import os
import re
import signal
import subprocess
import sys
import time

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
DEFAULT_SERVER_BIN = os.path.join(
    SCRIPT_DIR, "..", "..", "target", "release", "certus-server"
)

ALL_DEVICES = [
    "0000:61:00.0",
    "0000:62:00.0",
    "0000:63:00.0",
    "0000:64:00.0",
    "0000:c1:00.0",
    "0000:c2:00.0",
    "0000:c3:00.0",
#    "0000:c4:00.0",
]

BENCH_SCRIPT = os.path.join(SCRIPT_DIR, "certus-api-bench.py")
PYTHON = "python3.12"
LISTEN_ADDR = "0.0.0.0:50051"
SERVER_ADDR = "localhost:50051"
NUM_CLIENTS = 16


def start_server(server_bin, devices):
    """Start certus-server with the given device list. Returns Popen."""
    cmd = [server_bin, "--listen", LISTEN_ADDR]
    for dev in devices:
        cmd.extend(["--device-pci", dev])
    print(f"  Starting server with {len(devices)} device(s): {' '.join(devices)}")
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        preexec_fn=os.setsid,
    )
    time.sleep(5)
    if proc.poll() is not None:
        out = proc.stdout.read().decode(errors="replace")
        raise RuntimeError(f"Server exited early (rc={proc.returncode}):\n{out}")
    return proc


def stop_server(proc):
    """Gracefully stop the server process group."""
    if proc and proc.poll() is None:
        os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            proc.wait()


def run_benchmark(iterations, num_objects):
    """Run certus-api-bench.py and return (populate_gbps, hot_gbps, cold_gbps)."""
    cmd = [
        PYTHON,
        BENCH_SCRIPT,
        "--server", SERVER_ADDR,
        "--clients", str(NUM_CLIENTS),
        "--iterations", str(iterations),
        "--num-objects", str(num_objects),
    ]
    print(f"  Running benchmark: {' '.join(cmd)}")
    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=600,
        cwd=SCRIPT_DIR,
    )
    output = result.stdout + result.stderr
    print(f"  Benchmark exit code: {result.returncode}")
    if result.returncode != 0:
        # Show last 20 lines of output for debugging
        lines = output.strip().split("\n")
        for line in lines[-20:]:
            print(f"    | {line}")

    populate_tp = parse_aggregate_throughput(output, "Populate")
    hot_tp = parse_aggregate_throughput(output, "Lookup (hot)")
    cold_tp = parse_aggregate_throughput(output, "Lookup (cold)")

    print(f"  Results: populate={populate_tp:.2f} GB/s, hot={hot_tp:.2f} GB/s, cold={cold_tp:.2f} GB/s")
    return populate_tp, hot_tp, cold_tp


def parse_aggregate_throughput(output, label):
    """Parse aggregate throughput in GB/s from benchmark output for a given label."""
    # The output format after each label's stats line has:
    #   aggregate=  X.XX GB/s
    lines = output.split("\n")
    found_label = False
    for line in lines:
        if label in line and "us" in line:
            found_label = True
            continue
        if found_label and "aggregate=" in line:
            match = re.search(r"aggregate=\s*([\d.]+)\s*GB/s", line)
            if match:
                return float(match.group(1))
            found_label = False
    return 0.0


def make_plot(device_counts, populate_tp, hot_tp, cold_tp, output_path):
    """Generate throughput vs. device-count plot."""
    fig, ax = plt.subplots(figsize=(10, 6))

    x = np.array(device_counts)
    ax.plot(x, populate_tp, "o-", linewidth=2, markersize=8, label="Populate", color="#2196F3")
    ax.plot(x, hot_tp, "s-", linewidth=2, markersize=8, label="Lookup (hot)", color="#4CAF50")
    ax.plot(x, cold_tp, "^-", linewidth=2, markersize=8, label="Lookup (cold)", color="#FF9800")

    ax.set_xlabel("Number of NVMe Devices", fontsize=12)
    ax.set_ylabel("Aggregate Throughput (GB/s)", fontsize=12)
    ax.set_title(f"Certus Throughput vs. Device Count (4 MiB blocks, {NUM_CLIENTS} clients)", fontsize=13)
    ax.set_xticks(x)
    ax.legend(fontsize=11)
    ax.grid(True, alpha=0.3)
    ax.set_xlim(0.5, max(device_counts) + 0.5)
    ax.set_ylim(bottom=0)

    plt.tight_layout()
    plt.savefig(output_path, dpi=150)
    print(f"\nPlot saved to: {output_path}")


def main():
    parser = argparse.ArgumentParser(description="Sweep device count and plot throughput")
    parser.add_argument(
        "--server-bin",
        default=DEFAULT_SERVER_BIN,
        help=f"Path to certus-server binary (default: {DEFAULT_SERVER_BIN})",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=10,
        help="Benchmark iterations per phase (default: 10)",
    )
    parser.add_argument(
        "--num-objects",
        type=int,
        default=16,
        help="Objects per lookup batch per client (default: 16)",
    )
    parser.add_argument(
        "--max-devices",
        type=int,
        default=8,
        help="Maximum number of devices to sweep (default: 8)",
    )
    parser.add_argument(
        "--output",
        default=os.path.join(SCRIPT_DIR, "throughput_vs_devices.png"),
        help="Output plot file path",
    )
    args = parser.parse_args()

    if not os.path.isfile(args.server_bin):
        print(f"ERROR: server binary not found: {args.server_bin}")
        sys.exit(1)

    max_dev = min(args.max_devices, len(ALL_DEVICES))
    device_counts = list(range(1, max_dev + 1))

    populate_results = []
    hot_results = []
    cold_results = []

    print(f"{'='*70}")
    print(f"Device Sweep Benchmark")
    print(f"{'='*70}")
    print(f"  Clients:       {NUM_CLIENTS}")
    print(f"  Iterations:    {args.iterations}")
    print(f"  Objects/batch: {args.num_objects}")
    print(f"  Device sweep:  1..{max_dev}")
    print(f"  Server binary: {args.server_bin}")
    print()

    for n_dev in device_counts:
        devices = ALL_DEVICES[:n_dev]
        print(f"\n{'─'*70}")
        print(f"  Configuration: {n_dev} device(s)")
        print(f"{'─'*70}")

        server_proc = None
        try:
            server_proc = start_server(args.server_bin, devices)
            pop_tp, hot_tp, cold_tp = run_benchmark(args.iterations, args.num_objects)
            populate_results.append(pop_tp)
            hot_results.append(hot_tp)
            cold_results.append(cold_tp)
        except Exception as e:
            print(f"  ERROR: {e}")
            populate_results.append(0)
            hot_results.append(0)
            cold_results.append(0)
        finally:
            stop_server(server_proc)
            time.sleep(2)

    print(f"\n{'='*70}")
    print("Summary")
    print(f"{'='*70}")
    print(f"  {'Devices':<10} {'Populate (GB/s)':<18} {'Hot (GB/s)':<15} {'Cold (GB/s)':<15}")
    print(f"  {'─'*55}")
    for i, n_dev in enumerate(device_counts):
        print(
            f"  {n_dev:<10} {populate_results[i]:<18.2f} "
            f"{hot_results[i]:<15.2f} {cold_results[i]:<15.2f}"
        )

    make_plot(device_counts, populate_results, hot_results, cold_results, args.output)


if __name__ == "__main__":
    main()
