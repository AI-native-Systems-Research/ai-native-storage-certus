#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Synthetic micro-benchmark for CertusGrpcWorker queue/pool latency.

Exercises the connector's async dispatch and reaping under mixed store/load
workloads WITHOUT needing vLLM, a GPU, or a running certus-server. Uses a
LatencyFakeStub that injects configurable sleep per RPC to simulate realistic
gRPC round-trip times.

Detects:
  - Head-of-line (HOL) blocking: loads stuck behind slow stores in a shared deque
  - Thread pool starvation: loads waiting for a free thread in a shared pool

Usage:
    python benchmarks/bench_connector_latency.py
    python benchmarks/bench_connector_latency.py --ci
    python benchmarks/bench_connector_latency.py --store-latency-ms 20 --iterations 50
"""

from __future__ import annotations

import argparse
import os
import random
import sys
import time
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass

# ── Bootstrap: install fake vLLM modules if real vLLM is unavailable ──

_here = os.path.dirname(os.path.abspath(__file__))
_pkg = os.path.dirname(_here)
if _pkg not in sys.path:
    sys.path.insert(0, _pkg)

if "vllm" not in sys.modules:
    _tests = os.path.join(_pkg, "tests")
    if _tests not in sys.path:
        sys.path.insert(0, _tests)
    from conftest import build_fake_vllm
    build_fake_vllm((0, 26))

from certus_grpc_connector import dispatcher_pb2 as pb
from certus_grpc_connector.gpu import KvCacheIpc
from certus_grpc_connector.handler import worker_class
from certus_grpc_connector.mediums import BlockLocation, CertusLoadStoreSpec

from vllm.v1.kv_offload.base import GPULoadStoreSpec


# ── LatencyFakeStub ──


class LatencyFakeStub:
    """Fake gRPC stub that simulates RPC latency with configurable sleep."""

    def __init__(self, store_latency_s: float = 0.010, load_latency_s: float = 0.005):
        self.store_latency_s = store_latency_s
        self.load_latency_s = load_latency_s

    def CopyToStore(self, req):
        time.sleep(self.store_latency_s)
        return pb.BatchCopyToStoreResponse(
            results=[pb.EntryResult(key=e.key, success=True) for e in req.entries]
        )

    def Lookup(self, req):
        time.sleep(self.load_latency_s)
        return pb.BatchLookupResponse(
            results=[pb.EntryResult(key=e.key, success=True) for e in req.entries]
        )

    def AbortStore(self, req):
        return pb.BatchAbortStoreResponse(
            results=[pb.EntryResult(key=k, success=True) for k in req.keys]
        )


# ── Helpers ──


def make_worker(stub, store_workers: int = 4, load_workers: int = 4):
    kv = KvCacheIpc(
        handle_bytes=b"\x00" * 64, gpu_device_id=0,
        stride_bytes=131072, base_delta=0,
    )
    store_exec = ThreadPoolExecutor(max_workers=store_workers, thread_name_prefix="bench-store")
    load_exec = ThreadPoolExecutor(max_workers=load_workers, thread_name_prefix="bench-load")
    return worker_class()(stub, [kv], 131072, store_exec, load_exec)


def make_store_spec(job_id, n_blocks=4):
    block_ids = list(range(job_id * n_blocks, (job_id + 1) * n_blocks))
    keys = [k + 1_000_000 for k in block_ids]
    src = GPULoadStoreSpec(block_ids=block_ids, group_sizes=[n_blocks], block_indices=[0])
    dst = CertusLoadStoreSpec([BlockLocation(key=k) for k in keys])
    return src, dst


def make_load_spec(job_id, n_blocks=4):
    block_ids = list(range(job_id * n_blocks, (job_id + 1) * n_blocks))
    keys = [k + 2_000_000 for k in block_ids]
    src = CertusLoadStoreSpec([BlockLocation(key=k) for k in keys])
    dst = GPULoadStoreSpec(block_ids=block_ids, group_sizes=[n_blocks], block_indices=[0])
    return src, dst


def percentiles(latencies_us: list[float]) -> dict:
    if not latencies_us:
        return {}
    s = sorted(latencies_us)
    n = len(s)
    return {
        "n": n,
        "avg": sum(s) / n,
        "p50": s[int(n * 0.50)],
        "p95": s[int(n * 0.95)],
        "p99": s[min(int(n * 0.99), n - 1)],
        "max": s[-1],
    }


def print_stats(label: str, latencies_us: list[float]):
    p = percentiles(latencies_us)
    if not p:
        print(f"  {label:<35} no data")
        return
    print(
        f"  {label:<35} n={p['n']:>4}  "
        f"avg={p['avg']:>8.0f}us  p50={p['p50']:>8.0f}us  "
        f"p95={p['p95']:>8.0f}us  p99={p['p99']:>8.0f}us  "
        f"max={p['max']:>8.0f}us"
    )


# ── Scenarios ──


def scenario_hol_blocking(stub, n_stores: int = 20, iterations: int = 20,
                          store_workers: int = 4, load_workers: int = 4) -> list[float]:
    """Measure load reporting latency when stores are in-flight.

    Submit N stores then 1 load. Poll get_finished() until the load appears.
    With separate deques the load reports as soon as its RPC finishes (~load_latency).
    With a shared deque it waits behind all stores (~n_stores * store_latency / workers).
    """
    latencies = []
    for trial in range(iterations):
        worker = make_worker(stub, store_workers, load_workers)
        for i in range(n_stores):
            src, dst = make_store_spec(trial * 100 + i)
            worker.submit_store(job_id=i, src_spec=src, dst_spec=dst)

        load_src, load_dst = make_load_spec(trial * 100 + n_stores)
        t_submit = time.perf_counter()
        worker.submit_load(job_id=n_stores, src_spec=load_src, dst_spec=load_dst)

        while True:
            finished = worker.get_finished()
            for r in finished:
                if r.job_id == n_stores:
                    t_done = time.perf_counter()
                    latencies.append((t_done - t_submit) * 1e6)
                    break
            else:
                time.sleep(0.0005)
                continue
            break

        worker.wait(set(range(n_stores + 1)))
    return latencies


def scenario_pool_saturation(stub, n_stores: int = 16, iterations: int = 20,
                             store_workers: int = 4, load_workers: int = 4) -> list[float]:
    """Measure load dispatch latency when the store pool is saturated.

    Submit enough stores to fill the store pool 4x over, then submit a load.
    With separate pools the load dispatches immediately (~load_latency).
    With a shared pool it queues behind stores (~3-4x store_latency).
    """
    latencies = []
    for trial in range(iterations):
        worker = make_worker(stub, store_workers, load_workers)
        for i in range(n_stores):
            src, dst = make_store_spec(trial * 100 + i)
            worker.submit_store(job_id=i, src_spec=src, dst_spec=dst)

        time.sleep(0.002)

        load_src, load_dst = make_load_spec(trial * 100 + n_stores)
        t_submit = time.perf_counter()
        worker.submit_load(job_id=n_stores, src_spec=load_src, dst_spec=load_dst)
        worker.wait({n_stores})
        t_done = time.perf_counter()
        latencies.append((t_done - t_submit) * 1e6)

        worker.wait(set(range(n_stores)))
    return latencies


def scenario_mixed(stub, n_ops: int = 100, store_ratio: float = 0.5,
                   iterations: int = 5, store_workers: int = 4,
                   load_workers: int = 4) -> tuple[list[float], list[float]]:
    """Interleaved stores+loads; measure per-direction latency."""
    store_lats = []
    load_lats = []
    for trial in range(iterations):
        worker = make_worker(stub, store_workers, load_workers)
        pending: dict[int, tuple[str, float]] = {}
        job_id = 0
        for _ in range(n_ops):
            if random.random() < store_ratio:
                src, dst = make_store_spec(trial * 1000 + job_id)
                t = time.perf_counter()
                worker.submit_store(job_id=job_id, src_spec=src, dst_spec=dst)
                pending[job_id] = ("store", t)
            else:
                src, dst = make_load_spec(trial * 1000 + job_id)
                t = time.perf_counter()
                worker.submit_load(job_id=job_id, src_spec=src, dst_spec=dst)
                pending[job_id] = ("load", t)
            job_id += 1

        worker.wait(set(pending.keys()))
        t_now = time.perf_counter()
        for jid, (direction, t_submit) in pending.items():
            lat = (t_now - t_submit) * 1e6
            if direction == "store":
                store_lats.append(lat)
            else:
                load_lats.append(lat)
    return store_lats, load_lats


def scenario_baseline(stub, direction: str = "store", n_ops: int = 100,
                      iterations: int = 5, store_workers: int = 4,
                      load_workers: int = 4) -> tuple[int, float]:
    """Single-direction throughput ceiling."""
    t_start = time.perf_counter()
    for trial in range(iterations):
        worker = make_worker(stub, store_workers, load_workers)
        ids = set()
        for i in range(n_ops):
            job_id = trial * n_ops + i
            ids.add(job_id)
            if direction == "store":
                src, dst = make_store_spec(job_id)
                worker.submit_store(job_id=job_id, src_spec=src, dst_spec=dst)
            else:
                src, dst = make_load_spec(job_id)
                worker.submit_load(job_id=job_id, src_spec=src, dst_spec=dst)
        worker.wait(ids)
    t_end = time.perf_counter()
    total_ops = n_ops * iterations
    return total_ops, t_end - t_start


# ── Main ──


def main():
    parser = argparse.ArgumentParser(description="Connector latency micro-benchmark")
    parser.add_argument("--store-latency-ms", type=float, default=10.0,
                        help="Simulated CopyToStore RPC latency (ms)")
    parser.add_argument("--load-latency-ms", type=float, default=5.0,
                        help="Simulated Lookup RPC latency (ms)")
    parser.add_argument("--store-workers", type=int, default=4)
    parser.add_argument("--load-workers", type=int, default=4)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--n-ops", type=int, default=100,
                        help="Operations per trial in mixed/baseline scenarios")
    parser.add_argument("--ci", action="store_true",
                        help="Exit with code 1 if any scenario fails threshold")
    args = parser.parse_args()

    stub = LatencyFakeStub(
        store_latency_s=args.store_latency_ms / 1000.0,
        load_latency_s=args.load_latency_ms / 1000.0,
    )
    max_acceptable_us = (args.load_latency_ms / 1000.0 * 2 + 0.002) * 1e6

    print("=" * 78)
    print("Certus gRPC Connector Latency Micro-Benchmark")
    print("=" * 78)
    print(f"  Store RPC latency:  {args.store_latency_ms:.1f} ms (simulated)")
    print(f"  Load RPC latency:   {args.load_latency_ms:.1f} ms (simulated)")
    print(f"  Store pool:         {args.store_workers} threads")
    print(f"  Load pool:          {args.load_workers} threads")
    print(f"  Pass threshold:     load p95 < {max_acceptable_us:.0f} us")
    print()

    failures = []

    # Scenario A: HOL blocking
    print("Scenario A: HOL Blocking (20 stores + 1 load)")
    hol_lats = scenario_hol_blocking(
        stub, n_stores=20, iterations=args.iterations,
        store_workers=args.store_workers, load_workers=args.load_workers,
    )
    print_stats("load_reporting_latency", hol_lats)
    p95 = percentiles(hol_lats).get("p95", 0)
    if p95 < max_acceptable_us:
        print(f"  PASS (p95={p95:.0f}us < {max_acceptable_us:.0f}us)")
    else:
        print(f"  FAIL (p95={p95:.0f}us >= {max_acceptable_us:.0f}us)")
        failures.append("HOL blocking")
    print()

    # Scenario B: Pool saturation
    print("Scenario B: Pool Saturation (16 stores saturating pool + 1 load)")
    sat_lats = scenario_pool_saturation(
        stub, n_stores=16, iterations=args.iterations,
        store_workers=args.store_workers, load_workers=args.load_workers,
    )
    print_stats("load_dispatch_latency", sat_lats)
    p95 = percentiles(sat_lats).get("p95", 0)
    if p95 < max_acceptable_us:
        print(f"  PASS (p95={p95:.0f}us < {max_acceptable_us:.0f}us)")
    else:
        print(f"  FAIL (p95={p95:.0f}us >= {max_acceptable_us:.0f}us)")
        failures.append("Pool saturation")
    print()

    # Scenario C: Mixed workload
    print("Scenario C: Mixed Workload")
    for ratio, label in [(0.8, "4:1"), (0.5, "1:1"), (0.2, "1:4")]:
        s_lats, l_lats = scenario_mixed(
            stub, n_ops=args.n_ops, store_ratio=ratio, iterations=5,
            store_workers=args.store_workers, load_workers=args.load_workers,
        )
        print_stats(f"  {label} stores", s_lats)
        print_stats(f"  {label} loads", l_lats)
    print()

    # Scenario D: Baseline throughput
    print("Scenario D: Baseline Throughput")
    for direction in ("store", "load"):
        ops, elapsed = scenario_baseline(
            stub, direction=direction, n_ops=args.n_ops, iterations=5,
            store_workers=args.store_workers, load_workers=args.load_workers,
        )
        print(f"  pure_{direction}s: {ops} ops in {elapsed:.3f}s = {ops/elapsed:.0f} ops/s")
    print()

    print("=" * 78)
    if failures:
        print(f"FAILED: {', '.join(failures)}")
        if args.ci:
            sys.exit(1)
    else:
        print("ALL SCENARIOS PASSED")


if __name__ == "__main__":
    main()
