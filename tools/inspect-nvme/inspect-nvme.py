#!/usr/bin/env python3
"""NVMe device inspection tool for Certus benchmark tuning.

Characterizes NVMe drive behavior under conditions relevant to the Certus
cold-path pipeline: GC settle time after writes, power state transitions,
DRAM write cache, and read latency at various queue depths.

Requires: root/sudo, nvme-cli, fio
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time


def run_cmd(args, check=True, capture=True):
    """Run a shell command and return stdout."""
    result = subprocess.run(
        args, capture_output=capture, text=True,
        timeout=300 if not any("fio" in str(a) for a in args) else 600,
    )
    if check and result.returncode != 0:
        return None
    return result.stdout if capture else ""


def run_fio(device, rw, bs, size=None, runtime=None, qdepth=1,
            numjobs=1, direct=True, fsync=0, offset=None):
    """Run fio and return parsed JSON results."""
    cmd = [
        "fio", "--name=test", f"--filename={device}",
        f"--rw={rw}", f"--bs={bs}", f"--iodepth={qdepth}",
        f"--numjobs={numjobs}", "--group_reporting",
        "--ioengine=libaio", "--output-format=json",
        "--time_based" if runtime else "--size_based",
    ]
    if direct:
        cmd.append("--direct=1")
    if runtime:
        cmd.append(f"--runtime={runtime}")
    if size:
        cmd.append(f"--size={size}")
    if fsync:
        cmd.append(f"--fsync={fsync}")
    if offset:
        cmd.append(f"--offset={offset}")

    result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    if result.returncode != 0:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


def extract_lat_us(fio_result, rw_type="read"):
    """Extract latency stats from fio JSON output in microseconds."""
    if not fio_result or "jobs" not in fio_result:
        return None
    job = fio_result["jobs"][0]
    lat = job.get(rw_type, {}).get("clat_ns", job.get(rw_type, {}).get("lat_ns", {}))
    if not lat:
        return None
    return {
        "avg": lat.get("mean", 0) / 1000,
        "p50": lat.get("percentile", {}).get("50.000000", 0) / 1000,
        "p99": lat.get("percentile", {}).get("99.000000", 0) / 1000,
        "max": lat.get("max", 0) / 1000,
    }


class NvmeInspector:
    def __init__(self, device):
        # Handle both /dev/nvme4 and /dev/nvme4n1
        self.raw_device = device
        match = re.match(r"(/dev/nvme\d+)(n\d+)?", device)
        if not match:
            print(f"Error: invalid NVMe device path: {device}", file=sys.stderr)
            sys.exit(1)
        self.ctrl = match.group(1)
        self.ns = device if match.group(2) else f"{device}n1"
        self.results = {}

    def identify(self):
        """Gather device identification via sysfs and nvme-cli."""
        info = {}
        ctrl_name = os.path.basename(self.ctrl)
        sysfs = f"/sys/class/nvme/{ctrl_name}"

        for attr in ["model", "firmware_rev", "serial", "numa_node", "state"]:
            path = os.path.join(sysfs, attr)
            if os.path.exists(path):
                with open(path) as f:
                    info[attr] = f.read().strip()

        # Namespace count
        ns_count = 0
        if os.path.isdir(sysfs):
            for entry in os.listdir(sysfs):
                if re.match(rf"{ctrl_name}n\d+", entry):
                    ns_count += 1
        info["namespaces"] = ns_count

        # Capacity from nvme-cli
        id_ns = run_cmd(["nvme", "id-ns", self.ns, "-o", "json"], check=False)
        if id_ns:
            try:
                ns_data = json.loads(id_ns)
                nsze = ns_data.get("nsze", 0)
                lba_size = 512
                lbaf = ns_data.get("lbafs", [{}])
                if lbaf:
                    ds = lbaf[0].get("ds", 9)
                    lba_size = 1 << ds
                info["capacity_bytes"] = nsze * lba_size
                info["block_size"] = lba_size
            except (json.JSONDecodeError, KeyError):
                pass

        # MDTS from identify controller
        id_ctrl = run_cmd(["nvme", "id-ctrl", self.ctrl, "-o", "json"], check=False)
        if id_ctrl:
            try:
                ctrl_data = json.loads(id_ctrl)
                mdts = ctrl_data.get("mdts", 0)
                if mdts > 0:
                    # MDTS is in units of minimum memory page size (4KiB) power of 2
                    info["mdts_bytes"] = (1 << mdts) * 4096
                info["nn"] = ctrl_data.get("nn", 0)
                # Power states
                npss = ctrl_data.get("npss", 0)
                info["power_states"] = npss + 1 if npss else 0
                ps_list = []
                for i in range(info.get("power_states", 0)):
                    ps = ctrl_data.get(f"ps{i}", ctrl_data.get("psd", [{}] * (i + 1))[i] if "psd" in ctrl_data else {})
                    if isinstance(ps, dict):
                        ps_list.append(ps)
                info["ps_details"] = ps_list
                # VWC
                info["vwc"] = ctrl_data.get("vwc", 0)
            except (json.JSONDecodeError, KeyError):
                pass

        # APST feature
        apst = run_cmd(["nvme", "get-feature", self.ctrl, "-f", "0x0c", "-o", "json"], check=False)
        if apst:
            try:
                apst_data = json.loads(apst)
                info["apst_enabled"] = apst_data.get("result", 0) != 0
            except (json.JSONDecodeError, KeyError):
                info["apst_enabled"] = None
        else:
            info["apst_enabled"] = None

        self.results["identify"] = info
        return info

    def measure_gc_settle(self, write_gb=2, intervals=None):
        """Write heavily, then measure read latency at intervals to find GC settle time."""
        if intervals is None:
            intervals = [0, 5, 10, 15, 20, 30, 45, 60]

        print(f"  Writing {write_gb} GiB to trigger GC...", end="", flush=True)
        # Sequential write to fill SSD write buffer
        run_fio(self.ns, rw="write", bs="4M", size=f"{write_gb}G", qdepth=32)
        print(" done.")

        measurements = []
        for wait_s in intervals:
            if wait_s > 0:
                print(f"  Waiting {wait_s}s...", end="", flush=True)
                time.sleep(wait_s - (intervals[intervals.index(wait_s) - 1] if intervals.index(wait_s) > 0 else 0))
                print(" measuring...", end="", flush=True)
            else:
                print("  Measuring immediately...", end="", flush=True)

            result = run_fio(
                self.ns, rw="randread", bs="128k", runtime="3",
                qdepth=1, offset="0",
            )
            lat = extract_lat_us(result, "read")
            avg = lat["avg"] if lat else 0
            measurements.append({"elapsed_s": wait_s, "avg_us": avg, "p99_us": lat["p99"] if lat else 0})
            print(f" {avg:.0f} us")

        # Find settle point (within 10% of minimum)
        avgs = [m["avg_us"] for m in measurements if m["avg_us"] > 0]
        if avgs:
            min_lat = min(avgs)
            settle_time = 0
            for m in measurements:
                if m["avg_us"] > 0 and m["avg_us"] <= min_lat * 1.1:
                    settle_time = m["elapsed_s"]
                    break
            recommended = settle_time
        else:
            recommended = 30

        result = {
            "measurements": measurements,
            "recommended_gc_settle_s": recommended,
        }
        self.results["gc_settle"] = result
        return result

    def measure_power_states(self, intervals=None):
        """Measure read latency after various idle durations to detect power state entry."""
        if intervals is None:
            intervals = [0, 5, 10, 30, 60]

        # Warm up the drive first
        print("  Warming up drive...", end="", flush=True)
        run_fio(self.ns, rw="randread", bs="128k", runtime="5", qdepth=4)
        print(" done.")

        measurements = []
        for idle_s in intervals:
            if idle_s > 0:
                print(f"  Idle {idle_s}s...", end="", flush=True)
                time.sleep(idle_s)
            else:
                print("  Baseline (no idle)...", end="", flush=True)

            # Single read to measure first-access latency
            result = run_fio(
                self.ns, rw="randread", bs="128k",
                size="128k", qdepth=1,
            )
            lat = extract_lat_us(result, "read")
            avg = lat["avg"] if lat else 0
            measurements.append({"idle_s": idle_s, "avg_us": avg})
            print(f" {avg:.0f} us")

            # Re-warm between tests
            if idle_s < intervals[-1]:
                run_fio(self.ns, rw="randread", bs="128k", runtime="2", qdepth=4)

        # Detect power state transitions (>2x baseline)
        baseline = measurements[0]["avg_us"] if measurements and measurements[0]["avg_us"] > 0 else 500
        transitions = []
        for m in measurements[1:]:
            if m["avg_us"] > baseline * 2:
                transitions.append(m["idle_s"])

        result = {
            "measurements": measurements,
            "baseline_us": baseline,
            "power_state_transitions_at_s": transitions,
            "recommendation": "Disable APST for benchmarks" if transitions else "No power state issues detected",
        }
        self.results["power_states"] = result
        return result

    def check_write_cache(self):
        """Detect DRAM write cache behavior by comparing write latency with/without fsync."""
        print("  Measuring write latency without fsync...", end="", flush=True)
        result_no_sync = run_fio(
            self.ns, rw="write", bs="4M", size="64M", qdepth=1, fsync=0,
        )
        lat_no_sync = extract_lat_us(result_no_sync, "write")
        print(f" {lat_no_sync['avg']:.0f} us" if lat_no_sync else " failed")

        print("  Measuring write latency with fsync...", end="", flush=True)
        result_sync = run_fio(
            self.ns, rw="write", bs="4M", size="64M", qdepth=1, fsync=1,
        )
        lat_sync = extract_lat_us(result_sync, "write")
        print(f" {lat_sync['avg']:.0f} us" if lat_sync else " failed")

        vwc_enabled = self.results.get("identify", {}).get("vwc", 0) & 0x1

        result = {
            "vwc_enabled": bool(vwc_enabled),
            "write_no_fsync_us": lat_no_sync["avg"] if lat_no_sync else None,
            "write_with_fsync_us": lat_sync["avg"] if lat_sync else None,
            "cache_ratio": (lat_sync["avg"] / lat_no_sync["avg"]) if lat_no_sync and lat_sync and lat_no_sync["avg"] > 0 else None,
        }
        self.results["write_cache"] = result
        return result

    def read_latency_profile(self, queue_depths=None):
        """Measure steady-state read latency at various queue depths."""
        if queue_depths is None:
            queue_depths = [1, 4, 16, 64, 128]

        # Pre-condition: sequential read to warm NAND pages
        print("  Pre-conditioning (sequential read)...", end="", flush=True)
        run_fio(self.ns, rw="read", bs="128k", runtime="5", qdepth=32)
        print(" done.")

        measurements = []
        for qd in queue_depths:
            print(f"  QD={qd:>3}...", end="", flush=True)
            result = run_fio(
                self.ns, rw="randread", bs="128k", runtime="5", qdepth=qd,
            )
            lat = extract_lat_us(result, "read")
            if lat:
                measurements.append({"queue_depth": qd, **lat})
                print(f" avg={lat['avg']:.0f} us  p99={lat['p99']:.0f} us")
            else:
                measurements.append({"queue_depth": qd, "avg": 0, "p50": 0, "p99": 0, "max": 0})
                print(" failed")

        self.results["read_profile"] = measurements
        return measurements

    def generate_report(self):
        """Generate human-readable report from collected results."""
        lines = []
        sep = "=" * 70
        sub = "-" * 70

        lines.append(sep)
        lines.append("NVMe Device Inspection Report")
        lines.append(sep)

        # Identity
        info = self.results.get("identify", {})
        lines.append(f"Device:          {self.ns}")
        lines.append(f"Model:           {info.get('model', 'unknown')}")
        lines.append(f"Firmware:        {info.get('firmware_rev', 'unknown')}")
        lines.append(f"Serial:          {info.get('serial', 'unknown')}")
        cap = info.get("capacity_bytes", 0)
        if cap:
            lines.append(f"Capacity:        {cap / 1e12:.2f} TB")
        mdts = info.get("mdts_bytes", 0)
        if mdts:
            lines.append(f"MDTS:            {mdts // 1024} KiB")
        lines.append(f"Block Size:      {info.get('block_size', 512)} B")
        lines.append(f"NUMA Node:       {info.get('numa_node', 'unknown')}")
        lines.append(f"Namespaces:      {info.get('namespaces', 'unknown')}")
        lines.append(f"Power States:    {info.get('power_states', 'unknown')}")
        vwc = info.get("vwc", 0)
        lines.append(f"Volatile WC:     {'Enabled' if vwc & 0x1 else 'Disabled'}")
        lines.append("")

        # GC Settle
        gc = self.results.get("gc_settle")
        if gc:
            lines.append(sub)
            lines.append("GC Settle Time")
            lines.append(sub)
            lines.append("Post-write read latency (128 KiB random, QD=1):")
            min_lat = min((m["avg_us"] for m in gc["measurements"] if m["avg_us"] > 0), default=0)
            for m in gc["measurements"]:
                marker = ""
                if m["avg_us"] > 0 and m["avg_us"] <= min_lat * 1.1:
                    marker = "  <-- stable"
                lines.append(f"  {m['elapsed_s']:>2}s after write:  {m['avg_us']:>8,.0f} us{marker}")
            lines.append(f"\nRecommended --gc-settle: {gc['recommended_gc_settle_s']}s")
            lines.append("")

        # Power States
        ps = self.results.get("power_states")
        if ps:
            lines.append(sub)
            lines.append("Power State Behavior")
            lines.append(sub)
            apst = info.get("apst_enabled")
            lines.append(f"APST:            {'Enabled' if apst else 'Disabled' if apst is False else 'Unknown'}")
            lines.append("Idle-to-read latency (128 KiB, first read after idle):")
            for m in ps["measurements"]:
                marker = ""
                if m["avg_us"] > ps["baseline_us"] * 2:
                    marker = "  (power state entry)"
                lines.append(f"  {m['idle_s']:>2}s idle:        {m['avg_us']:>8,.0f} us{marker}")
            if ps["power_state_transitions_at_s"]:
                lines.append(f"\nRecommendation: {ps['recommendation']}")
                lines.append(f"  Command: nvme set-feature {self.ctrl} -f 0x0c -v 0")
            else:
                lines.append(f"\n{ps['recommendation']}")
            lines.append("")

        # Write Cache
        wc = self.results.get("write_cache")
        if wc:
            lines.append(sub)
            lines.append("DRAM Write Cache")
            lines.append(sub)
            lines.append(f"Volatile Write Cache: {'Enabled' if wc['vwc_enabled'] else 'Disabled'}")
            if wc["write_no_fsync_us"] is not None:
                lines.append(f"Write latency (4 MiB, QD=1):")
                lines.append(f"  Without fsync: {wc['write_no_fsync_us']:>8,.0f} us")
                lines.append(f"  With fsync:    {wc['write_with_fsync_us']:>8,.0f} us")
                if wc["cache_ratio"]:
                    lines.append(f"  Ratio:         {wc['cache_ratio']:.1f}x (fsync/no-fsync)")
            lines.append("")

        # Read Latency Profile
        rp = self.results.get("read_profile")
        if rp:
            lines.append(sub)
            lines.append("Read Latency Profile (steady-state, 128 KiB random)")
            lines.append(sub)
            lines.append(f"{'Queue Depth':<12} {'Avg':>10} {'p50':>10} {'p99':>10} {'Max':>10}")
            for m in rp:
                lines.append(
                    f"{m['queue_depth']:<12} {m['avg']:>9,.0f} us {m['p50']:>9,.0f} us "
                    f"{m['p99']:>9,.0f} us {m['max']:>9,.0f} us"
                )
            lines.append("")

        # Summary
        lines.append(sep)
        lines.append("Summary")
        lines.append(sep)
        if gc:
            lines.append(f"Recommended --gc-settle:     {gc['recommended_gc_settle_s']}s")
        if ps and ps["power_state_transitions_at_s"]:
            lines.append(f"APST action:                 Disable for benchmarks")
        elif ps:
            lines.append(f"APST action:                 None needed")
        if wc:
            lines.append(f"Write cache:                 {'Enabled' if wc['vwc_enabled'] else 'Disabled'}"
                         + (f" (writes ACK from DRAM, {wc['cache_ratio']:.0f}x faster)" if wc.get("cache_ratio") and wc["cache_ratio"] > 2 else ""))
        if rp:
            # Find optimal QD (lowest p99 that's within 20% of QD=1 avg)
            qd1_avg = next((m["avg"] for m in rp if m["queue_depth"] == 1), 0)
            optimal = [m for m in rp if m["avg"] <= qd1_avg * 1.5 and m["queue_depth"] > 1]
            if optimal:
                lines.append(f"Optimal read QD:             {optimal[-1]['queue_depth']} (latency/throughput sweet spot)")
            else:
                lines.append(f"Optimal read QD:             1")
        lines.append("")

        return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(
        description="NVMe device inspection tool for Certus benchmark tuning. "
        "Characterizes GC settle time, power state behavior, write cache, "
        "and read latency profile.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="Examples:\n"
        "  sudo python3 inspect-nvme.py /dev/nvme4n1\n"
        "  sudo python3 inspect-nvme.py /dev/nvme4n1 --skip-gc --skip-power\n"
        "  sudo python3 inspect-nvme.py /dev/nvme4n1 --json --output report.json\n",
    )
    parser.add_argument("device", help="NVMe device path (e.g., /dev/nvme4n1)")
    parser.add_argument("--gc-write-gb", type=float, default=2.0,
                        help="GiB to write for GC settle test (default: 2)")
    parser.add_argument("--skip-gc", action="store_true",
                        help="Skip GC settle time test")
    parser.add_argument("--skip-power", action="store_true",
                        help="Skip power state behavior test")
    parser.add_argument("--skip-cache", action="store_true",
                        help="Skip write cache test")
    parser.add_argument("--skip-profile", action="store_true",
                        help="Skip read latency profile")
    parser.add_argument("--json", action="store_true",
                        help="Output results as JSON")
    parser.add_argument("--output", "-o", type=str, default=None,
                        help="Save report to file")
    args = parser.parse_args()

    if os.geteuid() != 0:
        print("Error: this tool requires root privileges (sudo).", file=sys.stderr)
        sys.exit(1)

    for tool in ["nvme", "fio"]:
        if not subprocess.run(["which", tool], capture_output=True).returncode == 0:
            print(f"Error: '{tool}' not found in PATH.", file=sys.stderr)
            sys.exit(1)

    if not os.path.exists(args.device):
        print(f"Error: device '{args.device}' does not exist.", file=sys.stderr)
        sys.exit(1)

    inspector = NvmeInspector(args.device)

    print("=" * 70)
    print("NVMe Device Inspection")
    print("=" * 70)
    print()

    print("[1/5] Identifying device...")
    inspector.identify()
    info = inspector.results["identify"]
    print(f"  Model: {info.get('model', 'unknown')}")
    print(f"  FW:    {info.get('firmware_rev', 'unknown')}")
    print(f"  NUMA:  {info.get('numa_node', 'unknown')}")
    print()

    if not args.skip_gc:
        print("[2/5] Measuring GC settle time...")
        inspector.measure_gc_settle(write_gb=args.gc_write_gb)
        print()
    else:
        print("[2/5] GC settle test: skipped")
        print()

    if not args.skip_power:
        print("[3/5] Measuring power state behavior...")
        inspector.measure_power_states()
        print()
    else:
        print("[3/5] Power state test: skipped")
        print()

    if not args.skip_cache:
        print("[4/5] Checking write cache behavior...")
        inspector.check_write_cache()
        print()
    else:
        print("[4/5] Write cache test: skipped")
        print()

    if not args.skip_profile:
        print("[5/5] Read latency profile...")
        inspector.read_latency_profile()
        print()
    else:
        print("[5/5] Read latency profile: skipped")
        print()

    # Generate output
    if args.json:
        output = json.dumps(inspector.results, indent=2, default=str)
    else:
        output = inspector.generate_report()

    print(output)

    if args.output:
        with open(args.output, "w") as f:
            f.write(output)
        print(f"\nReport saved to: {args.output}")


if __name__ == "__main__":
    main()
