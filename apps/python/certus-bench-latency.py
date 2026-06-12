#!/usr/bin/env python3
"""Single-client latency benchmark for the Certus gRPC Dispatcher.

Measures per-operation latency for sporadic small-batch requests (no pipelining).
Each RPC is a single batch of N objects, sent one at a time with the response
fully received before the next is sent. Reports percentile latency distribution.

Usage:
    python certus-bench-latency.py --block-size 2M --num-objects 20 --iterations 200
"""

import argparse
import ctypes
import os
import statistics
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import grpc
import dispatcher_pb2
import dispatcher_pb2_grpc

# --- CUDA helpers ---

_libcudart = ctypes.CDLL("libcudart.so")
_libcudart.cudaSetDevice.restype = ctypes.c_int
_libcudart.cudaSetDevice.argtypes = [ctypes.c_int]
_libcudart.cudaMalloc.restype = ctypes.c_int
_libcudart.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
_libcudart.cudaFree.restype = ctypes.c_int
_libcudart.cudaFree.argtypes = [ctypes.c_void_p]
_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_libcudart.cudaMemcpy.restype = ctypes.c_int
_libcudart.cudaMemcpy.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
_libcudart.cudaDeviceSynchronize.restype = ctypes.c_int
_CUDA_MEMCPY_H2D = 1


def cuda_alloc(size):
    dev_ptr = ctypes.c_void_p()
    err = _libcudart.cudaMalloc(ctypes.byref(dev_ptr), size)
    if err != 0:
        raise RuntimeError(f"cudaMalloc failed: {err}")
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), dev_ptr)
    if err != 0:
        raise RuntimeError(f"cudaIpcGetMemHandle failed: {err}")
    return dev_ptr, bytes(handle_buf)


def cuda_free(dev_ptr):
    _libcudart.cudaFree(dev_ptr)


def gpu_write(dev_ptr, data):
    buf = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
    _libcudart.cudaMemcpy(dev_ptr, ctypes.byref(buf), len(data), _CUDA_MEMCPY_H2D)


def parse_size(s):
    s = s.strip()
    if not s:
        raise argparse.ArgumentTypeError("empty size string")
    suffix = s[-1].upper()
    multipliers = {"K": 1024, "M": 1024 * 1024, "G": 1024 * 1024 * 1024}
    if suffix in multipliers:
        num_str = s[:-1]
        multiplier = multipliers[suffix]
    else:
        num_str = s
        multiplier = 1
    try:
        value = int(num_str)
    except ValueError:
        raise argparse.ArgumentTypeError(f"invalid size: '{num_str}'")
    if value <= 0:
        raise argparse.ArgumentTypeError("size must be positive")
    return value * multiplier


def print_latency(label, latencies_us):
    """Print latency percentile distribution."""
    if not latencies_us:
        print(f"  {label:<20} no data")
        return
    latencies_us.sort()
    n = len(latencies_us)
    avg = statistics.mean(latencies_us)
    p50 = latencies_us[int(n * 0.50)]
    p90 = latencies_us[int(n * 0.90)]
    p95 = latencies_us[int(n * 0.95)]
    p99 = latencies_us[min(int(n * 0.99), n - 1)]
    mn = latencies_us[0]
    mx = latencies_us[-1]

    print(f"  {label:<20} n={n}")
    print(f"    avg   = {avg:>8.1f} us")
    print(f"    p50   = {p50:>8.1f} us")
    print(f"    p90   = {p90:>8.1f} us")
    print(f"    p95   = {p95:>8.1f} us")
    print(f"    p99   = {p99:>8.1f} us")
    print(f"    min   = {mn:>8.1f} us")
    print(f"    max   = {mx:>8.1f} us")


def main():
    parser = argparse.ArgumentParser(
        description="Single-client latency benchmark (no pipelining)")
    parser.add_argument("--server", default="localhost:50051")
    parser.add_argument("--block-size", type=parse_size, default=2 * 1024 * 1024,
                        help="Block size (default: 2M)")
    parser.add_argument("--num-objects", type=int, default=20,
                        help="Objects per batch/RPC (default: 20)")
    parser.add_argument("--iterations", type=int, default=200,
                        help="Number of RPC calls per phase (default: 200)")
    parser.add_argument("--warmup", type=int, default=10,
                        help="Warmup iterations (not counted) (default: 10)")
    parser.add_argument("--gpu", type=int, default=0,
                        help="GPU device index (default: 0)")
    parser.add_argument("--writes-settle", type=float, default=15.0,
                        help="Seconds to wait for write-through (default: 15)")
    args = parser.parse_args()

    block_size = args.block_size
    num_objects = args.num_objects
    iterations = args.iterations
    warmup = args.warmup
    base_key = 20_000_000

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 70)
    print("Certus Latency Benchmark (single client, no pipelining)")
    print("=" * 70)
    print(f"  Server:          {args.server}")
    print(f"  Block size:      {block_size // 1024} KiB")
    print(f"  Objects/batch:   {num_objects}")
    print(f"  Iterations:      {iterations}")
    print(f"  Warmup:          {warmup}")
    print()

    # Allocate GPU buffers — one set for populate, one for lookups
    print("  Allocating GPU buffers...")
    pop_ptrs, pop_handles = [], []
    for _ in range(num_objects):
        ptr, handle = cuda_alloc(block_size)
        pop_ptrs.append(ptr)
        pop_handles.append(handle)

    lookup_ptrs, lookup_handles = [], []
    for _ in range(num_objects):
        ptr, handle = cuda_alloc(block_size)
        lookup_ptrs.append(ptr)
        lookup_handles.append(handle)

    # Fill populate buffers with data
    pattern = bytes(0xCD for _ in range(block_size))
    for ptr in pop_ptrs:
        gpu_write(ptr, pattern)

    # Connect
    channel = grpc.insecure_channel(
        args.server,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    # --- Populate phase: measure per-batch latency ---
    print("  Running populate latency test...")
    pop_latencies = []
    for it in range(warmup + iterations):
        keys = [base_key + it * num_objects + i for i in range(num_objects)]
        entries = [
            dispatcher_pb2.PopulateEntry(
                key=k,
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=pop_handles[i],
                    size=block_size,
                ),
            )
            for i, k in enumerate(keys)
        ]
        req = dispatcher_pb2.BatchPopulateRequest(entries=entries)
        t0 = time.perf_counter()
        resp = stub.Populate(req)
        t1 = time.perf_counter()
        if it >= warmup:
            pop_latencies.append((t1 - t0) * 1e6)

    total_populated = (warmup + iterations) * num_objects

    # Wait for write-through
    if args.writes_settle > 0:
        print(f"  Waiting {args.writes_settle}s for write-through...")
        time.sleep(args.writes_settle)

    # --- Hot lookup phase: objects are in memory tier ---
    # Use the last batch's keys (still in memory tier)
    hot_keys = [base_key + (warmup + iterations - 1) * num_objects + i
                for i in range(num_objects)]

    print("  Running hot lookup latency test...")
    hot_latencies = []
    for it in range(warmup + iterations):
        entries = [
            dispatcher_pb2.LookupEntry(
                key=k,
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=lookup_handles[i],
                    size=block_size,
                ),
            )
            for i, k in enumerate(hot_keys)
        ]
        req = dispatcher_pb2.BatchLookupRequest(entries=entries)
        t0 = time.perf_counter()
        resp = stub.Lookup(req)
        _libcudart.cudaDeviceSynchronize()
        t1 = time.perf_counter()
        if it >= warmup:
            failed = sum(1 for r in resp.results if not r.success)
            if failed:
                continue  # skip failed iterations
            hot_latencies.append((t1 - t0) * 1e6)

    # --- Cold lookup phase: clear memory tier, force SSD reads ---
    print("  Clearing memory tier...")
    try:
        stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
    except grpc.RpcError as e:
        print(f"    WARNING: ClearMemoryTier failed: {e.details()}")

    # Use early keys (written to SSD during settle)
    cold_keys = [base_key + i for i in range(num_objects)]

    print("  Running cold lookup latency test...")
    cold_latencies = []
    for it in range(warmup + iterations):
        # Use different keys each iteration to avoid re-promote caching
        cold_iter_keys = [base_key + it * num_objects + i for i in range(num_objects)]
        entries = [
            dispatcher_pb2.LookupEntry(
                key=k,
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=lookup_handles[i],
                    size=block_size,
                ),
            )
            for i, k in enumerate(cold_iter_keys)
        ]
        req = dispatcher_pb2.BatchLookupRequest(entries=entries)
        t0 = time.perf_counter()
        resp = stub.Lookup(req)
        _libcudart.cudaDeviceSynchronize()
        t1 = time.perf_counter()
        if it >= warmup:
            failed = sum(1 for r in resp.results if not r.success)
            if failed:
                continue
            cold_latencies.append((t1 - t0) * 1e6)

    # --- Results ---
    print()
    print("=" * 70)
    print(f"Latency Results (batch={num_objects} x {block_size//1024} KiB = "
          f"{num_objects * block_size // (1024*1024)} MiB per RPC)")
    print("=" * 70)
    print()
    print("  Per-batch latency (one RPC round-trip):")
    print()
    print_latency("Populate", pop_latencies)
    print()
    print_latency("Lookup (hot)", hot_latencies)
    if hot_latencies:
        batch_bytes = num_objects * block_size
        avg_s = statistics.mean(hot_latencies) / 1e6
        print(f"    equiv  = {batch_bytes / avg_s / (1024**3):>8.2f} GB/s")
    print()
    print_latency("Lookup (cold)", cold_latencies)
    if cold_latencies:
        batch_bytes = num_objects * block_size
        avg_s = statistics.mean(cold_latencies) / 1e6
        print(f"    equiv  = {batch_bytes / avg_s / (1024**3):>8.2f} GB/s")
    print()

    # Per-object latency
    print("  Per-object latency (batch latency / num_objects):")
    print()
    if hot_latencies:
        per_obj = [l / num_objects for l in hot_latencies]
        print_latency("Hot per-object", per_obj)
    if cold_latencies:
        per_obj = [l / num_objects for l in cold_latencies]
        print_latency("Cold per-object", per_obj)

    # --- Summary ---
    print()
    print("=" * 70)
    print("Summary")
    print("=" * 70)
    batch_mib = num_objects * block_size / (1024 * 1024)
    print(f"  {'Phase':<16} {'Batch Latency':>14} {'Per-Object':>12} {'Throughput':>12}")
    print(f"  {'-'*16} {'-'*14} {'-'*12} {'-'*12}")
    for label, lats in [("Populate", pop_latencies),
                        ("Hot Lookup", hot_latencies),
                        ("Cold Lookup", cold_latencies)]:
        if lats:
            avg_us = statistics.mean(lats)
            per_obj_us = avg_us / num_objects
            gbps = (num_objects * block_size) / (avg_us / 1e6) / (1024**3)
            print(f"  {label:<16} {avg_us:>10.0f} us {per_obj_us:>9.0f} us {gbps:>9.2f} GB/s")
        else:
            print(f"  {label:<16} {'no data':>14}")
    print()

    # Cleanup
    for ptr in pop_ptrs:
        cuda_free(ptr)
    for ptr in lookup_ptrs:
        cuda_free(ptr)
    channel.close()


if __name__ == "__main__":
    main()
