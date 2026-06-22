#!/usr/bin/env python3
"""Single-client pipelined benchmark for the Certus gRPC Dispatcher.

Measures populate, hot lookup, and cold lookup throughput using pipelined
gRPC futures to saturate the server's GPU PCIe link from a single client.

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

def phase_populate(stub, base_key, num_objects, block_size, batch_size, pipeline_depth, gpu_ptrs, ipc_handles):
    """Populate objects into the server's memory tier with pipelined RPCs."""
    total = num_objects
    pattern = bytes(0xAB for _ in range(block_size))

    # Write pattern to all GPU buffers
    for ptr in gpu_ptrs[:total]:
        gpu_write(ptr, pattern)

    latencies = []
    errors = 0
    in_flight = []
    next_to_send = 0
    completed = 0

    while completed < total:
        while next_to_send < total and len(in_flight) < pipeline_depth * batch_size:
            batch_end = min(next_to_send + batch_size, total)
            entries = []
            for i in range(next_to_send, batch_end):
                entries.append(dispatcher_pb2.PopulateEntry(
                    key=base_key + i,
                    ipc_handle=dispatcher_pb2.IpcHandle(
                        cuda_ipc_handle=ipc_handles[i],
                        size=block_size,
                    ),
                ))
            req = dispatcher_pb2.BatchPopulateRequest(entries=entries)
            future = stub.Populate.future(req)
            in_flight.append((batch_end - next_to_send, future, time.perf_counter()))
            next_to_send = batch_end

        if in_flight:
            count, future, t0 = in_flight.pop(0)
            try:
                resp = future.result()
                t1 = time.perf_counter()
                failed = sum(1 for r in resp.results if not r.success)
                errors += failed
                latencies.append((t1 - t0) / count)
            except grpc.RpcError:
                errors += count
            completed += count

    return latencies, errors


def phase_hot_lookup(stub, base_key, num_objects, block_size, iterations, pipeline_depth, ipc_handles):
    """Pipelined hot lookups — all objects are in memory tier."""
    keys = [base_key + i for i in range(num_objects)]
    latencies = []
    errors = 0
    in_flight = []
    next_to_send = 0
    completed = 0

    while completed < iterations:
        while next_to_send < iterations and len(in_flight) < pipeline_depth:
            entries = [
                dispatcher_pb2.LookupEntry(
                    key=k,
                    ipc_handle=dispatcher_pb2.IpcHandle(
                        cuda_ipc_handle=ipc_handles[i],
                        size=block_size,
                    ),
                )
                for i, k in enumerate(keys)
            ]
            req = dispatcher_pb2.BatchLookupRequest(entries=entries)
            future = stub.Lookup.future(req)
            in_flight.append((future, time.perf_counter()))
            next_to_send += 1

        if in_flight:
            future, t0 = in_flight.pop(0)
            try:
                resp = future.result()
                _libcudart.cudaDeviceSynchronize()
                t1 = time.perf_counter()
                failed = sum(1 for r in resp.results if not r.success)
                errors += failed
                latencies.append((t1 - t0) / num_objects)
            except grpc.RpcError:
                errors += num_objects
            completed += 1

    return latencies, errors


def phase_cold_lookup(stub, base_key, num_objects, block_size, iterations, pipeline_depth, cold_ipc_handles):
    """Pipelined cold lookups — objects must be promoted from SSD."""
    latencies = []
    errors = 0
    in_flight = []
    next_to_send = 0
    completed = 0

    while completed < iterations:
        while next_to_send < iterations and len(in_flight) < pipeline_depth:
            cold_start = next_to_send * num_objects
            keys = [base_key + cold_start + i for i in range(num_objects)]
            entries = [
                dispatcher_pb2.LookupEntry(
                    key=k,
                    ipc_handle=dispatcher_pb2.IpcHandle(
                        cuda_ipc_handle=cold_ipc_handles[i],
                        size=block_size,
                    ),
                )
                for i, k in enumerate(keys)
            ]
            req = dispatcher_pb2.BatchLookupRequest(entries=entries)
            future = stub.Lookup.future(req)
            in_flight.append((future, time.perf_counter()))
            next_to_send += 1

        if in_flight:
            future, t0 = in_flight.pop(0)
            try:
                resp = future.result()
                _libcudart.cudaDeviceSynchronize()
                t1 = time.perf_counter()
                failed = sum(1 for r in resp.results if not r.success)
                errors += failed
                latencies.append((t1 - t0) / num_objects)
            except grpc.RpcError:
                errors += num_objects
            completed += 1

    return latencies, errors


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
    parser.add_argument("--server", default="localhost:50051",
                        help="Server address (default: localhost:50051)")
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
    print(f"  Server:          {args.server}")
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
    channel = grpc.insecure_channel(
        args.server,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    # --- Phase 1: Populate ---
    print("  Populating...")
    t0 = time.perf_counter()
    pop_latencies, pop_errors = phase_populate(
        stub, base_key, total_objects, block_size, batch_size,
        pipeline_depth, populate_ptrs, populate_handles,
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
    entries = [
        dispatcher_pb2.LookupEntry(
            key=hot_base_key + i,
            ipc_handle=dispatcher_pb2.IpcHandle(cuda_ipc_handle=hot_handles[i], size=block_size),
        )
        for i in range(num_objects)
    ]
    try:
        stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
        _libcudart.cudaDeviceSynchronize()
    except grpc.RpcError:
        pass

    print("  Running hot lookups...")
    t0 = time.perf_counter()
    hot_latencies, hot_errors = phase_hot_lookup(
        stub, hot_base_key, num_objects, block_size, iterations,
        pipeline_depth, hot_handles,
    )
    t_hot = time.perf_counter() - t0

    # --- Phase 3: Cold lookups ---
    # Evict memory tier so lookups hit SSD
    print("  Clearing memory tier for cold lookups...")
    try:
        stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
    except grpc.RpcError as e:
        print(f"    WARNING: ClearMemoryTier failed: {e.details()}")

    print("  Running cold lookups...")
    t0 = time.perf_counter()
    cold_latencies, cold_errors = phase_cold_lookup(
        stub, base_key, num_objects, block_size, iterations,
        pipeline_depth, cold_handles,
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
    channel.close()


if __name__ == "__main__":
    main()
