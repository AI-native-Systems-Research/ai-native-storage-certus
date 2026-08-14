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

    def __init__(self, store_latency_s: float = 0.010, load_latency_s: float = 0.005,
                 straggler_rate: float = 0.0, straggler_multiplier: float = 10.0):
        self.store_latency_s = store_latency_s
        self.load_latency_s = load_latency_s
        self.straggler_rate = straggler_rate
        self.straggler_multiplier = straggler_multiplier
        self._store_count = 0
        self._load_count = 0

    def _store_delay(self) -> float:
        self._store_count += 1
        if self.straggler_rate > 0 and random.random() < self.straggler_rate:
            return self.store_latency_s * self.straggler_multiplier
        return self.store_latency_s

    def _load_delay(self) -> float:
        self._load_count += 1
        if self.straggler_rate > 0 and random.random() < self.straggler_rate:
            return self.load_latency_s * self.straggler_multiplier
        return self.load_latency_s

    def CopyToStore(self, req):
        time.sleep(self._store_delay())
        return pb.BatchCopyToStoreResponse(
            results=[pb.EntryResult(key=e.key, success=True) for e in req.entries]
        )

    def Lookup(self, req):
        time.sleep(self._load_delay())
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


# ── Additional Scenarios ──


def scenario_straggler(stub, n_ops: int = 100, iterations: int = 5,
                       store_workers: int = 4, load_workers: int = 4) -> tuple[list[float], list[float]]:
    """Measure load p99 when 5% of store RPCs are 10x slower (stragglers).

    Real gRPC systems get rare slow calls from server GC, TCP hiccups, GPU
    contention. Tests whether one slow store within a direction ruins load
    tail latency via intra-direction HOL (FIFO deque blocks fast completions
    behind a straggler).
    """
    straggler_stub = LatencyFakeStub(
        store_latency_s=stub.store_latency_s,
        load_latency_s=stub.load_latency_s,
        straggler_rate=0.05,
        straggler_multiplier=10.0,
    )
    store_lats = []
    load_lats = []
    for trial in range(iterations):
        worker = make_worker(straggler_stub, store_workers, load_workers)
        pending: dict[int, tuple[str, float]] = {}
        job_id = 0
        for _ in range(n_ops):
            if random.random() < 0.6:
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


def scenario_burst_recovery(stub, burst_size: int = 60, iterations: int = 10,
                            store_workers: int = 4, load_workers: int = 4) -> list[float]:
    """Saturate with a burst of stores, idle briefly, then measure load latency.

    Tests whether backlog from a previous burst contaminates subsequent loads
    (e.g., unreaped futures, pool threads still occupied, deque not drained).
    """
    latencies = []
    for trial in range(iterations):
        worker = make_worker(stub, store_workers, load_workers)

        for i in range(burst_size):
            src, dst = make_store_spec(trial * 200 + i)
            worker.submit_store(job_id=i, src_spec=src, dst_spec=dst)

        time.sleep(0.005)
        worker.get_finished()

        load_src, load_dst = make_load_spec(trial * 200 + burst_size)
        t_submit = time.perf_counter()
        worker.submit_load(job_id=burst_size, src_spec=load_src, dst_spec=load_dst)
        worker.wait({burst_size})
        t_done = time.perf_counter()
        latencies.append((t_done - t_submit) * 1e6)

        worker.wait(set(range(burst_size + 1)))
    return latencies


def scenario_asymmetric_saturation(store_workers: int = 4, load_workers: int = 4,
                                   iterations: int = 10) -> tuple[list[float], list[float]]:
    """Slow stores (50ms) with fast loads (2ms) — the realistic production case.

    Server-side GPU->host DMA is slow; DRAM-tier lookups are fast. Tests whether
    abundant slow stores degrade fast load latency. Measures load completion time
    independently (via polling) rather than waiting for all stores to finish.
    """
    asymmetric_stub = LatencyFakeStub(store_latency_s=0.050, load_latency_s=0.002)
    store_lats = []
    load_lats = []
    for trial in range(iterations):
        worker = make_worker(asymmetric_stub, store_workers, load_workers)
        job_id = 0
        load_ids = set()
        store_ids = set()
        submit_times: dict[int, float] = {}

        for i in range(20):
            src, dst = make_store_spec(trial * 100 + job_id)
            submit_times[job_id] = time.perf_counter()
            worker.submit_store(job_id=job_id, src_spec=src, dst_spec=dst)
            store_ids.add(job_id)
            job_id += 1

        time.sleep(0.005)

        for i in range(10):
            src, dst = make_load_spec(trial * 100 + job_id)
            submit_times[job_id] = time.perf_counter()
            worker.submit_load(job_id=job_id, src_spec=src, dst_spec=dst)
            load_ids.add(job_id)
            job_id += 1

        # Poll until all loads are reported (don't wait for stores)
        reported_loads = set()
        reported_stores = set()
        while reported_loads != load_ids:
            time.sleep(0.0005)
            t_poll = time.perf_counter()
            for r in worker.get_finished():
                if r.job_id in load_ids and r.job_id not in reported_loads:
                    reported_loads.add(r.job_id)
                    load_lats.append((t_poll - submit_times[r.job_id]) * 1e6)
                elif r.job_id in store_ids and r.job_id not in reported_stores:
                    reported_stores.add(r.job_id)
                    store_lats.append((t_poll - submit_times[r.job_id]) * 1e6)

        # Drain remaining stores
        worker.wait(store_ids)
        t_final = time.perf_counter()
        for r in worker.get_finished():
            if r.job_id in store_ids and r.job_id not in reported_stores:
                reported_stores.add(r.job_id)
                store_lats.append((t_final - submit_times[r.job_id]) * 1e6)

    return store_lats, load_lats


def scenario_intra_direction_hol(stub, store_workers: int = 4, load_workers: int = 4,
                                 iterations: int = 10) -> list[float]:
    """One very slow store followed by many fast stores.

    Tests whether get_finished() can report fast completions that finished
    before a straggler at an earlier position. With unordered reaping, fast
    stores report at ~5ms. With FIFO reaping, they wait ~100ms.
    """
    fast_lats = []
    for trial in range(iterations):
        class FirstSlowStub:
            def __init__(self):
                self._call = 0
            def CopyToStore(self, req):
                self._call += 1
                if self._call == 1:
                    time.sleep(0.100)
                else:
                    time.sleep(0.005)
                return pb.BatchCopyToStoreResponse(
                    results=[pb.EntryResult(key=e.key, success=True) for e in req.entries]
                )
            def Lookup(self, req):
                time.sleep(0.002)
                return pb.BatchLookupResponse(
                    results=[pb.EntryResult(key=e.key, success=True) for e in req.entries]
                )
            def AbortStore(self, req):
                return pb.BatchAbortStoreResponse(
                    results=[pb.EntryResult(key=k, success=True) for k in req.keys]
                )

        fs = FirstSlowStub()
        worker = make_worker(fs, store_workers, load_workers)

        n_stores = 8
        t_submits = {}
        for i in range(n_stores):
            src, dst = make_store_spec(trial * 100 + i)
            t_submits[i] = time.perf_counter()
            worker.submit_store(job_id=i, src_spec=src, dst_spec=dst)

        # Poll get_finished() repeatedly to measure when fast stores report
        reported: dict[int, float] = {}
        while len(reported) < n_stores:
            time.sleep(0.001)
            t_poll = time.perf_counter()
            for r in worker.get_finished():
                if r.job_id not in reported:
                    reported[r.job_id] = t_poll

        for jid in range(1, n_stores):  # skip straggler (job 0)
            fast_lats.append((reported[jid] - t_submits[jid]) * 1e6)

    return fast_lats


def scenario_reaping_cadence(stub, poll_interval_ms: float = 1.0,
                             n_ops: int = 50, store_workers: int = 4,
                             load_workers: int = 4) -> tuple[list[float], list[float]]:
    """Measure completed-but-unobserved delay at different polling intervals.

    Work can be done but invisible to the scheduler until get_finished() is
    called. This measures the gap between actual RPC completion and when the
    connector reports it.
    """
    submit_times: dict[int, float] = {}
    report_times: dict[int, float] = {}
    directions: dict[int, str] = {}

    worker = make_worker(stub, store_workers, load_workers)
    job_id = 0

    for i in range(n_ops):
        if random.random() < 0.5:
            src, dst = make_store_spec(job_id)
            submit_times[job_id] = time.perf_counter()
            directions[job_id] = "store"
            worker.submit_store(job_id=job_id, src_spec=src, dst_spec=dst)
        else:
            src, dst = make_load_spec(job_id)
            submit_times[job_id] = time.perf_counter()
            directions[job_id] = "load"
            worker.submit_load(job_id=job_id, src_spec=src, dst_spec=dst)
        job_id += 1

    # Poll at the given interval until all are reaped
    reaped = set()
    while len(reaped) < n_ops:
        time.sleep(poll_interval_ms / 1000.0)
        t_poll = time.perf_counter()
        finished = worker.get_finished()
        for r in finished:
            if r.job_id not in reaped:
                report_times[r.job_id] = t_poll
                reaped.add(r.job_id)

    store_lats = []
    load_lats = []
    for jid in range(n_ops):
        lat = (report_times[jid] - submit_times[jid]) * 1e6
        if directions[jid] == "store":
            store_lats.append(lat)
        else:
            load_lats.append(lat)
    return store_lats, load_lats


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

    # Scenario E: Straggler injection
    print("Scenario E: Straggler Injection (5% of RPCs are 10x slower)")
    s_lats, l_lats = scenario_straggler(
        stub, n_ops=args.n_ops, iterations=5,
        store_workers=args.store_workers, load_workers=args.load_workers,
    )
    print_stats("stores (with stragglers)", s_lats)
    print_stats("loads (with stragglers)", l_lats)
    if l_lats:
        p = percentiles(l_lats)
        tail_ratio = p["p99"] / p["p50"] if p["p50"] > 0 else 0
        print(f"  Load tail ratio (p99/p50): {tail_ratio:.1f}x")
    print()

    # Scenario F: Burst recovery
    print("Scenario F: Burst Recovery (60 stores → idle → 1 load)")
    recovery_lats = scenario_burst_recovery(
        stub, burst_size=60, iterations=args.iterations,
        store_workers=args.store_workers, load_workers=args.load_workers,
    )
    print_stats("post-burst load latency", recovery_lats)
    p95 = percentiles(recovery_lats).get("p95", 0)
    if p95 < max_acceptable_us:
        print(f"  PASS (p95={p95:.0f}us < {max_acceptable_us:.0f}us)")
    else:
        print(f"  FAIL (p95={p95:.0f}us >= {max_acceptable_us:.0f}us)")
        failures.append("Burst recovery")
    print()

    # Scenario G: Direction-asymmetric saturation
    print("Scenario G: Asymmetric Saturation (stores=50ms, loads=2ms)")
    asym_s, asym_l = scenario_asymmetric_saturation(
        store_workers=args.store_workers, load_workers=args.load_workers,
        iterations=args.iterations,
    )
    print_stats("stores (50ms RPC)", asym_s)
    print_stats("loads (2ms RPC)", asym_l)
    if asym_l:
        p = percentiles(asym_l)
        # Loads should stay near 2ms even under heavy store pressure
        load_ceiling_us = 10000  # 10ms — loads at 2ms RPC should never hit this
        if p["p95"] < load_ceiling_us:
            print(f"  PASS: loads independent of slow stores (p95={p['p95']:.0f}us)")
        else:
            print(f"  FAIL: loads affected by slow stores (p95={p['p95']:.0f}us >= {load_ceiling_us}us)")
            failures.append("Asymmetric saturation")
    print()

    # Scenario H: Intra-direction HOL
    print("Scenario H: Intra-Direction HOL (1 slow store + 7 fast stores)")
    intra_lats = scenario_intra_direction_hol(
        stub, store_workers=args.store_workers, load_workers=args.load_workers,
        iterations=10,
    )
    print_stats("fast stores behind straggler", intra_lats)
    if intra_lats:
        p = percentiles(intra_lats)
        # These fast stores (5ms RPC) can't report until the 100ms straggler
        # at deque position 0 finishes. Expected: ~100ms, not ~5ms.
        expected_ms = 5  # the fast stores' RPC time
        if p['avg'] / 1000 < expected_ms * 4:
            print(f"  PASS: fast stores report at ~{p['avg']/1000:.0f}ms (near {expected_ms}ms RPC time)")
        else:
            print(f"  WARN: fast stores delayed to ~{p['avg']/1000:.0f}ms (expected ~{expected_ms}ms)")
    print()

    # Scenario I: Reaping cadence sensitivity
    print("Scenario I: Reaping Cadence Sensitivity")
    for poll_ms in [1.0, 5.0, 10.0, 30.0]:
        s_lats, l_lats = scenario_reaping_cadence(
            stub, poll_interval_ms=poll_ms, n_ops=50,
            store_workers=args.store_workers, load_workers=args.load_workers,
        )
        all_lats = s_lats + l_lats
        p = percentiles(all_lats)
        if p:
            print(f"  poll={poll_ms:>5.1f}ms  avg={p['avg']:>8.0f}us  "
                  f"p50={p['p50']:>8.0f}us  p95={p['p95']:>8.0f}us")
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
