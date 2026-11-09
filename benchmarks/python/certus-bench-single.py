#!/usr/bin/env python3
"""Single-client pipelined benchmark for the Certus shmq Dispatcher.

Measures populate, hot lookup, and cold lookup throughput. gRPC futures pipelined
many RPCs down one channel; the shmq ``Ring`` is one-in-flight per channel, so
pipelining here is a pool of ``--pipeline-depth`` worker threads (via
``run_pipeline``), each holding its own channel and releasing it when the phase
ends (the server must expose at least ``--pipeline-depth`` + 1 --channels).

Usage:
    python certus-bench-single.py --block-size 2M --num-objects 16 --pipeline-depth 4
    python certus-bench-single.py --block-size 4M --num-objects 32 --pipeline-depth 8
"""

import argparse
import ctypes
import os
import statistics
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from certus_shmq_helpers import (
    RingError,
    add_shm_arg,
    connect,
    run_pipeline,
    single_region,
)

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
    """Allocate GPU memory, return (device_ptr, ipc_handle_bytes)."""
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
        raise argparse.ArgumentTypeError(f"size must be positive")
    return value * multiplier


# --- Benchmark phases ---

def phase_populate(ring, base_key, num_objects, block_size, batch_size,
                   pipeline_depth, gpu_ptrs, ipc_handles, gpu_device):
    """Populate objects into the server's memory tier, pipelined across threads."""
    total = num_objects
    pattern = bytes(0xAB for _ in range(block_size))

    # Write pattern to all GPU buffers
    for ptr in gpu_ptrs[:total]:
        gpu_write(ptr, pattern)

    def do_batch(start, end):
        entries = [
            (base_key + i, [single_region(ipc_handles[i], gpu_device, block_size)])
            for i in range(start, end)
        ]
        t0 = time.perf_counter()
        try:
            oks = ring.populate(entries)
            t1 = time.perf_counter()
            failed = sum(1 for ok in oks if not ok)
            return (end - start), failed, (t1 - t0) / (end - start)
        except RingError:
            return (end - start), (end - start), None

    latencies = []
    errors = 0
    batches = [
        (start, min(start + batch_size, total))
        for start in range(0, total, batch_size)
    ]
    for count, failed, lat in run_pipeline(
        ring, lambda se: do_batch(*se), batches, pipeline_depth
    ):
        errors += failed
        if lat is not None:
            latencies.append(lat)

    return latencies, errors


def _lookup_iterations(ring, key_sets, block_size, pipeline_depth, ipc_handles,
                       gpu_device):
    """Run one lookup RPC per key-set, pipelined across ``pipeline_depth`` threads.

    ``key_sets`` is a list of key lists (one per iteration). Each iteration looks
    up its keys against the (constant) handle buffers and syncs the GPU.
    """
    num_objects = len(key_sets[0]) if key_sets else 0

    def do_iter(keys):
        entries = [
            (k, [single_region(ipc_handles[i], gpu_device, block_size)])
            for i, k in enumerate(keys)
        ]
        t0 = time.perf_counter()
        try:
            oks = ring.lookup(entries)
            _libcudart.cudaDeviceSynchronize()
            t1 = time.perf_counter()
            failed = sum(1 for ok in oks if not ok)
            return failed, (t1 - t0) / num_objects
        except RingError:
            return num_objects, None

    latencies = []
    errors = 0
    for failed, lat in run_pipeline(ring, do_iter, key_sets, pipeline_depth):
        errors += failed
        if lat is not None:
            latencies.append(lat)
    return latencies, errors


def phase_hot_lookup(ring, base_key, num_objects, block_size, iterations,
                     pipeline_depth, ipc_handles, gpu_device):
    """Pipelined hot lookups — all objects are in memory tier."""
    keys = [base_key + i for i in range(num_objects)]
    key_sets = [list(keys) for _ in range(iterations)]
    return _lookup_iterations(ring, key_sets, block_size, pipeline_depth,
                              ipc_handles, gpu_device)


def phase_cold_lookup(ring, base_key, num_objects, block_size, iterations,
                      pipeline_depth, cold_ipc_handles, gpu_device):
    """Pipelined cold lookups — objects must be promoted from SSD."""
    key_sets = [
        [base_key + it * num_objects + i for i in range(num_objects)]
        for it in range(iterations)
    ]
    return _lookup_iterations(ring, key_sets, block_size, pipeline_depth,
                              cold_ipc_handles, gpu_device)


def print_phase(label, latencies, num_objects_per_iter, block_size, wall_time):
    """Print results for a benchmark phase."""
    if not latencies:
        print(f"  {label:<20} no data")
        return
    avg = statistics.mean(latencies)
    p50 = statistics.median(latencies)
    p99 = sorted(latencies)[int(len(latencies) * 0.99)]
    mn = min(latencies)
    mx = max(latencies)
    gbps = (block_size / avg) / (1024 * 1024 * 1024)
    total_bytes = len(latencies) * num_objects_per_iter * block_size
    wall_gbps = total_bytes / wall_time / (1024 * 1024 * 1024) if wall_time > 0 else 0

    print(f"  {label:<20} avg={avg*1e6:>8.1f} us  p50={p50*1e6:>8.1f} us  "
          f"p99={p99*1e6:>8.1f} us  min={mn*1e6:>8.1f} us  max={mx*1e6:>8.1f} us")
    print(f"  {'':20} throughput={wall_gbps:>6.2f} GB/s  wall={wall_time*1e3:.1f} ms")


def main():
    parser = argparse.ArgumentParser(
        description="Single-client pipelined Certus benchmark")
    add_shm_arg(parser)
    parser.add_argument("--block-size", type=parse_size, default=2 * 1024 * 1024,
                        help="Block size (default: 2M)")
    parser.add_argument("--num-objects", type=int, default=16,
                        help="Objects per lookup batch (default: 16)")
    parser.add_argument("--iterations", type=int, default=100,
                        help="Lookup iterations per phase (default: 100)")
    parser.add_argument("--pipeline-depth", type=int, default=4,
                        help="Concurrent RPCs in flight (default: 4)")
    parser.add_argument("--batch-size", type=int, default=10,
                        help="Objects per populate RPC (default: 10)")
    parser.add_argument("--pool-capacity", type=int, default=None,
                        help="Objects in memory-tier pool (default: auto from block-size)")
    parser.add_argument("--gpu", type=int, default=0,
                        help="GPU device index (default: 0)")
    parser.add_argument("--writes-settle", type=float, default=30.0,
                        help="Seconds to wait for write-through after populate (default: 30)")
    args = parser.parse_args()

    block_size = args.block_size
    num_objects = args.num_objects
    iterations = args.iterations
    pipeline_depth = args.pipeline_depth
    batch_size = args.batch_size

    # Auto-size pool: 1 GiB worth of objects or 512, whichever is larger
    if args.pool_capacity:
        pool_capacity = args.pool_capacity
    else:
        pool_capacity = max(512, (1024 * 1024 * 1024) // block_size)

    cold_objects = num_objects * iterations
    total_objects = pool_capacity + cold_objects
    base_key = 10_000_000

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 70)
    print("Certus Single-Client Benchmark")
    print("=" * 70)
    print(f"  Server:          {args.shm_path}")
    print(f"  GPU:             {args.gpu}")
    print(f"  Block size:      {block_size // (1024*1024)} MiB")
    print(f"  Objects/batch:   {num_objects}")
    print(f"  Iterations:      {iterations}")
    print(f"  Pipeline depth:  {pipeline_depth}")
    print(f"  Pool capacity:   {pool_capacity} objects ({pool_capacity * block_size // (1024*1024)} MiB)")
    print(f"  Cold objects:    {cold_objects}")
    print(f"  Total objects:   {total_objects}")
    print()

    # Allocate GPU buffers
    print("  Allocating GPU buffers...")
    populate_ptrs = []
    populate_handles = []
    for _ in range(total_objects):
        ptr, handle = cuda_alloc(block_size)
        populate_ptrs.append(ptr)
        populate_handles.append(handle)

    hot_ptrs = []
    hot_handles = []
    for _ in range(num_objects):
        ptr, handle = cuda_alloc(block_size)
        hot_ptrs.append(ptr)
        hot_handles.append(handle)

    cold_ptrs = []
    cold_handles = []
    for _ in range(num_objects):
        ptr, handle = cuda_alloc(block_size)
        cold_ptrs.append(ptr)
        cold_handles.append(handle)

    # Connect to server
    ring = connect(args.shm_path)
    # Peak = pipeline_depth pool workers + 1 for this (main) thread's own direct
    # calls (warmup lookup, clear_memory_tier), which it holds across the phases.
    needed_channels = pipeline_depth + 1
    if ring.channel_count < needed_channels:
        print(f"  WARNING: server exposes {ring.channel_count} channels but "
              f"pipeline-depth+1 is {needed_channels}; extra threads will error. "
              f"Launch certus-server with --channels >= {needed_channels}.")

    # --- Phase 1: Populate ---
    print("  Populating...")
    t0 = time.perf_counter()
    pop_latencies, pop_errors = phase_populate(
        ring, base_key, total_objects, block_size, batch_size,
        pipeline_depth, populate_ptrs, populate_handles, args.gpu,
    )
    t_populate = time.perf_counter() - t0
    print(f"    populated {total_objects} objects in {t_populate:.1f}s "
          f"({pop_errors} errors)")

    # Wait for write-through to SSD
    if args.writes_settle > 0:
        print(f"  Waiting {args.writes_settle}s for write-through to drain...")
        time.sleep(args.writes_settle)

    # --- Phase 2: Hot lookups ---
    # The last pool_capacity objects are still in memory tier.
    # We use the last num_objects of those for hot lookups.
    hot_base_key = base_key + total_objects - num_objects

    # Warmup
    warmup_entries = [
        (hot_base_key + i, [single_region(hot_handles[i], args.gpu, block_size)])
        for i in range(num_objects)
    ]
    try:
        ring.lookup(warmup_entries)
        _libcudart.cudaDeviceSynchronize()
    except RingError:
        pass

    print("  Running hot lookups...")
    t0 = time.perf_counter()
    hot_latencies, hot_errors = phase_hot_lookup(
        ring, hot_base_key, num_objects, block_size, iterations,
        pipeline_depth, hot_handles, args.gpu,
    )
    t_hot = time.perf_counter() - t0

    # --- Phase 3: Cold lookups ---
    # Evict memory tier so lookups hit SSD
    print("  Clearing memory tier for cold lookups...")
    try:
        ring.clear_memory_tier()
    except RingError as e:
        print(f"    WARNING: ClearMemoryTier failed: {e}")

    print("  Running cold lookups...")
    t0 = time.perf_counter()
    cold_latencies, cold_errors = phase_cold_lookup(
        ring, base_key, num_objects, block_size, iterations,
        pipeline_depth, cold_handles, args.gpu,
    )
    t_cold = time.perf_counter() - t0

    # --- Results ---
    print()
    print("=" * 70)
    print(f"Results (block={block_size//(1024*1024)} MiB, objects/batch={num_objects}, "
          f"pipeline={pipeline_depth})")
    print("=" * 70)
    print()
    print_phase("Populate", pop_latencies, batch_size, block_size, t_populate)
    print()
    print_phase("Lookup (hot)", hot_latencies, num_objects, block_size, t_hot)
    if hot_errors:
        print(f"  {'':20} errors={hot_errors}")
    print()
    print_phase("Lookup (cold)", cold_latencies, num_objects, block_size, t_cold)
    if cold_errors:
        print(f"  {'':20} errors={cold_errors}")
    print()

    if hot_latencies and cold_latencies:
        ratio = statistics.mean(cold_latencies) / statistics.mean(hot_latencies)
        print(f"  Cold/Hot ratio:  {ratio:.1f}x latency")

    print()
    total_wall = t_populate + args.writes_settle + t_hot + t_cold
    print(f"  Total wall time: {total_wall:.1f}s")

    # Cleanup
    for ptr in populate_ptrs:
        cuda_free(ptr)
    for ptr in hot_ptrs:
        cuda_free(ptr)
    for ptr in cold_ptrs:
        cuda_free(ptr)
    ring.close()  # releases this thread's channel, then drops the mapping


if __name__ == "__main__":
    main()
