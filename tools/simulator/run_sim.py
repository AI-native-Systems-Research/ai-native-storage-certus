#!/usr/bin/env python3
"""Certus Server Discrete Event Simulator.

Simulates the certus-server two-tier GPU cache (DRAM memory-tier + NVMe SSDs)
with realistic pipeline stage timing based on the system specifications.

Examples:
    # Synthetic workload: populate 1000 entries, lookup 5000, 4 drives, 64 MiB memory tier
    python run_sim.py --synthetic-populate 1000 --synthetic-lookup 5000 \\
        --num-drives 4 --memory-tier-size 64M --entry-size 128K

    # Trace replay
    python run_sim.py --trace workload.jsonl --num-drives 4

    # Explore cache dynamics with small memory tier
    python run_sim.py --synthetic-populate 500 --synthetic-lookup 2000 \\
        --memory-tier-size 8M --entry-size 128K --num-drives 2
"""

from __future__ import annotations

import argparse
import sys
import time

import simpy

from certus_sim.config import SimConfig
from certus_sim.dispatcher import Dispatcher
from certus_sim.grpc_server import GrpcServer
from certus_sim.metrics import Metrics
from certus_sim.workload import WorkloadDriver, generate_synthetic, load_trace


def parse_size(s: str) -> int:
    """Parse human-readable size (e.g., '128K', '2G', '512M')."""
    s = s.strip().upper()
    multipliers = {"K": 1024, "M": 1024**2, "G": 1024**3, "T": 1024**4}
    if s[-1] in multipliers:
        return int(float(s[:-1]) * multipliers[s[-1]])
    return int(s)


def build_config(args: argparse.Namespace) -> SimConfig:
    config = SimConfig(
        num_drives=args.num_drives,
        drive_capacity_bytes=parse_size(args.drive_capacity),
        memory_tier_capacity_bytes=parse_size(args.memory_tier_size),
        entry_size_bytes=parse_size(args.entry_size),
        max_eviction_attempts=args.max_eviction_attempts,
        ssd_eviction_threshold=args.ssd_eviction_threshold,
        ssd_eviction_low_watermark=args.ssd_eviction_low_watermark,
        gpu_d2h_latency_us=args.gpu_d2h_us,
        gpu_h2d_latency_us=args.gpu_h2d_us,
        nvme_read_latency_us=args.nvme_read_us,
        nvme_write_latency_us=args.nvme_write_us,
        max_queue_depth=args.max_queue_depth,
        max_queues_per_drive=args.max_queues_per_drive,
        write_through_enabled=not args.no_write_through,
    )
    return config


def main():
    parser = argparse.ArgumentParser(
        description="Certus Server Discrete Event Simulator",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )

    # Workload source
    wl = parser.add_argument_group("Workload")
    wl.add_argument("--trace", type=str, help="JSONL trace file to replay")
    wl.add_argument("--synthetic-populate", type=int, default=0,
                    help="Number of entries to populate in synthetic mode")
    wl.add_argument("--synthetic-lookup", type=int, default=0,
                    help="Number of lookups in synthetic mode")
    wl.add_argument("--key-space", type=int, default=0,
                    help="Key space for lookups (0 = same as populate count)")
    wl.add_argument("--batch-size", type=int, default=100,
                    help="Entries per batch (default: 100)")
    wl.add_argument("--inter-batch-us", type=float, default=1000.0,
                    help="Microseconds between batches (default: 1000)")

    # System configuration
    sys_grp = parser.add_argument_group("System Configuration")
    sys_grp.add_argument("--num-drives", type=int, default=4,
                         help="Number of NVMe drives (default: 4)")
    sys_grp.add_argument("--drive-capacity", type=str, default="1T",
                         help="Per-drive capacity (default: 1T)")
    sys_grp.add_argument("--memory-tier-size", type=str, default="2G",
                         help="DRAM memory-tier pool size (default: 2G)")
    sys_grp.add_argument("--entry-size", type=str, default="128K",
                         help="Cache entry size (default: 128K)")

    # Eviction tuning
    ev = parser.add_argument_group("Eviction")
    ev.add_argument("--max-eviction-attempts", type=int, default=2048,
                    help="Max eviction loop iterations (default: 2048)")
    ev.add_argument("--ssd-eviction-threshold", type=float, default=0.9,
                    help="SSD utilization threshold to trigger eviction (default: 0.9)")
    ev.add_argument("--ssd-eviction-low-watermark", type=float, default=0.8,
                    help="SSD utilization target after eviction (default: 0.8)")
    ev.add_argument("--no-write-through", action="store_true",
                    help="Disable background write-through to SSD")

    # Timing parameters
    tm = parser.add_argument_group("Timing (microseconds)")
    tm.add_argument("--gpu-d2h-us", type=float, default=50.0,
                    help="GPU device-to-host DMA latency (default: 50)")
    tm.add_argument("--gpu-h2d-us", type=float, default=40.0,
                    help="GPU host-to-device DMA latency (default: 40)")
    tm.add_argument("--nvme-read-us", type=float, default=80.0,
                    help="NVMe read latency per MDTS segment (default: 80)")
    tm.add_argument("--nvme-write-us", type=float, default=20.0,
                    help="NVMe write latency per MDTS segment (default: 20)")
    tm.add_argument("--max-queue-depth", type=int, default=16,
                    help="NVMe pipeline queue depth (default: 16)")
    tm.add_argument("--max-queues-per-drive", type=int, default=2,
                    help="Parallel queue threads per drive (default: 2)")

    args = parser.parse_args()

    # Validate inputs
    if not args.trace and args.synthetic_populate == 0:
        parser.error("Must specify --trace or --synthetic-populate")

    config = build_config(args)

    # Print configuration
    print("=" * 60)
    print("  CERTUS SERVER SIMULATOR")
    print("=" * 60)
    print(f"  Drives:          {config.num_drives} x {args.drive_capacity}")
    print(f"  Memory tier:     {args.memory_tier_size}")
    print(f"  Entry size:      {args.entry_size}")
    print(f"  Queue depth:     {config.max_queue_depth}")
    print(f"  Write-through:   {'enabled' if config.write_through_enabled else 'disabled'}")
    print(f"  NVMe read:       {config.nvme_read_latency_us} µs/segment")
    print(f"  GPU H2D:         {config.gpu_h2d_latency_us} µs")
    print(f"  GPU D2H:         {config.gpu_d2h_latency_us} µs")
    print("=" * 60)
    print()

    # Build workload
    if args.trace:
        print(f"Loading trace: {args.trace}")
        ops = load_trace(args.trace)
        print(f"  {len(ops)} batch operations loaded")
    else:
        print(f"Generating synthetic workload:")
        print(f"  Populate: {args.synthetic_populate} entries")
        print(f"  Lookup:   {args.synthetic_lookup} entries")
        print(f"  Key space:{args.key_space or args.synthetic_populate}")
        print(f"  Batch:    {args.batch_size}")
        ops = generate_synthetic(
            num_populate=args.synthetic_populate,
            num_lookup=args.synthetic_lookup,
            entry_size=config.entry_size_bytes,
            key_space=args.key_space,
            batch_size=args.batch_size,
            inter_batch_us=args.inter_batch_us,
        )
        print(f"  {len(ops)} batch operations generated")
    print()

    # Run simulation
    print("Running simulation...")
    wall_start = time.perf_counter()

    env = simpy.Environment()
    metrics = Metrics()
    dispatcher = Dispatcher(env, config, metrics)
    server = GrpcServer(env, config, dispatcher, metrics)
    driver = WorkloadDriver(env, server, config)

    env.process(driver._run(ops))

    # Run until workload completes + drain period for write-through
    max_workload_time = max(op.time_us for op in ops) if ops else 0.0
    # Generous drain: workload time + time for all ops + write-through drain
    drain_budget = len(ops) * len(ops[0].keys if ops else []) * 500.0  # ~500us per entry
    sim_until = max_workload_time + drain_budget + 100_000.0  # +100ms drain
    env.run(until=sim_until)

    dispatcher.shutdown()
    wall_elapsed = time.perf_counter() - wall_start
    sim_time_us = env.now

    # Report
    print(f"Simulation complete: {sim_time_us:.0f} µs simulated in {wall_elapsed:.2f}s wall time")
    print(f"  Speedup: {sim_time_us / (wall_elapsed * 1e6):.1f}x" if wall_elapsed > 0 else "")
    print()

    # Final state
    print(f"── Final State ──")
    print(f"  Memory tier: {dispatcher.memory_tier.used_bytes / 1024**2:.1f} MiB / "
          f"{dispatcher.memory_tier.capacity_bytes / 1024**2:.1f} MiB "
          f"({dispatcher.memory_tier.entry_count()} entries)")
    print(f"  SSD total:   {dispatcher.ssd.total_used_bytes() / 1024**2:.1f} MiB / "
          f"{dispatcher.ssd.total_capacity_bytes() / 1024**3:.1f} GiB")
    for i, drive in enumerate(dispatcher.ssd.drives):
        print(f"    Drive {i}: {drive.utilization()*100:.2f}% ({drive.extent_count()} extents)")
    print(f"  Dispatch map: {dispatcher.dispatch_map.entry_count()} entries")
    print()

    print(metrics.summary())


if __name__ == "__main__":
    main()
