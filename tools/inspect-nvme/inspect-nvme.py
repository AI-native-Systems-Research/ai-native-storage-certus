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
        """Measure how long after sustained writes until read latency stabilizes.

        This models the Certus benchmark scenario: populate writes fill the SSD,
        then cold reads must wait for internal GC/write-folding to complete before
        achieving steady-state read latency.
        """
        if intervals is None:
            intervals = [0, 5, 10, 15, 20, 30, 45, 60, 90, 120]

        # Phase 1: Establish baseline read latency on a quiet drive.
        print("  Measuring baseline read latency (quiet drive)...", end="", flush=True)
        run_fio(self.ns, rw="randread", bs="128k", runtime="3", qdepth=1)
        result_baseline = run_fio(
            self.ns, rw="randread", bs="128k", runtime="5", qdepth=1,
        )
        lat_baseline = extract_lat_us(result_baseline, "read")
        baseline_us = lat_baseline["avg"] if lat_baseline else 500
        print(f" {baseline_us:.0f} us")

        # Phase 2: Sustained write to trigger GC/write-folding.
        print(f"  Writing {write_gb} GiB to trigger GC pressure...", end="", flush=True)
        run_fio(self.ns, rw="write", bs="4M", size=f"{write_gb}G", qdepth=32)
        print(" done.")

        # Phase 3: Measure read latency at increasing intervals after write completes.
        # The drive is left truly idle between samples — no reads during the wait.
        # Each sample is a short 1s burst to minimize interference with GC.
        measurements = []
        t_write_done = time.time()

        for wait_target_s in intervals:
            # Sleep truly idle until the target time.
            elapsed = time.time() - t_write_done
            remaining = wait_target_s - elapsed
            if remaining > 0:
                print(f"  Idle wait until {wait_target_s}s...", end="", flush=True)
                time.sleep(remaining)
            else:
                print(f"  Sampling at {wait_target_s}s...", end="", flush=True)

            # Brief measurement burst (1s) to minimize GC interference from reads.
            actual_idle = time.time() - t_write_done
            result = run_fio(
                self.ns, rw="randread", bs="128k", runtime="1",
                qdepth=1,
            )
            lat = extract_lat_us(result, "read")
            avg = lat["avg"] if lat else 0
            measurements.append({
                "target_s": wait_target_s,
                "idle_s": round(actual_idle, 1),
                "avg_us": avg,
                "p99_us": lat["p99"] if lat else 0,
            })
            ratio = avg / baseline_us if baseline_us > 0 else 0
            marker = "" if ratio < 1.2 else f" ({ratio:.1f}x baseline)"
            print(f" {avg:.0f} us{marker}")

        # Find settle point: first measurement within 20% of baseline where
        # all subsequent measurements are also within 20% of baseline.
        threshold = baseline_us * 1.2
        recommended = intervals[-1]  # default to longest interval
        for i, m in enumerate(measurements):
            if m["avg_us"] <= 0:
                continue
            if m["avg_us"] <= threshold:
                rest = [mm["avg_us"] for mm in measurements[i:] if mm["avg_us"] > 0]
                if all(v <= threshold for v in rest):
                    recommended = m["target_s"]
                    break

        result = {
            "baseline_us": baseline_us,
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

    def reset_controller(self):
        """Issue a controller reset to invalidate all internal DRAM caches."""
        sysfs_reset = f"/sys/class/nvme/{os.path.basename(self.ctrl)}/reset_controller"
        if os.path.exists(sysfs_reset):
            try:
                with open(sysfs_reset, "w") as f:
                    f.write("1")
                time.sleep(2)
                return True
            except OSError:
                pass
        # Fallback to nvme-cli
        result = run_cmd(["nvme", "reset", self.ctrl], check=False)
        if result is not None:
            time.sleep(2)
            return True
        return False

    def check_read_cache(self):
        """Detect DRAM read cache by comparing repeated reads of a small region vs full-device random."""
        # Reset controller to invalidate any stale DRAM cache state.
        print("  Resetting controller to clear DRAM caches...", end="", flush=True)
        if self.reset_controller():
            print(" done.")
        else:
            print(" failed (continuing without reset).")

        # Phase 1: Read a small region (16 MiB) repeatedly to warm the drive's DRAM cache.
        # Then measure latency of reads within that cached region.
        cache_region = "16M"
        print(f"  Warming DRAM cache ({cache_region} region, sequential reads)...", end="", flush=True)
        run_fio(self.ns, rw="read", bs="128k", size=cache_region, qdepth=32)
        run_fio(self.ns, rw="read", bs="128k", size=cache_region, qdepth=32)
        print(" done.")

        print("  Measuring cached-region read latency (128 KiB, QD=1)...", end="", flush=True)
        result_cached = run_fio(
            self.ns, rw="randread", bs="128k", size=cache_region,
            runtime="3", qdepth=1,
        )
        lat_cached = extract_lat_us(result_cached, "read")
        print(f" {lat_cached['avg']:.0f} us" if lat_cached else " failed")

        # Phase 2: Random reads across the full device (far exceeds any DRAM cache).
        print("  Measuring full-device read latency (128 KiB, QD=1)...", end="", flush=True)
        result_full = run_fio(
            self.ns, rw="randread", bs="128k",
            runtime="3", qdepth=1,
        )
        lat_full = extract_lat_us(result_full, "read")
        print(f" {lat_full['avg']:.0f} us" if lat_full else " failed")

        # Phase 3: Read a large region sequentially then re-read to detect cache eviction.
        # If the 16 MiB region is now slower, it was evicted from DRAM cache.
        print("  Evicting cache (256 MiB sequential read)...", end="", flush=True)
        run_fio(self.ns, rw="read", bs="128k", size="256M", qdepth=32)
        print(" done.")

        print("  Re-measuring original region (post-eviction)...", end="", flush=True)
        result_evicted = run_fio(
            self.ns, rw="randread", bs="128k", size=cache_region,
            runtime="3", qdepth=1,
        )
        lat_evicted = extract_lat_us(result_evicted, "read")
        print(f" {lat_evicted['avg']:.0f} us" if lat_evicted else " failed")

        cached_avg = lat_cached["avg"] if lat_cached else 0
        full_avg = lat_full["avg"] if lat_full else 0
        evicted_avg = lat_evicted["avg"] if lat_evicted else 0

        # A drive with effective DRAM read cache shows: cached << full ≈ evicted
        has_read_cache = (
            cached_avg > 0 and full_avg > 0
            and full_avg > cached_avg * 1.5
        )
        cache_speedup = full_avg / cached_avg if cached_avg > 0 else 0

        result = {
            "cached_region_us": cached_avg,
            "full_device_us": full_avg,
            "post_eviction_us": evicted_avg,
            "cache_speedup": cache_speedup,
            "has_read_cache": has_read_cache,
            "estimated_cache_size": "< 16 MiB" if not has_read_cache else ">= 16 MiB",
        }
        self.results["read_cache"] = result
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
            baseline = gc.get("baseline_us", 0)
            lines.append(f"Baseline read latency (quiet drive): {baseline:,.0f} us")
            threshold = baseline * 1.2
            lines.append(f"Settle threshold (120% of baseline): {threshold:,.0f} us")
            lines.append("")
            lines.append("Post-write read latency (128 KiB random, QD=1):")
            for m in gc["measurements"]:
                ratio = m["avg_us"] / baseline if baseline > 0 else 0
                marker = ""
                if m["avg_us"] > 0 and m["avg_us"] <= threshold:
                    marker = "  <-- settled"
                elif ratio > 1:
                    marker = f"  ({ratio:.1f}x baseline)"
                lines.append(f"  {m['target_s']:>2}s after write:  {m['avg_us']:>8,.0f} us{marker}")
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

        # Read Cache
        rc = self.results.get("read_cache")
        if rc:
            lines.append(sub)
            lines.append("DRAM Read Cache")
            lines.append(sub)
            lines.append(f"Read latency (128 KiB random, QD=1):")
            lines.append(f"  Cached region (16 MiB): {rc['cached_region_us']:>8,.0f} us")
            lines.append(f"  Full device:            {rc['full_device_us']:>8,.0f} us")
            lines.append(f"  Post-eviction:          {rc['post_eviction_us']:>8,.0f} us")
            if rc["has_read_cache"]:
                lines.append(f"\n  DRAM read cache detected: {rc['cache_speedup']:.1f}x speedup for cached data")
                lines.append(f"  Estimated cache coverage: {rc['estimated_cache_size']}")
            else:
                lines.append(f"\n  No significant DRAM read cache effect detected")
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
        if rc:
            if rc["has_read_cache"]:
                lines.append(f"Read cache:                  Active ({rc['cache_speedup']:.1f}x for hot data)")
            else:
                lines.append(f"Read cache:                  Not detected")
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
    parser.add_argument("--gc-write-gb", type=float, default=128.0,
                        help="GiB to write for GC settle test (default: 128)")
    parser.add_argument("--skip-gc", action="store_true",
                        help="Skip GC settle time test")
    parser.add_argument("--skip-power", action="store_true",
                        help="Skip power state behavior test")
    parser.add_argument("--skip-cache", action="store_true",
                        help="Skip write cache test")
    parser.add_argument("--skip-read-cache", action="store_true",
                        help="Skip DRAM read cache detection test")
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

    print("[1/6] Identifying device...")
    inspector.identify()
    info = inspector.results["identify"]
    print(f"  Model: {info.get('model', 'unknown')}")
    print(f"  FW:    {info.get('firmware_rev', 'unknown')}")
    print(f"  NUMA:  {info.get('numa_node', 'unknown')}")
    print()

    if not args.skip_gc:
        print("[2/6] Measuring GC settle time...")
        inspector.measure_gc_settle(write_gb=args.gc_write_gb)
        print()
    else:
        print("[2/6] GC settle test: skipped")
        print()

    if not args.skip_power:
        print("[3/6] Measuring power state behavior...")
        inspector.measure_power_states()
        print()
    else:
        print("[3/6] Power state test: skipped")
        print()

    if not args.skip_cache:
        print("[4/6] Checking write cache behavior...")
        inspector.check_write_cache()
        print()
    else:
        print("[4/6] Write cache test: skipped")
        print()

    if not args.skip_read_cache:
        print("[5/6] Detecting DRAM read cache...")
        inspector.check_read_cache()
        print()
    else:
        print("[5/6] DRAM read cache test: skipped")
        print()

    if not args.skip_profile:
        print("[6/6] Read latency profile...")
        inspector.read_latency_profile()
        print()
    else:
        print("[6/6] Read latency profile: skipped")
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
