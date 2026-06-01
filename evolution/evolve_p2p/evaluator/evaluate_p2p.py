"""Workload-first evaluator for the P2P evolution experiment.

Scores candidates on GPU data-delivery performance: throughput and tail latency.
Does NOT use CPU utilization in scoring (measured baseline: 3.2% — not discriminating).
CPU metrics are logged for post-hoc analysis only.

Flow: patch → build → start server → benchmark → verify integrity → score → restore.

Fitness (until scalability/stability are implemented):
  0.60 * throughput (cold lookup aggregate GB/s)
  0.40 * latency   (inverse of p99 cold lookup latency)

Hard constraints:
  - Build must succeed (score = 0.0 if not)
  - Data integrity must pass (score = -1.0 if not)
  - p99 latency must be parseable (score = 0.0 if not)
"""

from __future__ import annotations

import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
DISPATCHER_SRC = REPO_ROOT / "components" / "dispatcher" / "src"
PIPELINE_RS = DISPATCHER_SRC / "pipeline.rs"
LIB_RS = DISPATCHER_SRC / "lib.rs"
DMA_RS = REPO_ROOT / "components" / "gpu-services" / "src" / "dma.rs"
SERVICE_RS = REPO_ROOT / "apps" / "certus-server" / "src" / "service.rs"
SERVER_BIN = REPO_ROOT / "target" / "release" / "certus-server"
BENCH_SCRIPT = REPO_ROOT / "apps" / "python" / "certus-api-bench.py"
SYSTEM_PYTHON = "/usr/bin/python3"

MULTI_FILE_MAP = {
    "pipeline.rs": PIPELINE_RS,
    "lib.rs": LIB_RS,
    "dma.rs": DMA_RS,
    "service.rs": SERVICE_RS,
}

ALL_TARGETS = set(MULTI_FILE_MAP.values())


def restore_stale_backups():
    """On startup, restore any .bak files left by a previous crash."""
    for target in ALL_TARGETS:
        bak = target.with_suffix(target.suffix + ".bak")
        if bak.exists():
            shutil.copy2(bak, target)
            bak.unlink()


restore_stale_backups()

# Server config — override via env vars
DATA_PCI_LIST = os.environ.get(
    "CERTUS_DATA_PCI", "0000:62:00.0"
).split(",")

GRPC_PORT = 50051
SERVER_STARTUP_TIMEOUT = 15
BUILD_TIMEOUT = 120
BENCH_TIMEOUT = 90

# Scoring ceilings (from measured baselines — Section 2.5 of experiment doc)
THROUGHPUT_CEILING_GBPS = 12.0  # Above best observed (7.11 GB/s, 7 drives single client)
LATENCY_TARGET_MS = 0.4         # Below best observed p50 (382us on 4 drives)


def kill_server():
    subprocess.run(["pkill", "-x", "certus-server"], capture_output=True, timeout=5)
    time.sleep(1)
    subprocess.run(["pkill", "-9", "-x", "certus-server"], capture_output=True, timeout=5)
    time.sleep(1)


def build_server() -> tuple[bool, str]:
    try:
        result = subprocess.run(
            ["cargo", "build", "-p", "certus-server", "--release"],
            capture_output=True, text=True,
            timeout=BUILD_TIMEOUT, cwd=REPO_ROOT,
        )
        if result.returncode != 0:
            return False, result.stderr[-2000:]
        return True, ""
    except subprocess.TimeoutExpired:
        return False, f"Build timed out ({BUILD_TIMEOUT}s)"
    except Exception as e:
        return False, str(e)


def start_server() -> tuple[bool, str]:
    cmd = [str(SERVER_BIN)]
    for pci in DATA_PCI_LIST:
        cmd.extend(["--device-pci", pci.strip()])
    cmd.append("--format")

    try:
        proc = subprocess.Popen(
            cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            preexec_fn=os.setsid,
        )
    except Exception as e:
        return False, str(e)

    deadline = time.time() + SERVER_STARTUP_TIMEOUT
    while time.time() < deadline:
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(1)
            s.connect(("localhost", GRPC_PORT))
            s.close()
            return True, f"PID {proc.pid}"
        except (ConnectionRefusedError, OSError):
            time.sleep(0.5)

    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except ProcessLookupError:
        pass
    return False, "Server startup timeout"


def read_cpu_stats() -> dict:
    with open("/proc/stat") as f:
        line = f.readline()
    parts = line.split()
    vals = [int(x) for x in parts[1:]]
    return {"total": sum(vals), "idle": vals[3] + vals[4]}


def measure_cpu_during(func):
    """Run func() while measuring CPU utilization. Returns (result, cpu_fraction)."""
    before = read_cpu_stats()
    result = func()
    after = read_cpu_stats()
    total_delta = after["total"] - before["total"]
    idle_delta = after["idle"] - before["idle"]
    cpu_fraction = 1.0 - (idle_delta / total_delta) if total_delta > 0 else 0.0
    return result, cpu_fraction


def run_benchmark() -> tuple[float | None, dict, str]:
    """Run certus-api-bench.py, return (throughput_gbps, latency_dict, raw_output)."""
    cmd = [
        SYSTEM_PYTHON, str(BENCH_SCRIPT),
        "--server", f"localhost:{GRPC_PORT}",
        "--clients", "1",
        "--num-objects", "16",
        "--iterations", "10",
        "--block-size", str(4 * 1024 * 1024),
    ]

    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True,
            timeout=BENCH_TIMEOUT, cwd=BENCH_SCRIPT.parent,
        )
    except subprocess.TimeoutExpired:
        return None, {}, "Benchmark timed out"
    except Exception as e:
        return None, {}, str(e)

    output = result.stdout + result.stderr

    # Check for errors reported by benchmark
    error_match = re.search(r"ERRORS \((\d+)\)", output)
    if error_match and int(error_match.group(1)) > 0:
        return None, {}, f"Benchmark reported errors: {output[-500:]}"

    # Parse cold lookup throughput and latency
    cold_section = False
    throughput_gbps = None
    latency = {}
    for line in output.split("\n"):
        if "Lookup (cold)" in line:
            cold_section = True
            for key in ("avg", "p50", "p99"):
                m = re.search(rf"{key}=\s*([\d.]+)\s*us", line)
                if m:
                    latency[f"{key}_us"] = float(m.group(1))
            continue
        if cold_section and "aggregate=" in line:
            m = re.search(r"aggregate=\s*([\d.]+)\s*GB/s", line)
            if m:
                throughput_gbps = float(m.group(1))
            break

    if throughput_gbps is None:
        return None, {}, f"Could not parse throughput:\n{output[-800:]}"

    return throughput_gbps, latency, output


def fitness(metrics: dict) -> float:
    """Workload-first fitness. Rewards throughput and low latency.

    CPU is NOT used — baseline is 3.2% on 64-core SPDK system, not discriminating.
    """
    if not metrics.get("build_succeeded", True):
        return 0.0
    if not metrics.get("data_integrity", False):
        return -1.0

    throughput_gbps = metrics.get("throughput_gbps")
    p99_ms = metrics.get("p99_latency_ms")

    if throughput_gbps is None or p99_ms is None:
        return 0.0

    throughput = min(1.0, throughput_gbps / THROUGHPUT_CEILING_GBPS)
    latency = min(1.0, LATENCY_TARGET_MS / max(0.01, p99_ms))

    return round(0.60 * throughput + 0.40 * latency, 4)


def evaluate(candidate: str | dict) -> tuple[float, dict]:
    """GEPA-compatible evaluator. Scores only throughput + latency."""
    import gepa.optimize_anything as oa

    t_start = time.time()

    if isinstance(candidate, str):
        files = {"pipeline.rs": candidate}
    else:
        files = candidate

    restore_stale_backups()

    # Backup all target files
    backups = {}
    for target in ALL_TARGETS:
        if target.exists():
            bak = target.with_suffix(target.suffix + ".bak")
            shutil.copy2(target, bak)
            backups[target] = bak

    # Signal handler — restore on kill
    def _restore_on_signal(signum, frame):
        for t, b in backups.items():
            if b.exists():
                shutil.copy2(b, t)
                b.unlink()
        kill_server()
        sys.exit(128 + signum)

    prev_sigterm = signal.signal(signal.SIGTERM, _restore_on_signal)
    prev_sigint = signal.signal(signal.SIGINT, _restore_on_signal)

    try:
        # Patch
        patched = []
        for filename, content in files.items():
            target = MULTI_FILE_MAP.get(filename)
            if target and target.exists():
                target.write_text(content)
                patched.append(filename)

        if not patched:
            oa.log("No recognized files in candidate")
            return 0.0, {"error": "No recognized files", "build_succeeded": False}

        # Build
        ok, msg = build_server()
        if not ok:
            oa.log(f"Build failed: {msg[:500]}")
            return 0.0, {"error": msg[:500], "build_succeeded": False}

        # Start server
        kill_server()
        ok, msg = start_server()
        if not ok:
            oa.log(f"Server failed: {msg}")
            return 0.0, {"error": msg, "build_succeeded": True}

        # Benchmark with CPU measurement (CPU logged, not scored)
        def _bench():
            return run_benchmark()

        (throughput_gbps, latency, output), cpu_fraction = measure_cpu_during(_bench)

        if throughput_gbps is None:
            oa.log(f"Benchmark failed: {output[:500]}")
            return 0.0, {"error": output[:500], "build_succeeded": True}

        # Validate p99 was parsed
        if "p99_us" not in latency:
            oa.log("Missing p99 latency in benchmark output")
            return 0.0, {"error": "Missing p99 latency", "build_succeeded": True}

        # Data integrity: check benchmark reported no errors
        # The benchmark itself verifies data by checking GPU buffer patterns
        # and reports ERRORS (N) if any lookups returned wrong data.
        has_errors = "ERRORS" in output and not "ERRORS (0)" in output
        data_ok = not has_errors

        # Assemble metrics
        metrics = {
            "throughput_gbps": throughput_gbps,
            "p99_latency_ms": latency["p99_us"] / 1000.0,
            "p50_latency_ms": latency.get("p50_us", 0) / 1000.0,
            "mean_latency_ms": latency.get("avg_us", 0) / 1000.0,
            "cpu_util_fraction": cpu_fraction,  # Logged, not scored
            "data_integrity": data_ok,
            "build_succeeded": True,
        }

        score = fitness(metrics)

        elapsed = time.time() - t_start
        oa.log(f"Score: {score:.4f} | Throughput: {throughput_gbps:.2f} GB/s | "
               f"p99: {latency['p99_us']:.0f}us | CPU: {cpu_fraction:.1%} | "
               f"Integrity: {'PASS' if data_ok else 'FAIL'} | "
               f"Time: {elapsed:.0f}s | Files: {patched}")

        return score, metrics

    finally:
        for target, bak in backups.items():
            if bak.exists():
                shutil.copy2(bak, target)
                bak.unlink()
        kill_server()
        signal.signal(signal.SIGTERM, prev_sigterm)
        signal.signal(signal.SIGINT, prev_sigint)


if __name__ == "__main__":
    if "--test" in sys.argv:
        print("Evaluating wild-type (current code)...")
        ok, msg = build_server()
        if not ok:
            print(f"Build failed: {msg}")
            sys.exit(1)

        kill_server()
        ok, msg = start_server()
        if not ok:
            print(f"Server failed: {msg}")
            sys.exit(1)

        (tput, lat, out), cpu = measure_cpu_during(run_benchmark)
        kill_server()

        if tput is None:
            print(f"Benchmark failed: {out}")
            sys.exit(1)

        has_errors = "ERRORS" in out and "ERRORS (0)" not in out
        metrics = {
            "throughput_gbps": tput,
            "p99_latency_ms": lat.get("p99_us", 1e9) / 1000.0,
            "p50_latency_ms": lat.get("p50_us", 0) / 1000.0,
            "mean_latency_ms": lat.get("avg_us", 0) / 1000.0,
            "cpu_util_fraction": cpu,
            "data_integrity": not has_errors,
            "build_succeeded": True,
        }
        score = fitness(metrics)
        print(f"\n{'='*50}")
        print(json.dumps(metrics, indent=2))
        print(f"\nFitness: {score:.4f}")
    else:
        print("Usage: python evaluate_p2p.py --test")
