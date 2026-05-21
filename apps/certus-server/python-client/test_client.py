#!/usr/bin/env python3
"""Test client for the Certus gRPC Dispatcher server.

Exercises all batch operations: populate, check, lookup, remove.
Validates per-entry error handling and duplicate-key rejection.
"""

import argparse
import random
import sys
import os

# Add current directory to path for generated stubs
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import ctypes

import grpc
import torch
import dispatcher_pb2
import dispatcher_pb2_grpc

assert torch.cuda.is_available(), "CUDA GPU required for test client"

_libcudart = ctypes.CDLL("libcudart.so")
_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]


class TestResult:
    def __init__(self):
        self.passed = 0
        self.failed = 0
        self.errors = []

    def ok(self, name):
        self.passed += 1
        print(f"[PASS] {name}")

    def fail(self, name, reason):
        self.failed += 1
        self.errors.append((name, reason))
        print(f"[FAIL] {name}: {reason}")

    def summary(self):
        total = self.passed + self.failed
        print(f"\n{self.passed}/{total} tests passed.")
        if self.errors:
            print("Failures:")
            for name, reason in self.errors:
                print(f"  - {name}: {reason}")
        return self.failed == 0


# Keep GPU tensors alive for the duration of the test so IPC handles remain valid
_gpu_buffers = {}


def _get_cuda_ipc_handle(data_ptr):
    """Call cudaIpcGetMemHandle to get the 64-byte opaque IPC handle for a device pointer."""
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), data_ptr)
    if err != 0:
        raise RuntimeError(f"cudaIpcGetMemHandle failed with error {err}")
    return bytes(handle_buf)


def make_ipc_handle(key, size=4096):
    """Allocate GPU memory via PyTorch and return an IpcHandle with a CUDA IPC handle."""
    num_elements = size // 4  # float32 = 4 bytes
    tensor = torch.zeros(num_elements, dtype=torch.float32, device="cuda:0")
    _gpu_buffers[key] = tensor

    handle_bytes = _get_cuda_ipc_handle(tensor.data_ptr())
    return dispatcher_pb2.IpcHandle(cuda_ipc_handle=handle_bytes, size=size)


def cleanup_keys(stub, keys):
    """Best-effort removal of keys to ensure a clean slate."""
    try:
        req = dispatcher_pb2.BatchRemoveRequest(keys=keys)
        stub.Remove(req)
    except grpc.RpcError:
        pass


def test_batch_populate(stub, results, base_key):
    """US2: Batch populate 10 entries."""
    keys = [base_key + i for i in range(10)]
    cleanup_keys(stub, keys)

    entries = [
        dispatcher_pb2.PopulateEntry(key=k, ipc_handle=make_ipc_handle(k))
        for k in keys
    ]
    req = dispatcher_pb2.BatchPopulateRequest(entries=entries)
    resp = stub.Populate(req)

    if len(resp.results) != 10:
        results.fail("Batch populate: 10 entries", f"got {len(resp.results)} results")
        return

    all_ok = all(r.success for r in resp.results)
    if all_ok:
        results.ok("Batch populate: 10 entries")
    else:
        failed = [r for r in resp.results if not r.success]
        results.fail(
            "Batch populate: 10 entries",
            f"{len(failed)} entries failed: {failed[0].error_message}",
        )


def test_batch_check(stub, results, base_key):
    """US3: Check existence of populated and non-existent keys."""
    populated_keys = [base_key + i for i in range(10)]
    missing_keys = [base_key + 1000 + i for i in range(5)]
    keys = populated_keys + missing_keys
    req = dispatcher_pb2.BatchCheckRequest(keys=keys)
    resp = stub.Check(req)

    if len(resp.results) != 15:
        results.fail("Batch check: all 10 exist", f"got {len(resp.results)} results")
        return

    existing = [r for r in resp.results if r.key in populated_keys]
    missing = [r for r in resp.results if r.key in missing_keys]

    all_exist = all(r.exists for r in existing)
    none_exist = not any(r.exists for r in missing)

    if all_exist and none_exist:
        results.ok("Batch check: all 10 exist")
    else:
        results.fail(
            "Batch check: all 10 exist",
            f"existing={all_exist}, missing_correct={none_exist}",
        )


def test_batch_lookup(stub, results, base_key):
    """US3: Lookup populated entries."""
    keys = [base_key + i for i in range(10)]
    entries = [
        dispatcher_pb2.LookupEntry(key=k, ipc_handle=make_ipc_handle(k, 4096))
        for k in keys
    ]
    req = dispatcher_pb2.BatchLookupRequest(entries=entries)
    resp = stub.Lookup(req)

    if len(resp.results) != 10:
        results.fail(
            "Batch lookup: 10 entries retrieved", f"got {len(resp.results)} results"
        )
        return

    all_ok = all(r.success for r in resp.results)
    if all_ok:
        results.ok("Batch lookup: 10 entries retrieved")
    else:
        failed = [r for r in resp.results if not r.success]
        results.fail(
            "Batch lookup: 10 entries retrieved",
            f"{len(failed)} entries failed: {failed[0].error_message}",
        )


def test_batch_remove(stub, results, base_key):
    """US4: Remove all populated entries."""
    import time
    time.sleep(0.5)  # allow background writer to complete staging-to-storage conversion
    keys = [base_key + i for i in range(10)]
    req = dispatcher_pb2.BatchRemoveRequest(keys=keys)
    resp = stub.Remove(req)

    if len(resp.results) != 10:
        results.fail("Batch remove: 10 entries removed", f"got {len(resp.results)} results")
        return

    all_ok = all(r.success for r in resp.results)
    if all_ok:
        results.ok("Batch remove: 10 entries removed")
    else:
        failed = [r for r in resp.results if not r.success]
        results.fail(
            "Batch remove: 10 entries removed",
            f"{len(failed)} entries failed: {failed[0].error_message}",
        )


def test_check_after_remove(stub, results, base_key):
    """US4: Verify entries are gone after removal."""
    keys = [base_key + i for i in range(10)]
    req = dispatcher_pb2.BatchCheckRequest(keys=keys)
    resp = stub.Check(req)

    none_exist = not any(r.exists for r in resp.results)
    if none_exist:
        results.ok("Check after remove: 0 exist")
    else:
        still_exist = sum(1 for r in resp.results if r.exists)
        results.fail("Check after remove: 0 exist", f"{still_exist} still exist")


def test_duplicate_key_rejection(stub, results):
    """FR-015: Batch with duplicate keys is rejected entirely."""
    entries = [
        dispatcher_pb2.PopulateEntry(key=42, ipc_handle=make_ipc_handle(42)),
        dispatcher_pb2.PopulateEntry(key=43, ipc_handle=make_ipc_handle(43)),
        dispatcher_pb2.PopulateEntry(key=42, ipc_handle=make_ipc_handle(42)),  # duplicate
    ]
    req = dispatcher_pb2.BatchPopulateRequest(entries=entries)

    try:
        stub.Populate(req)
        results.fail("Duplicate key rejection", "expected error but got success")
    except grpc.RpcError as e:
        if e.code() == grpc.StatusCode.INVALID_ARGUMENT and "duplicate" in e.details().lower():
            results.ok("Duplicate key rejection")
        else:
            results.fail(
                "Duplicate key rejection",
                f"wrong error: code={e.code()}, details={e.details()}",
            )


def test_batch_touch(stub, results, base_key):
    """Touch populated entries to refresh their eviction timestamps."""
    keys = [base_key + i for i in range(10)]
    req = dispatcher_pb2.BatchTouchRequest(keys=keys)
    resp = stub.Touch(req)

    if len(resp.results) != 10:
        results.fail("Batch touch: 10 entries", f"got {len(resp.results)} results")
        return

    all_ok = all(r.success for r in resp.results)
    if all_ok:
        results.ok("Batch touch: 10 entries")
    else:
        failed = [r for r in resp.results if not r.success]
        results.fail(
            "Batch touch: 10 entries",
            f"{len(failed)} entries failed: {failed[0].error_message}",
        )


def test_touch_nonexistent(stub, results):
    """Touch on non-existent key returns KeyNotFound."""
    req = dispatcher_pb2.BatchTouchRequest(keys=[9998])
    resp = stub.Touch(req)

    if len(resp.results) == 1 and not resp.results[0].success:
        if resp.results[0].error_code == dispatcher_pb2.ERROR_CODE_KEY_NOT_FOUND:
            results.ok("Touch non-existent key handling")
            return

    results.fail("Touch non-existent key handling", "expected KeyNotFound error")


def test_nonexistent_key_handling(stub, results):
    """Edge case: operations on keys that don't exist."""
    # Remove non-existent key
    req = dispatcher_pb2.BatchRemoveRequest(keys=[9999])
    resp = stub.Remove(req)

    if len(resp.results) == 1 and not resp.results[0].success:
        if resp.results[0].error_code == dispatcher_pb2.ERROR_CODE_KEY_NOT_FOUND:
            results.ok("Non-existent key handling")
            return

    results.fail("Non-existent key handling", "expected KeyNotFound error")


def test_large_batch(stub, results, base_key, count=1000):
    """SC-002: Large batch operations complete without timeout."""
    keys = [base_key + i for i in range(count)]
    cleanup_keys(stub, keys)

    # Populate
    entries = [
        dispatcher_pb2.PopulateEntry(key=k, ipc_handle=make_ipc_handle(k))
        for k in keys
    ]
    req = dispatcher_pb2.BatchPopulateRequest(entries=entries)
    resp = stub.Populate(req)
    pop_ok = all(r.success for r in resp.results)

    # Check
    req = dispatcher_pb2.BatchCheckRequest(keys=keys)
    resp = stub.Check(req)
    check_ok = all(r.exists for r in resp.results)

    # Remove (allow background writer to finish)
    import time
    time.sleep(1.0)
    req = dispatcher_pb2.BatchRemoveRequest(keys=keys)
    resp = stub.Remove(req)
    rm_ok = all(r.success for r in resp.results)

    if pop_ok and check_ok and rm_ok:
        results.ok(f"Large batch ({count} entries): populate/check/remove")
    else:
        results.fail(
            f"Large batch ({count} entries): populate/check/remove",
            f"populate={pop_ok}, check={check_ok}, remove={rm_ok}",
        )


def bench_lookup_latency(stub, object_size=65536, num_objects=100, iterations=10):
    """Benchmark lookup latency for memory-tier vs SSD-tier objects.

    Strategy:
    - The memory-tier pool is 256 MiB. With `object_size` bytes per object,
      pool_capacity = 256 MiB / object_size objects can fit in DRAM.
    - We populate `pool_capacity + num_objects` objects so that the first
      `num_objects` are evicted to SSD (write-through completes, then evicted
      by LRU pressure from subsequent inserts).
    - We then measure lookup latency for:
      (a) "hot" objects still in memory-tier (the last `num_objects` populated)
      (b) "cold" objects evicted to SSD (the first `num_objects` populated)

    To avoid cudaIpcOpenMemHandle/Close overhead dominating measurements,
    we pre-allocate a single GPU buffer and reuse its IPC handle for all lookups.
    """
    import time

    pool_size = 256 * 1024 * 1024  # 256 MiB
    pool_capacity = pool_size // object_size
    total_objects = pool_capacity + num_objects
    base_key = random.randint(1_000_000, 8_000_000)

    print(f"\n{'='*60}")
    print(f"Lookup Latency Benchmark")
    print(f"{'='*60}")
    print(f"  Object size:      {object_size // 1024} KiB")
    print(f"  Pool capacity:    {pool_capacity} objects ({pool_size // (1024*1024)} MiB)")
    print(f"  Total to populate: {total_objects} (to force {num_objects} evictions)")
    print(f"  Lookup iterations: {iterations}")
    print()

    # Pre-allocate a single reusable GPU buffer for lookup targets.
    # This avoids per-entry cudaIpcOpenMemHandle/Close overhead on the server.
    lookup_tensor = torch.zeros(object_size // 4, dtype=torch.float32, device="cuda:0")
    _gpu_buffers["_bench_lookup"] = lookup_tensor
    lookup_handle_bytes = _get_cuda_ipc_handle(lookup_tensor.data_ptr())
    lookup_ipc = dispatcher_pb2.IpcHandle(cuda_ipc_handle=lookup_handle_bytes, size=object_size)

    # Phase 1: Populate all objects (first `num_objects` will be evicted by LRU)
    print(f"  Populating {total_objects} objects...", end="", flush=True)
    batch_size = 50
    t0 = time.perf_counter()
    for batch_start in range(0, total_objects, batch_size):
        batch_end = min(batch_start + batch_size, total_objects)
        keys = [base_key + i for i in range(batch_start, batch_end)]
        entries = [
            dispatcher_pb2.PopulateEntry(key=k, ipc_handle=make_ipc_handle(k, object_size))
            for k in keys
        ]
        resp = stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))
        failed = [r for r in resp.results if not r.success]
        if failed:
            print(f"\n  ERROR: populate failed for {len(failed)} entries: {failed[0].error_message}")
            return
    populate_time = time.perf_counter() - t0
    print(f" done ({populate_time:.2f}s, {total_objects/populate_time:.0f} obj/s)")

    # Wait for background write-through to complete for evicted objects
    print("  Waiting for write-through to complete...", end="", flush=True)
    time.sleep(3.0)
    print(" done")

    # Phase 2: Measure memory-tier (hot) lookups — last `num_objects` populated
    hot_keys = [base_key + total_objects - num_objects + i for i in range(num_objects)]
    hot_latencies = []

    print(f"  Benchmarking memory-tier lookups ({num_objects} objects x {iterations} iters)...", end="", flush=True)
    for _ in range(iterations):
        entries = [
            dispatcher_pb2.LookupEntry(key=k, ipc_handle=lookup_ipc)
            for k in hot_keys
        ]
        t_start = time.perf_counter()
        resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
        t_end = time.perf_counter()
        failed = [r for r in resp.results if not r.success]
        if failed:
            print(f"\n  WARNING: {len(failed)} hot lookups failed: {failed[0].error_message}")
        hot_latencies.append((t_end - t_start) / num_objects)
    print(" done")

    # Phase 3: Measure SSD-tier (cold) lookups — first `num_objects` populated (evicted)
    cold_keys = [base_key + i for i in range(num_objects)]
    cold_latencies = []

    print(f"  Benchmarking SSD-tier lookups ({num_objects} objects x {iterations} iters)...", end="", flush=True)
    for _ in range(iterations):
        entries = [
            dispatcher_pb2.LookupEntry(key=k, ipc_handle=lookup_ipc)
            for k in cold_keys
        ]
        t_start = time.perf_counter()
        resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=entries))
        t_end = time.perf_counter()
        failed = [r for r in resp.results if not r.success]
        if failed:
            print(f"\n  WARNING: {len(failed)} cold lookups failed: {failed[0].error_message}")
        cold_latencies.append((t_end - t_start) / num_objects)
    print(" done")

    # Results
    hot_avg = sum(hot_latencies) / len(hot_latencies)
    hot_min = min(hot_latencies)
    hot_max = max(hot_latencies)
    cold_avg = sum(cold_latencies) / len(cold_latencies)
    cold_min = min(cold_latencies)
    cold_max = max(cold_latencies)

    # Throughput: GB/s = object_size / latency_per_object
    hot_tp_avg = (object_size / hot_avg) / 1e9 if hot_avg > 0 else 0
    hot_tp_max = (object_size / hot_min) / 1e9 if hot_min > 0 else 0
    cold_tp_avg = (object_size / cold_avg) / 1e9 if cold_avg > 0 else 0
    cold_tp_max = (object_size / cold_min) / 1e9 if cold_min > 0 else 0

    print(f"\n  {'Tier':<15} {'Avg (us/obj)':<14} {'Min (us/obj)':<14} {'Max (us/obj)':<14} {'Avg (GB/s)':<12} {'Peak (GB/s)':<12}")
    print(f"  {'-'*80}")
    print(f"  {'Memory-tier':<15} {hot_avg*1e6:<14.1f} {hot_min*1e6:<14.1f} {hot_max*1e6:<14.1f} {hot_tp_avg:<12.2f} {hot_tp_max:<12.2f}")
    print(f"  {'SSD-tier':<15} {cold_avg*1e6:<14.1f} {cold_min*1e6:<14.1f} {cold_max*1e6:<14.1f} {cold_tp_avg:<12.2f} {cold_tp_max:<12.2f}")
    if hot_avg > 0:
        print(f"\n  SSD/Memory-tier ratio: {cold_avg/hot_avg:.1f}x latency, {hot_tp_avg/cold_tp_avg:.1f}x throughput")
    print()

    # Cleanup
    print("  Cleaning up...", end="", flush=True)
    for batch_start in range(0, total_objects, batch_size):
        batch_end = min(batch_start + batch_size, total_objects)
        keys = [base_key + i for i in range(batch_start, batch_end)]
        stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print(" done")


def main():
    parser = argparse.ArgumentParser(description="Certus gRPC dispatcher test client")
    parser.add_argument(
        "--server", default="localhost:50051", help="Server address (default: localhost:50051)"
    )
    parser.add_argument(
        "--skip-large-batch", action="store_true", help="Skip the 1000-entry large batch test"
    )
    parser.add_argument(
        "--bench", action="store_true", help="Run lookup latency benchmark (memory-tier vs SSD)"
    )
    parser.add_argument(
        "--bench-only", action="store_true", help="Skip functional tests, run only the benchmark"
    )
    parser.add_argument(
        "--bench-object-size", type=int, default=65536,
        help="Object size in bytes for benchmark (default: 65536 = 64 KiB)"
    )
    parser.add_argument(
        "--bench-num-objects", type=int, default=100,
        help="Number of objects per tier to benchmark (default: 100)"
    )
    parser.add_argument(
        "--bench-iterations", type=int, default=10,
        help="Number of lookup iterations per tier (default: 10)"
    )
    args = parser.parse_args()

    print(f"Testing certus-server gRPC dispatcher at {args.server}...")

    channel = grpc.insecure_channel(args.server)
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    if args.bench_only:
        bench_lookup_latency(
            stub,
            object_size=args.bench_object_size,
            num_objects=args.bench_num_objects,
            iterations=args.bench_iterations,
        )
        sys.exit(0)

    results = TestResult()

    # Use random base key to avoid collisions with prior runs
    base_key = random.randint(100000, 900000)
    large_base_key = base_key + 10000

    # Core lifecycle tests
    test_batch_populate(stub, results, base_key)
    test_batch_check(stub, results, base_key)
    test_batch_touch(stub, results, base_key)
    test_batch_lookup(stub, results, base_key)
    test_batch_remove(stub, results, base_key)
    test_check_after_remove(stub, results, base_key)

    # Error handling tests
    test_duplicate_key_rejection(stub, results)
    test_nonexistent_key_handling(stub, results)
    test_touch_nonexistent(stub, results)

    # Scale test
    if not args.skip_large_batch:
        test_large_batch(stub, results, large_base_key)

    if not results.summary():
        sys.exit(1)

    # Benchmark (only if correctness tests pass)
    if args.bench:
        bench_lookup_latency(
            stub,
            object_size=args.bench_object_size,
            num_objects=args.bench_num_objects,
            iterations=args.bench_iterations,
        )

    print("All tests passed.")
    sys.exit(0)


if __name__ == "__main__":
    main()
