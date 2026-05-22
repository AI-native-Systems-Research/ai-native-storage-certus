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
    sys.exit(1 if all_errors else 0)


if __name__ == "__main__":
    main()
