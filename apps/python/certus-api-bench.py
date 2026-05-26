#!/usr/bin/env python3
"""Multi-client throughput and latency benchmark for the Certus gRPC Dispatcher.

Spawns N concurrent client threads, each issuing populate/lookup operations
with 4 MiB cache blocks. Measures both hot (memory-tier) and cold (SSD-tier)
throughput and latency.

Usage:
    python certus-test-client.py --clients 4 --server localhost:50051
"""

import argparse
import ctypes
import os
import random
import statistics
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import grpc
import torch
import dispatcher_pb2
import dispatcher_pb2_grpc

assert torch.cuda.is_available(), "CUDA GPU required"

BLOCK_SIZE = 4 * 1024 * 1024  # 4 MiB

_libcudart = ctypes.CDLL("libcudart.so")
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


def _get_cuda_ipc_handle(data_ptr):
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), data_ptr)
    if err != 0:
        raise RuntimeError(f"cudaIpcGetMemHandle failed with error {err}")
    return bytes(handle_buf)


class ClientResult:
    """Collects per-client benchmark results."""

    def __init__(self, client_id):
        self.client_id = client_id
        self.populate_latencies = []
        self.hot_latencies = []
        self.cold_latencies = []
        self.errors = []


def _make_pattern(key, block_size):
    """Create a deterministic byte pattern for a given key."""
    rng = random.Random(key)
    return bytes(rng.getrandbits(8) for _ in range(block_size))


def _cuda_alloc(size):
    """Allocate GPU memory via cudaMalloc and return (device_ptr, ipc_handle_bytes)."""
    dev_ptr = ctypes.c_void_p()
    err = _libcudart.cudaMalloc(ctypes.byref(dev_ptr), size)
    if err != 0:
        raise RuntimeError(f"cudaMalloc failed: {err}")
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), dev_ptr)
    if err != 0:
        raise RuntimeError(f"cudaIpcGetMemHandle failed: {err}")
    return dev_ptr, bytes(handle_buf)


def _cuda_free(dev_ptr):
    _libcudart.cudaFree(dev_ptr)


def _gpu_write(dev_ptr, data):
    """Copy bytes from host to GPU device memory."""
    buf = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
    err = _libcudart.cudaMemcpy(dev_ptr, ctypes.byref(buf), len(data), _CUDA_MEMCPY_H2D)
    if err != 0:
        raise RuntimeError(f"cudaMemcpy H2D failed: {err}")


def _gpu_read(dev_ptr, size):
    """Copy bytes from GPU device memory to host."""
    buf = (ctypes.c_ubyte * size)()
    err = _libcudart.cudaMemcpy(ctypes.byref(buf), dev_ptr, size, _CUDA_MEMCPY_D2H)
    if err != 0:
        raise RuntimeError(f"cudaMemcpy D2H failed: {err}")
    return bytes(buf)


def run_integrity_check(server_addr, num_objects):
    """Verify data integrity across memory-tier (hot) and SSD-tier (cold) paths.

    Uses raw cudaMalloc for IPC buffers (PyTorch's caching allocator is
    incompatible with cudaIpcGetMemHandle which requires base-of-allocation
    pointers).

    Strategy:
    - Populate num_objects with unique per-key byte patterns.
    - Immediately look them up (hot path, still in memory-tier) and verify.
    - Populate enough extra objects to evict the originals from the 256 MiB pool.
    - Wait for write-through, then look up the original keys (cold path, from SSD).
    - Verify the cold-path data matches the original pattern.
    """
    print(f"\n{'='*70}")
    print("Data Integrity Verification")
    print(f"{'='*70}")
    print(f"  Block size:     {BLOCK_SIZE // (1024*1024)} MiB")
    print(f"  Test objects:   {num_objects}")
    print()

    channel = grpc.insecure_channel(
        server_addr,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    base_key = random.randint(50_000_000, 90_000_000)
    pool_capacity = (256 * 1024 * 1024) // BLOCK_SIZE  # 64 for 4 MiB blocks
    passed = 0
    failed = 0

    # Allocate two raw CUDA buffers for IPC (one for populate, one for lookup).
    pop_ptr, pop_handle_bytes = _cuda_alloc(BLOCK_SIZE)
    look_ptr, look_handle_bytes = _cuda_alloc(BLOCK_SIZE)
    pop_ipc = dispatcher_pb2.IpcHandle(cuda_ipc_handle=pop_handle_bytes, size=BLOCK_SIZE)
    look_ipc = dispatcher_pb2.IpcHandle(cuda_ipc_handle=look_handle_bytes, size=BLOCK_SIZE)

    # --- Phase 1: Populate test objects with unique patterns ---
    print("  Phase 1: Populating test objects with unique patterns...", end="", flush=True)
    for i in range(num_objects):
        key = base_key + i
        pattern = _make_pattern(key, BLOCK_SIZE)
        _gpu_write(pop_ptr, pattern)
        _libcudart.cudaDeviceSynchronize()

        resp = stub.Populate(
            dispatcher_pb2.BatchPopulateRequest(
                entries=[dispatcher_pb2.PopulateEntry(key=key, ipc_handle=pop_ipc)]
            )
        )
        if not resp.results[0].success:
            print(f"\n  FAIL: populate key {key}: {resp.results[0].error_message}")
            failed += 1
    print(" done")

    # --- Phase 2: Hot-path verification (memory-tier) ---
    print("  Phase 2: Verifying hot-path reads (memory-tier)...", end="", flush=True)
    for i in range(num_objects):
        key = base_key + i

        resp = stub.Lookup(
            dispatcher_pb2.BatchLookupRequest(
                entries=[dispatcher_pb2.LookupEntry(key=key, ipc_handle=look_ipc)]
            )
        )
        if not resp.results[0].success:
            print(f"\n  FAIL: hot lookup key {key}: {resp.results[0].error_message}")
            failed += 1
            continue

        _libcudart.cudaDeviceSynchronize()
        actual = _gpu_read(look_ptr, BLOCK_SIZE)
        expected = _make_pattern(key, BLOCK_SIZE)
        if actual == expected:
            passed += 1
        else:
            first_bad = next(
                (j for j in range(len(actual)) if actual[j] != expected[j]), "?"
            )
            print(
                f"\n  FAIL: hot-path integrity mismatch at key {key}, "
                f"first bad byte offset {first_bad}"
            )
            failed += 1
    print(f" {passed} OK, {failed} FAIL")

    # --- Phase 3: Force eviction to SSD ---
    # Wait for write-through of ALL test objects before evicting them from DRAM.
    # Poll via Touch (succeeds only if key exists) as a health check, then wait
    # enough time for the background writer to flush everything to SSD.
    # The writer may be backed up from prior benchmark phases.
    print("  Phase 3: Waiting for write-through to complete...", end="", flush=True)
    deadline = time.time() + 15.0
    while time.time() < deadline:
        # Check if all test objects are still accessible (not lost to eviction).
        req = dispatcher_pb2.BatchCheckRequest(
            keys=[base_key + i for i in range(num_objects)]
        )
        resp = stub.Check(req)
        if all(r.exists for r in resp.results):
            time.sleep(0.5)
            break
        time.sleep(0.5)
    # Extra wait for the writer to finish SSD I/O after the last enqueue.
    time.sleep(3.0)
    print(" done")

    evict_base = base_key + num_objects + 1000
    evict_count = pool_capacity + 1
    print(
        f"  Phase 3: Evicting test objects from memory-tier "
        f"(populating {evict_count} eviction objects)...",
        end="", flush=True,
    )

    # Reuse pop_ptr for eviction fills (content irrelevant).
    batch_size = 10
    for batch_start in range(0, evict_count, batch_size):
        batch_end = min(batch_start + batch_size, evict_count)
        keys = [evict_base + j for j in range(batch_start, batch_end)]
        entries = [
            dispatcher_pb2.PopulateEntry(key=k, ipc_handle=pop_ipc)
            for k in keys
        ]
        resp = stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))
        err = [r for r in resp.results if not r.success]
        if err:
            print(f"\n  WARNING: eviction populate failed: {err[0].error_message}")
    print(" done")

    # --- Phase 4: Cold-path verification (SSD-tier) ---
    print("  Phase 4: Verifying cold-path reads (SSD-tier, promoted)...", end="", flush=True)
    cold_passed = 0
    cold_failed = 0

    for i in range(num_objects):
        key = base_key + i

        resp = stub.Lookup(
            dispatcher_pb2.BatchLookupRequest(
                entries=[dispatcher_pb2.LookupEntry(key=key, ipc_handle=look_ipc)]
            )
        )
        if not resp.results[0].success:
            print(f"\n  FAIL: cold lookup key {key}: {resp.results[0].error_message}")
            cold_failed += 1
            continue

        _libcudart.cudaDeviceSynchronize()
        actual = _gpu_read(look_ptr, BLOCK_SIZE)
        expected = _make_pattern(key, BLOCK_SIZE)
        if actual == expected:
            cold_passed += 1
        else:
            first_bad = next(
                (j for j in range(len(actual)) if actual[j] != expected[j]), "?"
            )
            # Check if data matches another key (detect extent reuse / wrong-key read)
            match_key = None
            for k2 in range(num_objects):
                other_key = base_key + k2
                if other_key == key:
                    continue
                if actual[:64] == _make_pattern(other_key, BLOCK_SIZE)[:64]:
                    match_key = other_key
                    break
            match_info = f", matches key {match_key}" if match_key else ""
            # Show first 16 bytes of actual vs expected
            act_hex = actual[:16].hex()
            exp_hex = expected[:16].hex()
            all_zero = all(b == 0 for b in actual[:1024])
            zero_info = " (ALL ZEROS in first 1KB)" if all_zero else ""
            print(
                f"\n  FAIL: cold-path integrity mismatch at key {key}, "
                f"first bad byte offset {first_bad}{match_info}{zero_info}"
                f"\n    expected[:16]={exp_hex}"
                f"\n    actual[:16]  ={act_hex}"
            )
            cold_failed += 1
    print(f" {cold_passed} OK, {cold_failed} FAIL")

    # --- Cleanup ---
    print("  Cleaning up...", end="", flush=True)
    all_keys = [base_key + i for i in range(num_objects)]
    all_keys += [evict_base + j for j in range(evict_count)]
    for batch_start in range(0, len(all_keys), batch_size):
        batch_end = min(batch_start + batch_size, len(all_keys))
        stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=all_keys[batch_start:batch_end]))
    print(" done")

    _cuda_free(pop_ptr)
    _cuda_free(look_ptr)

    # --- Summary ---
    total_passed = passed + cold_passed
    total_failed = failed + cold_failed
    print(f"\n  {'='*50}")
    print(f"  Integrity Results:")
    print(f"    Hot path (memory-tier):  {passed}/{passed+failed}")
    print(f"    Cold path (SSD-tier):    {cold_passed}/{cold_passed+cold_failed}")
    print(f"    Total:                   {total_passed}/{total_passed+total_failed}")
    print(f"  {'='*50}")

    if total_failed > 0:
        print(f"\n  INTEGRITY CHECK FAILED ({total_failed} errors)")
        return False
    else:
        print(f"\n  INTEGRITY CHECK PASSED")
        return True


def run_client(
    client_id,
    server_addr,
    num_objects,
    iterations,
    base_key,
    barrier,
    result,
):
    """Single client worker: populate objects, then measure hot and cold lookups."""

    channel = grpc.insecure_channel(
        server_addr,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    # Each client gets its own GPU buffer (4 MiB).
    populate_tensor = torch.zeros(
        BLOCK_SIZE // 4, dtype=torch.float32, device="cuda:0"
    )
    populate_handle_bytes = _get_cuda_ipc_handle(populate_tensor.data_ptr())
    populate_ipc = dispatcher_pb2.IpcHandle(
        cuda_ipc_handle=populate_handle_bytes, size=BLOCK_SIZE
    )

    lookup_tensor = torch.zeros(
        BLOCK_SIZE // 4, dtype=torch.float32, device="cuda:0"
    )
    lookup_handle_bytes = _get_cuda_ipc_handle(lookup_tensor.data_ptr())
    lookup_ipc = dispatcher_pb2.IpcHandle(
        cuda_ipc_handle=lookup_handle_bytes, size=BLOCK_SIZE
    )

    # Memory-tier pool is 256 MiB => can hold 64 x 4 MiB objects.
    # For cold-path testing we need objects evicted to SSD.
    # Strategy: populate enough objects to overflow the pool fraction this client owns,
    # so the earliest keys get evicted to SSD.
    pool_capacity = (256 * 1024 * 1024) // BLOCK_SIZE  # 64 objects total
    # We'll populate pool_capacity + cold objects so cold keys are evicted.
    cold_objects = num_objects * iterations
    total_objects = pool_capacity + cold_objects

    # --- Phase 1: Populate ---
    batch_size = 10
    barrier.wait()  # synchronize start across all clients

    t_pop_start = time.perf_counter()
    for batch_start in range(0, total_objects, batch_size):
        batch_end = min(batch_start + batch_size, total_objects)
        keys = [base_key + i for i in range(batch_start, batch_end)]
        entries = [
            dispatcher_pb2.PopulateEntry(key=k, ipc_handle=populate_ipc)
            for k in keys
        ]
        try:
            t0 = time.perf_counter()
            resp = stub.Populate(
                dispatcher_pb2.BatchPopulateRequest(entries=entries)
            )
            t1 = time.perf_counter()
            failed = [r for r in resp.results if not r.success]
            if failed:
                result.errors.append(
                    f"populate batch failed: {failed[0].error_message}"
                )
                return
            result.populate_latencies.append((t1 - t0) / len(keys))
        except grpc.RpcError as e:
            result.errors.append(f"populate RPC error: {e.details()}")
            return
    t_pop_end = time.perf_counter()

    # Wait for background write-through to flush to SSD.
    wt_wait = max(3.0, (cold_objects * BLOCK_SIZE) / (2 * 1024**3))
    time.sleep(wt_wait)

    # --- Phase 2: Hot lookups (memory-tier) ---
    # The last `num_objects` in the pool are still in DRAM.
    hot_keys = [
        base_key + cold_objects + pool_capacity - num_objects + i
        for i in range(num_objects)
    ]

    # Warmup
    entries = [
        dispatcher_pb2.LookupEntry(key=k, ipc_handle=lookup_ipc) for k in hot_keys
    ]
    try:
        stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
    except grpc.RpcError:
        pass

    barrier.wait()  # synchronize hot-lookup start

    for _ in range(iterations):
        entries = [
            dispatcher_pb2.LookupEntry(key=k, ipc_handle=lookup_ipc)
            for k in hot_keys
        ]
        try:
            t0 = time.perf_counter()
            resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
            t1 = time.perf_counter()
            failed = [r for r in resp.results if not r.success]
            if failed:
                result.errors.append(
                    f"hot lookup failed: {failed[0].error_message}"
                )
            result.hot_latencies.append((t1 - t0) / num_objects)
        except grpc.RpcError as e:
            result.errors.append(f"hot lookup RPC error: {e.details()}")

    # --- Phase 3: Cold lookups (SSD-tier) ---
    barrier.wait()  # synchronize cold-lookup start

    for iter_idx in range(iterations):
        cold_start = iter_idx * num_objects
        cold_keys = [base_key + cold_start + i for i in range(num_objects)]
        entries = [
            dispatcher_pb2.LookupEntry(key=k, ipc_handle=lookup_ipc)
            for k in cold_keys
        ]
        try:
            t0 = time.perf_counter()
            resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
            t1 = time.perf_counter()
            failed = [r for r in resp.results if not r.success]
            if failed:
                result.errors.append(
                    f"cold lookup iter {iter_idx} failed: {failed[0].error_message}"
                )
            result.cold_latencies.append((t1 - t0) / num_objects)
        except grpc.RpcError as e:
            result.errors.append(f"cold lookup RPC error: {e.details()}")

    # --- Cleanup ---
    for batch_start in range(0, total_objects, batch_size):
        batch_end = min(batch_start + batch_size, total_objects)
        keys = [base_key + i for i in range(batch_start, batch_end)]
        try:
            stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
        except grpc.RpcError:
            pass

    channel.close()


def print_stats(label, all_latencies, num_clients):
    """Print latency and throughput statistics."""
    if not all_latencies:
        print(f"  {label:<20} no data")
        return

    avg = statistics.mean(all_latencies)
    p50 = statistics.median(all_latencies)
    p99 = (
        sorted(all_latencies)[int(len(all_latencies) * 0.99)]
        if len(all_latencies) > 1
        else all_latencies[0]
    )
    mn = min(all_latencies)
    mx = max(all_latencies)

    # Throughput: each client does one object per latency measurement.
    # Aggregate throughput = num_clients * (BLOCK_SIZE / avg_latency)
    tp_per_client = BLOCK_SIZE / avg if avg > 0 else 0
    tp_aggregate = tp_per_client * num_clients

    print(
        f"  {label:<20} "
        f"avg={avg*1e6:>9.1f} us  "
        f"p50={p50*1e6:>9.1f} us  "
        f"p99={p99*1e6:>9.1f} us  "
        f"min={mn*1e6:>9.1f} us  "
        f"max={mx*1e6:>9.1f} us"
    )
    print(
        f"  {'':20} "
        f"per-client={tp_per_client/1e9:>6.2f} GB/s  "
        f"aggregate={tp_aggregate/1e9:>6.2f} GB/s"
    )


def main():
    parser = argparse.ArgumentParser(
        description="Certus multi-client throughput/latency benchmark (4 MiB blocks)"
    )
    parser.add_argument(
        "--server",
        default="localhost:50051",
        help="Server address (default: localhost:50051)",
    )
    parser.add_argument(
        "--clients",
        type=int,
        default=1,
        help="Number of concurrent client threads (default: 1)",
    )
    parser.add_argument(
        "--num-objects",
        type=int,
        default=16,
        help="Objects per lookup batch per client (default: 16)",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=10,
        help="Lookup iterations per phase (default: 10)",
    )
    parser.add_argument(
        "--verify-integrity",
        action="store_true",
        help="Run data integrity check (populate with known patterns, verify hot and cold reads)",
    )
    parser.add_argument(
        "--integrity-objects",
        type=int,
        default=16,
        help="Number of objects to verify in integrity check (default: 16)",
    )
    args = parser.parse_args()

    num_clients = args.clients
    num_objects = args.num_objects
    iterations = args.iterations

    pool_capacity = (256 * 1024 * 1024) // BLOCK_SIZE
    cold_per_client = num_objects * iterations
    total_per_client = pool_capacity + cold_per_client

    print(f"{'='*70}")
    print(f"Certus Multi-Client Benchmark")
    print(f"{'='*70}")
    print(f"  Server:            {args.server}")
    print(f"  Clients:           {num_clients}")
    print(f"  Block size:        {BLOCK_SIZE // (1024*1024)} MiB")
    print(f"  Objects/batch:     {num_objects}")
    print(f"  Iterations:        {iterations}")
    print(f"  Pool capacity:     {pool_capacity} objects (256 MiB)")
    print(f"  Total per client:  {total_per_client} objects")
    print(f"  Cold per client:   {cold_per_client} objects")
    print()

    # Each client gets a non-overlapping key range.
    key_range_size = 10_000_000
    base_keys = [
        random.randint(1_000_000, 100_000_000) + i * key_range_size
        for i in range(num_clients)
    ]

    barrier = threading.Barrier(num_clients)
    results = [ClientResult(i) for i in range(num_clients)]
    threads = []

    print(f"  Starting {num_clients} client(s)...")
    t_total_start = time.perf_counter()

    for i in range(num_clients):
        t = threading.Thread(
            target=run_client,
            args=(
                i,
                args.server,
                num_objects,
                iterations,
                base_keys[i],
                barrier,
                results[i],
            ),
            daemon=True,
        )
        threads.append(t)
        t.start()

    for t in threads:
        t.join()

    t_total_end = time.perf_counter()

    # Collect errors
    all_errors = []
    for r in results:
        all_errors.extend(r.errors)

    if all_errors:
        print(f"\n  ERRORS ({len(all_errors)}):")
        for e in all_errors[:10]:
            print(f"    - {e}")
        if len(all_errors) > 10:
            print(f"    ... and {len(all_errors) - 10} more")

    # Aggregate latencies across all clients
    all_populate = []
    all_hot = []
    all_cold = []
    for r in results:
        all_populate.extend(r.populate_latencies)
        all_hot.extend(r.hot_latencies)
        all_cold.extend(r.cold_latencies)

    print(f"\n{'='*70}")
    print(f"Results ({num_clients} client(s), {BLOCK_SIZE//(1024*1024)} MiB blocks)")
    print(f"{'='*70}")
    print()
    print("  Latency per object (all clients combined):")
    print()
    print_stats("Populate", all_populate, num_clients)
    print()
    print_stats("Lookup (hot)", all_hot, num_clients)
    print()
    print_stats("Lookup (cold)", all_cold, num_clients)
    print()

    if all_hot and all_cold:
        hot_avg = statistics.mean(all_hot)
        cold_avg = statistics.mean(all_cold)
        if hot_avg > 0:
            print(
                f"  Cold/Hot ratio:    {cold_avg/hot_avg:.1f}x latency"
            )

    print(f"\n  Total wall time:   {t_total_end - t_total_start:.2f}s")

    # Per-client summary
    print(f"\n  Per-client breakdown:")
    print(
        f"  {'Client':<8} {'Hot avg (us)':<14} {'Cold avg (us)':<14} {'Errors':<8}"
    )
    print(f"  {'-'*44}")
    for r in results:
        hot_avg = statistics.mean(r.hot_latencies) * 1e6 if r.hot_latencies else 0
        cold_avg = (
            statistics.mean(r.cold_latencies) * 1e6 if r.cold_latencies else 0
        )
        print(
            f"  {r.client_id:<8} {hot_avg:<14.1f} {cold_avg:<14.1f} {len(r.errors):<8}"
        )

    print()

    # --- Integrity verification (optional) ---
    integrity_ok = True
    if args.verify_integrity:
        integrity_ok = run_integrity_check(args.server, args.integrity_objects)

    sys.exit(1 if (all_errors or not integrity_ok) else 0)


if __name__ == "__main__":
    main()
