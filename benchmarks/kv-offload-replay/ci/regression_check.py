#!/usr/bin/env python3
"""CI regression check for KV-offload replay throughput.

Modes:
  --calibrate   Run replay and save results as the local machine baseline.
                Warns if the machine is >25% slower than the reference baseline
                checked into the repo.
  (default)     Run replay and compare against the local machine baseline.
                Exits 1 on regression (throughput below tolerance).

Local baselines are stored at --local-baselines (default:
/var/lib/certus-ci/baselines.json) so they persist across builds but are
machine-specific.
"""

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REFERENCE_BASELINES_PATH = SCRIPT_DIR / "baselines.json"
LOCAL_BASELINES_PATH = Path("/var/lib/certus-ci/baselines.json")
REPLAY_SCRIPT = SCRIPT_DIR.parent / "replay_offloading_traces.py"

HARDWARE_WARN_PCT = 25


def run_replay(connector: str, trace: str, num_blocks: int) -> dict:
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
        output_path = f.name

    cmd = [
        sys.executable, str(REPLAY_SCRIPT),
        "--trace", trace,
        "--connector", connector,
        "--num-blocks", str(num_blocks),
        "--output-json", output_path,
    ]
    print(f"[regression] running: {' '.join(cmd)}")
    subprocess.run(cmd, capture_output=False)

    try:
        with open(output_path) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return None


def calibrate(connector: str, results: dict, reference_baselines: dict,
              local_baselines_path: Path) -> bool:
    actual_tp = results["handler"]["throughput_mbps"]
    hit_ratio = results["manager"]["lookup"]["hit_ratio"]

    local_baselines_path.parent.mkdir(parents=True, exist_ok=True)

    if local_baselines_path.exists():
        with open(local_baselines_path) as f:
            local = json.load(f)
    else:
        local = {}

    local[connector] = {
        "throughput_mbps": actual_tp,
        "hit_ratio": hit_ratio,
        "tolerance_pct": 15,
    }

    with open(local_baselines_path, "w") as f:
        json.dump(local, f, indent=2)
        f.write("\n")

    print(f"\n[calibrate] saved local baseline for '{connector}':")
    print(f"  throughput: {actual_tp:.1f} MB/s")
    print(f"  hit_ratio:  {hit_ratio:.4f}")
    print(f"  file:       {local_baselines_path}")

    ref = reference_baselines.get(connector)
    if ref:
        ref_tp = ref["throughput_mbps"]
        deficit_pct = (1 - actual_tp / ref_tp) * 100
        if deficit_pct > HARDWARE_WARN_PCT:
            print(f"\n[calibrate] WARNING: this machine is {deficit_pct:.1f}% slower "
                  f"than the reference hardware ({actual_tp:.1f} vs {ref_tp:.1f} MB/s)")
            print(f"  This exceeds the {HARDWARE_WARN_PCT}% hardware capability threshold.")
            print(f"  Verify that the hardware (NVMe/CXL) is performing as expected.")
            return False

    return True


def check_regression(connector: str, results: dict, baselines: dict) -> bool:
    baseline = baselines.get(connector)
    if not baseline:
        print(f"[regression] no local baseline for connector '{connector}'")
        print(f"  Run with --calibrate first to establish this machine's baseline.")
        return False

    actual_tp = results["handler"]["throughput_mbps"]
    expected_tp = baseline["throughput_mbps"]
    tolerance = baseline["tolerance_pct"] / 100.0
    lower_bound = expected_tp * (1 - tolerance)

    passed = actual_tp >= lower_bound

    status = "PASS" if passed else "FAIL"
    print(f"\n[regression] {status}: {connector}")
    print(f"  throughput: {actual_tp:.1f} MB/s (baseline: {expected_tp:.1f} MB/s)")
    print(f"  tolerance:  {baseline['tolerance_pct']}% (lower bound: {lower_bound:.1f} MB/s)")

    if not passed:
        deficit_pct = (1 - actual_tp / expected_tp) * 100
        print(f"  regression: -{deficit_pct:.1f}% below baseline")

    return passed


def main():
    parser = argparse.ArgumentParser(description="KV-offload replay regression check")
    parser.add_argument("--connector", required=True, choices=["cpu", "fs", "certus"])
    parser.add_argument("--trace", required=True)
    parser.add_argument("--num-blocks", type=int, default=32768)
    parser.add_argument("--calibrate", action="store_true",
                        help="Run replay and save as local machine baseline")
    parser.add_argument("--local-baselines", type=Path, default=LOCAL_BASELINES_PATH,
                        help="Path to machine-local baselines file")
    parser.add_argument("--reference-baselines", type=Path, default=REFERENCE_BASELINES_PATH,
                        help="Path to repo reference baselines (for calibration warnings)")
    args = parser.parse_args()

    results = run_replay(args.connector, args.trace, args.num_blocks)
    if results is None:
        print(f"[regression] FAIL: replay produced no results")
        sys.exit(1)

    if args.calibrate:
        with open(args.reference_baselines) as f:
            reference = json.load(f)
        ok = calibrate(args.connector, results, reference, args.local_baselines)
        sys.exit(0 if ok else 2)
    else:
        if not args.local_baselines.exists():
            print(f"[regression] no local baselines at {args.local_baselines}")
            print(f"  Run with --calibrate first.")
            sys.exit(1)
        with open(args.local_baselines) as f:
            baselines = json.load(f)
        if not check_regression(args.connector, results, baselines):
            sys.exit(1)


if __name__ == "__main__":
    main()
