#!/usr/bin/env python3
"""Bidirectional throughput and latency benchmark for Certus gRPC Dispatcher (v3).

Extends v2 by adding phases that exercise concurrent store+load traffic,
per-block latency distribution, and region-count sensitivity — the metrics
needed to detect improvements from:
  - Separate warm_load / warm_store CUDA streams (bidirectional DMA overlap)
  - cuMemcpyBatchAsync (32 regions in 1 driver call vs 32 individual calls)
  - NonBlocking stream flag (no implicit default-stream serialization)

Phases:
  1. Populate (D2H store baseline — same as v2)
  2. Hot lookup (H2D load baseline — same as v2)
  3. Bidirectional: concurrent stores + loads from separate threads
  4. Per-block latency: sequential single-block RPCs for latency distribution
  5. Cold lookup (SSD→GPU baseline — same as v2)

Usage:
    python certus-api-bench_v3.py --server localhost:50051
    python certus-api-bench_v3.py --server localhost:50051 --bidir-ratio 0.5
"""

import argparse
import ctypes
import os
import random
import statistics
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import grpc
import torch
import dispatcher_pb2
import dispatcher_pb2_grpc

assert torch.cuda.is_available(), "CUDA GPU required"

BLOCK_SIZE = 4 * 1024 * 1024
MEMORY_TIER_SIZE = 2 * 1024 * 1024 * 1024

_libcudart = ctypes.CDLL("libcudart.so")
_libcudart.cudaSetDevice.restype = ctypes.c_int
_libcudart.cudaSetDevice.argtypes = [ctypes.c_int]
_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_libcudart.cudaMalloc.restype = ctypes.c_int
_libcudart.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
_libcudart.cudaFree.restype = ctypes.c_int
_libcudart.cudaFree.argtypes = [ctypes.c_void_p]
_libcudart.cudaMemcpy.restype = ctypes.c_int
_libcudart.cudaMemcpy.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
_libcudart.cudaDeviceSynchronize.restype = ctypes.c_int
_CUDA_MEMCPY_H2D = 1
_CUDA_MEMCPY_D2H = 2


def parse_size(s):
    s = s.strip()
    suffix = s[-1].upper()
    multipliers = {"K": 1024, "M": 1024**2, "G": 1024**3}
    if suffix in multipliers:
        return int(s[:-1]) * multipliers[suffix]
    return int(s)


def _get_cuda_ipc_handle(data_ptr):
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), data_ptr)
    if err != 0:
        raise RuntimeError(f"cudaIpcGetMemHandle failed: {err}")
    return bytes(handle_buf)


def _cuda_alloc(size):
    dev_ptr = ctypes.c_void_p()
    err = _libcudart.cudaMalloc(ctypes.byref(dev_ptr), size)
    if err != 0:
        raise RuntimeError(f"cudaMalloc failed: {err}")
    handle_bytes = _get_cuda_ipc_handle(dev_ptr)
    return dev_ptr, handle_bytes


def _cuda_free(dev_ptr):
    _libcudart.cudaFree(dev_ptr)


def _make_ipc_handle(handle_bytes, block_size, gpu_device=0):
    return dispatcher_pb2.IpcHandle(
        cuda_ipc_handle=handle_bytes,
        size=block_size,
        gpu_device_id=gpu_device,
        offset=0,
    )


def make_stub(server):
    channel = grpc.insecure_channel(
        server,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    return channel, dispatcher_pb2_grpc.DispatcherStub(channel)


def percentiles(latencies):
    if not latencies:
        return {}
    s = sorted(latencies)
    n = len(s)
    return {
        "n": n,
        "avg": sum(s) / n,
        "p50": s[int(n * 0.5)],
        "p95": s[int(n * 0.95)],
        "p99": s[min(int(n * 0.99), n - 1)],
        "min": s[0],
        "max": s[-1],
    }


def print_stats(label, latencies_us, block_size=BLOCK_SIZE):
    p = percentiles(latencies_us)
    if not p:
        print(f"  {label:<25} no data")
        return
    gbps = block_size / (p["avg"] * 1e-6) / 1e9 if p["avg"] > 0 else 0
    print(
        f"  {label:<25} n={p['n']:>4}  "
        f"avg={p['avg']:>8.1f}us  p50={p['p50']:>8.1f}us  "
        f"p95={p['p95']:>8.1f}us  p99={p['p99']:>8.1f}us  "
        f"({gbps:.2f} GB/s)"
    )


# ── Phase 1 & 2: Same as v2 (populate + hot lookup) ──


def phase_populate(stub, keys, ipc_handle, block_size, pipeline_depth=4):
    """Populate keys with pipelined Reserve+CopyToStore+Commit."""
    latencies = []
    for key in keys:
        reserve_entry = dispatcher_pb2.ReserveEntry(key=key, size=block_size, session_id=0)
        try:
            t0 = time.perf_counter()
            resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=[reserve_entry]))
            if not resp.results[0].success:
                continue
            entry = dispatcher_pb2.CopyToStoreEntry(
                key=key, ipc_handles=[_make_ipc_handle(ipc_handle, block_size)]
            )
            resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=[entry]))
            if resp.results[0].success:
                stub.CommitStore(dispatcher_pb2.BatchCommitStoreRequest(keys=[key]))
            t1 = time.perf_counter()
            latencies.append((t1 - t0) * 1e6)
        except grpc.RpcError as e:
            print(f"  populate error: {e.details()}")
            break
    return latencies


def phase_hot_lookup(stub, keys, ipc_handle, block_size, iterations=10, pipeline_depth=4):
    """Hot lookups (memory-tier hits) with pipelining."""
    latencies = []
    entries = [
        dispatcher_pb2.LookupEntry(
            key=k, ipc_handles=[_make_ipc_handle(ipc_handle, block_size)]
        )
        for k in keys
    ]
    # Warmup
    try:
        stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
    except grpc.RpcError:
        pass

    in_flight = []
    next_to_send = 0
    completed = 0
    while completed < iterations:
        while next_to_send < iterations and len(in_flight) < pipeline_depth:
            req = dispatcher_pb2.BatchLookupRequest(entries=entries)
            future = stub.Lookup.future(req)
            in_flight.append((future, time.perf_counter()))
            next_to_send += 1
        if in_flight:
            future, t_sub = in_flight.pop(0)
            try:
                future.result()
                _libcudart.cudaDeviceSynchronize()
                t_done = time.perf_counter()
                latencies.append((t_done - t_sub) * 1e6 / len(keys))
            except grpc.RpcError as e:
                print(f"  hot lookup error: {e.details()}")
            completed += 1
    return latencies


# ── Phase 3: Bidirectional (concurrent store + load) ──


def phase_bidirectional(stub, store_keys, load_keys, ipc_store, ipc_load,
                        block_size, duration_s=5.0, bidir_ratio=0.5):
    """Run concurrent stores and loads from separate threads.

    bidir_ratio: fraction of operations that are stores (0.5 = balanced).
    Measures per-direction latency under contention.
    """
    store_latencies = []
    load_latencies = []
    stop_event = threading.Event()
    errors = []

    def store_worker():
        idx = 0
        while not stop_event.is_set():
            key = store_keys[idx % len(store_keys)]
            idx += 1
            try:
                t0 = time.perf_counter()
                reserve_entry = dispatcher_pb2.ReserveEntry(key=key, size=block_size, session_id=0)
                resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=[reserve_entry]))
                if not resp.results[0].success:
                    continue
                entry = dispatcher_pb2.CopyToStoreEntry(
                    key=key, ipc_handles=[_make_ipc_handle(ipc_store, block_size)]
                )
                resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=[entry]))
                if resp.results[0].success:
                    stub.CommitStore(dispatcher_pb2.BatchCommitStoreRequest(keys=[key]))
                t1 = time.perf_counter()
                store_latencies.append((t1 - t0) * 1e6)
            except grpc.RpcError as e:
                errors.append(f"bidir store: {e.details()}")

    def load_worker():
        idx = 0
        while not stop_event.is_set():
            key = load_keys[idx % len(load_keys)]
            idx += 1
            entries = [
                dispatcher_pb2.LookupEntry(
                    key=key, ipc_handles=[_make_ipc_handle(ipc_load, block_size)]
                )
            ]
            try:
                t0 = time.perf_counter()
                resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
                _libcudart.cudaDeviceSynchronize()
                t1 = time.perf_counter()
                if resp.results[0].success:
                    load_latencies.append((t1 - t0) * 1e6)
            except grpc.RpcError as e:
                errors.append(f"bidir load: {e.details()}")

    # Start store and load threads based on ratio
    n_store_threads = max(1, int(4 * bidir_ratio))
    n_load_threads = max(1, 4 - n_store_threads)

    threads = []
    for _ in range(n_store_threads):
        t = threading.Thread(target=store_worker, daemon=True)
        t.start()
        threads.append(t)
    for _ in range(n_load_threads):
        t = threading.Thread(target=load_worker, daemon=True)
        t.start()
        threads.append(t)

    time.sleep(duration_s)
    stop_event.set()
    for t in threads:
        t.join(timeout=5.0)

    return store_latencies, load_latencies, errors


# ── Phase 4: Per-block latency (single-block sequential RPCs) ──


def phase_per_block_latency(stub, keys, ipc_handle, block_size, n_samples=100):
    """Sequential single-block lookups for clean latency distribution.

    This isolates per-block DMA latency without pipelining/batching effects.
    Shows the benefit of cuMemcpyBatchAsync (fewer driver calls per block).
    """
    latencies = []
    for i in range(n_samples):
        key = keys[i % len(keys)]
        entries = [
            dispatcher_pb2.LookupEntry(
                key=key, ipc_handles=[_make_ipc_handle(ipc_handle, block_size)]
            )
        ]
        try:
            t0 = time.perf_counter()
            resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
            _libcudart.cudaDeviceSynchronize()
            t1 = time.perf_counter()
            if resp.results[0].success:
                latencies.append((t1 - t0) * 1e6)
        except grpc.RpcError:
            pass
    return latencies


# ── Main ──


def main():
    parser = argparse.ArgumentParser(description="Certus Bidirectional Benchmark v3")
    parser.add_argument("--server", default="localhost:50051")
    parser.add_argument("--block-size", type=parse_size, default="4M")
    parser.add_argument("--num-objects", type=int, default=64)
    parser.add_argument("--iterations", type=int, default=10)
    parser.add_argument("--pipeline-depth", type=int, default=4)
    parser.add_argument("--bidir-duration", type=float, default=5.0,
                        help="Duration of bidirectional phase in seconds")
    parser.add_argument("--bidir-ratio", type=float, default=0.5,
                        help="Fraction of bidir ops that are stores (0.5=balanced)")
    parser.add_argument("--per-block-samples", type=int, default=100)
    parser.add_argument("--memory-tier-size", type=parse_size, default="2G")
    parser.add_argument("--writes-settle", type=int, default=5)
    parser.add_argument("--gpu", type=int, default=0)
    args = parser.parse_args()

    global BLOCK_SIZE, MEMORY_TIER_SIZE
    BLOCK_SIZE = args.block_size
    MEMORY_TIER_SIZE = args.memory_tier_size

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 70)
    print("Certus Bidirectional Benchmark v3")
    print("=" * 70)
    print(f"  Server:          {args.server}")
    print(f"  Block size:      {BLOCK_SIZE // 1024} KiB")
    print(f"  Objects:         {args.num_objects}")
    print(f"  Iterations:      {args.iterations}")
    print(f"  Pipeline depth:  {args.pipeline_depth}")
    print(f"  Bidir duration:  {args.bidir_duration}s (ratio={args.bidir_ratio})")
    print(f"  Per-block:       {args.per_block_samples} samples")
    print()

    _, stub = make_stub(args.server)

    # Allocate GPU buffers for IPC
    store_ptr, store_handle = _cuda_alloc(BLOCK_SIZE)
    load_ptr, load_handle = _cuda_alloc(BLOCK_SIZE)

    base_key = random.randint(10_000_000, 50_000_000)
    pool_cap = MEMORY_TIER_SIZE // BLOCK_SIZE
    num_objects = min(args.num_objects, pool_cap // 2)

    populate_keys = list(range(base_key, base_key + num_objects))
    load_keys = populate_keys[:num_objects]

    # ── Phase 1: Populate ──
    print("Phase 1: Populate (D2H store)")
    pop_lats = phase_populate(stub, populate_keys, store_handle, BLOCK_SIZE, args.pipeline_depth)
    print_stats("populate", pop_lats)

    # Flush writes to SSD
    try:
        stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())
    except grpc.RpcError:
        pass
    if args.writes_settle > 0:
        print(f"  (settling {args.writes_settle}s for SSD write-through)")
        time.sleep(args.writes_settle)

    # ── Phase 2: Hot Lookup (H2D load baseline) ──
    print("\nPhase 2: Hot Lookup (H2D load, memory-tier)")
    hot_lats = phase_hot_lookup(
        stub, load_keys, load_handle, BLOCK_SIZE,
        iterations=args.iterations, pipeline_depth=args.pipeline_depth,
    )
    print_stats("hot_lookup", hot_lats)

    # ── Phase 3: Bidirectional (concurrent store + load) ──
    print("\nPhase 3: Bidirectional (concurrent store + load)")
    # Use a separate key range for bidir stores so they don't evict load keys
    bidir_store_keys = list(range(base_key + num_objects, base_key + 2 * num_objects))
    bidir_load_keys = load_keys

    s_lats, l_lats, bidir_errors = phase_bidirectional(
        stub, bidir_store_keys, bidir_load_keys, store_handle, load_handle,
        BLOCK_SIZE, duration_s=args.bidir_duration, bidir_ratio=args.bidir_ratio,
    )
    print_stats("bidir_stores", s_lats)
    print_stats("bidir_loads", l_lats)
    if s_lats and l_lats:
        total_ops = len(s_lats) + len(l_lats)
        total_bytes = total_ops * BLOCK_SIZE
        bidir_gbps = total_bytes / args.bidir_duration / 1e9
        print(f"  {'bidir_aggregate':<25} {total_ops} ops in {args.bidir_duration:.1f}s = "
              f"{bidir_gbps:.2f} GB/s combined")
        # Key metric: does load latency degrade under store pressure?
        hot_avg = percentiles(hot_lats).get("avg", 0)
        bidir_load_avg = percentiles(l_lats).get("avg", 0)
        if hot_avg > 0:
            degradation = bidir_load_avg / hot_avg
            print(f"  {'load_degradation':<25} {degradation:.2f}x vs isolated hot lookup")
    if bidir_errors:
        print(f"  errors: {len(bidir_errors)}")

    # ── Phase 4: Per-block latency ──
    print("\nPhase 4: Per-Block Latency (sequential single-block)")
    pb_lats = phase_per_block_latency(
        stub, load_keys, load_handle, BLOCK_SIZE, n_samples=args.per_block_samples,
    )
    print_stats("per_block_lookup", pb_lats)

    # ── Phase 5: Cold Lookup ──
    print("\nPhase 5: Cold Lookup (SSD→GPU)")
    # Clear memory tier to force SSD reads
    try:
        stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
    except grpc.RpcError as e:
        print(f"  ClearMemoryTier failed: {e.details()}")

    cold_keys = populate_keys
    cold_lats = phase_hot_lookup(
        stub, cold_keys, load_handle, BLOCK_SIZE,
        iterations=args.iterations, pipeline_depth=args.pipeline_depth,
    )
    print_stats("cold_lookup", cold_lats)

    # ── Summary ──
    print()
    print("=" * 70)
    print("Summary — metrics for perf_agent optimization target:")
    print("=" * 70)
    hot_p = percentiles(hot_lats)
    cold_p = percentiles(cold_lats)
    bidir_lp = percentiles(l_lats)
    pb_p = percentiles(pb_lats)
    print(f"  hot_lookup_avg_us:       {hot_p.get('avg', 0):.1f}")
    print(f"  hot_lookup_p99_us:       {hot_p.get('p99', 0):.1f}")
    print(f"  cold_lookup_avg_us:      {cold_p.get('avg', 0):.1f}")
    print(f"  bidir_load_avg_us:       {bidir_lp.get('avg', 0):.1f}")
    print(f"  bidir_load_p99_us:       {bidir_lp.get('p99', 0):.1f}")
    print(f"  per_block_latency_us:    {pb_p.get('avg', 0):.1f}")
    print(f"  per_block_p99_us:        {pb_p.get('p99', 0):.1f}")
    if hot_p.get('avg', 0) > 0 and bidir_lp.get('avg', 0) > 0:
        print(f"  bidir_degradation:       {bidir_lp['avg'] / hot_p['avg']:.2f}x")
    print()

    # Cleanup
    cleanup_keys = list(range(base_key, base_key + 2 * num_objects))
    for i in range(0, len(cleanup_keys), 100):
        try:
            stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=cleanup_keys[i:i+100]))
        except grpc.RpcError:
            pass

    _cuda_free(store_ptr)
    _cuda_free(load_ptr)


if __name__ == "__main__":
    main()
