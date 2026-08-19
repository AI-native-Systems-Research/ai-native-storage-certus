#!/usr/bin/env python3
"""Multi-client throughput and latency benchmark for the Certus shmq Dispatcher (v2).

Both hot and cold lookups are pipelined across a ThreadPoolExecutor of
--pipeline-depth workers, keeping that many requests in flight simultaneously,
like iperf -P. The shmq Ring is one-request-in-flight per channel, so each
worker holds its own sticky channel; this saturates the server's GPU PCIe link
which sequential requests cannot fill alone. Use --sequential-hot for
per-request latency measurement on the hot path.

Usage:
    python certus-api-bench_v2.py --clients 1 --num-objects 64 --pipeline-depth 4
"""

import argparse
import ctypes
import os
import random
import statistics
import sys
import threading
import time
from concurrent import futures as _futures

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import torch
from certus_shmq_helpers import RingError, add_shm_arg, connect, single_region

assert torch.cuda.is_available(), "CUDA GPU required"

BLOCK_SIZE = 4 * 1024 * 1024  # 4 MiB (default, overridden by --block-size)
# TODO: query server for actual pool size instead of requiring manual --memory-tier-size.
# For multi-client runs, set this to HALF the server's --memory-tier-size so that
# total objects across all clients fits within the real pool without cross-client eviction.
MEMORY_TIER_SIZE = 2 * 1024 * 1024 * 1024  # 2 GiB default (half of typical 4G server)


def parse_size(s):
    """Parse a human-readable size string (e.g. '128K', '4M', '2G') into bytes."""
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
        raise argparse.ArgumentTypeError(f"invalid size number: '{num_str}'")
    if value <= 0:
        raise argparse.ArgumentTypeError(f"size must be positive, got '{s}'")
    return value * multiplier

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
        self.populate_start = 0.0
        self.populate_end = 0.0
        self.populate_objects = 0
        self.hot_start = 0.0
        self.hot_end = 0.0
        self.cold_start = 0.0
        self.cold_end = 0.0
        self.hot_objects = 0
        self.cold_objects = 0


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


def run_integrity_check(ring, num_objects):
    """Verify data integrity across memory-tier (hot) and SSD-tier (cold) paths.

    Uses raw cudaMalloc for IPC buffers (PyTorch's caching allocator is
    incompatible with cudaIpcGetMemHandle which requires base-of-allocation
    pointers).

    Strategy:
    - Populate num_objects with unique per-key byte patterns.
    - Immediately look them up (hot path, still in memory-tier) and verify.
    - Populate enough extra objects to evict the originals from the memory-tier pool.
    - Wait for write-through, then look up the original keys (cold path, from SSD).
    - Verify the cold-path data matches the original pattern.
    """
    print(f"\n{'='*70}")
    print("Data Integrity Verification")
    print(f"{'='*70}")
    print(f"  Block size:     {BLOCK_SIZE // (1024*1024)} MiB")
    print(f"  Test objects:   {num_objects}")
    print()

    base_key = random.randint(50_000_000, 90_000_000)
    pool_capacity = MEMORY_TIER_SIZE // BLOCK_SIZE
    passed = 0
    failed = 0

    # Allocate two raw CUDA buffers for IPC (one for populate, one for lookup).
    # The integrity check does not call cudaSetDevice, so buffers live on GPU 0.
    pop_ptr, pop_handle_bytes = _cuda_alloc(BLOCK_SIZE)
    look_ptr, look_handle_bytes = _cuda_alloc(BLOCK_SIZE)
    pop_region = single_region(pop_handle_bytes, 0, BLOCK_SIZE)
    look_region = single_region(look_handle_bytes, 0, BLOCK_SIZE)

    # --- Phase 1: Populate test objects with unique patterns ---
    print("  Phase 1: Populating test objects with unique patterns...", end="", flush=True)
    for i in range(num_objects):
        key = base_key + i
        pattern = _make_pattern(key, BLOCK_SIZE)
        _gpu_write(pop_ptr, pattern)
        _libcudart.cudaDeviceSynchronize()

        oks = ring.populate([(key, [pop_region])])
        if not oks[0]:
            print(f"\n  FAIL: populate key {key}")
            failed += 1
    print(" done")

    # --- Phase 2: Hot-path verification (memory-tier) ---
    print("  Phase 2: Verifying hot-path reads (memory-tier)...", end="", flush=True)
    for i in range(num_objects):
        key = base_key + i

        oks = ring.lookup([(key, [look_region])])
        if not oks[0]:
            print(f"\n  FAIL: hot lookup key {key}")
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
        oks = ring.check([base_key + i for i in range(num_objects)])
        if all(oks):
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
        entries = [(k, [pop_region]) for k in keys]
        oks = ring.populate(entries)
        err = [ok for ok in oks if not ok]
        if err:
            print(f"\n  WARNING: eviction populate failed: {len(err)} keys")
    print(" done")

    # --- Phase 4: Cold-path verification (SSD-tier) ---
    print("  Phase 4: Verifying cold-path reads (SSD-tier, promoted)...", end="", flush=True)
    cold_passed = 0
    cold_failed = 0

    for i in range(num_objects):
        key = base_key + i

        try:
            oks = ring.lookup([(key, [look_region])])
        except RingError as e:
            print(f"\n  FAIL: cold lookup key {key}: ring error: {str(e)}")
            cold_failed += 1
            continue
        if not oks[0]:
            print(f"\n  FAIL: cold lookup key {key}")
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
            match_key = None
            for k2 in range(num_objects):
                other_key = base_key + k2
                if other_key == key:
                    continue
                if actual[:64] == _make_pattern(other_key, BLOCK_SIZE)[:64]:
                    match_key = other_key
                    break
            match_info = f", matches key {match_key}" if match_key else ""
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
        ring.remove(all_keys[batch_start:batch_end])
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
    ring,
    num_objects,
    iterations,
    base_key,
    num_clients,
    batch_size,
    barrier,
    result,
    gpu_id=0,
    pipeline_depth=4,
    writes_settle=30.0,
    sequential_hot=False,
):
    """Single client worker: populate objects, then measure hot and cold lookups.

    ``ring`` is the shared shmq client created in main; this worker (and its
    pipeline ThreadPoolExecutor workers) each auto-claim a distinct sticky
    channel on first use.
    """

    # Pin this client to its assigned GPU.
    _libcudart.cudaSetDevice(gpu_id)
    cuda_device = f"cuda:{gpu_id}"

    # Each client gets its own GPU buffer (4 MiB) filled with unique data.
    torch.manual_seed(base_key)
    populate_tensor = torch.randint(
        0, 256, (BLOCK_SIZE // 4,), dtype=torch.float32, device=cuda_device
    )
    populate_handle_bytes = _get_cuda_ipc_handle(populate_tensor.data_ptr())
    populate_region = single_region(populate_handle_bytes, gpu_id, BLOCK_SIZE)

    # Separate GPU buffers for lookups — one per key in the batch.
    # A shared IPC handle allows the server to coalesce DMA writes to the
    # same destination, producing artificially low latencies. Distinct buffers
    # force a real H2D transfer per object.
    hot_lookup_ptrs = []
    hot_lookup_handles = []
    for _ in range(num_objects):
        ptr, handle_bytes = _cuda_alloc(BLOCK_SIZE)
        hot_lookup_ptrs.append(ptr)
        hot_lookup_handles.append(handle_bytes)

    cold_lookup_ptrs = []
    cold_lookup_handles = []
    for _ in range(num_objects):
        ptr, handle_bytes = _cuda_alloc(BLOCK_SIZE)
        cold_lookup_ptrs.append(ptr)
        cold_lookup_handles.append(handle_bytes)

    # Memory-tier pool can hold MEMORY_TIER_SIZE / BLOCK_SIZE objects total.
    # With num_clients concurrent clients each gets a fair share of the pool.
    # Strategy: populate enough objects to overflow this client's share,
    # so the earliest keys get evicted to SSD.
    pool_capacity = MEMORY_TIER_SIZE // BLOCK_SIZE
    client_pool_share = max(1, pool_capacity // num_clients)
    cold_objects = num_objects * iterations
    total_objects = client_pool_share + cold_objects

    # --- Phase 1: Populate ---
    barrier.wait()  # synchronize start across all clients

    t_pop_start = time.perf_counter()
    for batch_start in range(0, total_objects, batch_size):
        batch_end = min(batch_start + batch_size, total_objects)
        keys = [base_key + i for i in range(batch_start, batch_end)]
        # Write unique data per batch (seeded by first key in batch)
        torch.manual_seed(keys[0])
        populate_tensor.copy_(
            torch.randint(0, 256, (BLOCK_SIZE // 4,), dtype=torch.float32, device=cuda_device)
        )
        entries = [(k, [populate_region]) for k in keys]
        try:
            t0 = time.perf_counter()
            oks = ring.populate(entries)
            t1 = time.perf_counter()
            failed = [ok for ok in oks if not ok]
            if failed:
                result.errors.append(
                    f"populate batch failed: {len(failed)} keys"
                )
                break
            result.populate_latencies.append((t1 - t0) / len(keys))
        except RingError as e:
            result.errors.append(f"populate error: {str(e)}")
            break
    t_pop_end = time.perf_counter()
    result.populate_start = t_pop_start
    result.populate_end = t_pop_end
    result.populate_objects = total_objects

    # All clients synchronously flush background write-through, then barrier
    # to ensure every client's writes are on SSD before proceeding.
    try:
        ring.flush_to_ssd()
    except RingError:
        pass
    barrier.wait()
    if writes_settle > 0:
        time.sleep(writes_settle)
    barrier.wait()

    # --- Phase 2: Hot lookups (memory-tier) ---
    # The last `num_objects` in this client's pool share are still in DRAM.
    hot_keys = [
        base_key + cold_objects + client_pool_share - num_objects + i
        for i in range(num_objects)
    ]

    # Warmup
    warmup_entries = [
        (k, [single_region(hot_lookup_handles[i], gpu_id, BLOCK_SIZE)])
        for i, k in enumerate(hot_keys)
    ]
    try:
        ring.lookup(warmup_entries)
    except RingError:
        pass

    barrier.wait()  # synchronize hot-lookup start
    result.hot_start = time.perf_counter()

    if sequential_hot:
        for _ in range(iterations):
            entries = [
                (k, [single_region(hot_lookup_handles[i], gpu_id, BLOCK_SIZE)])
                for i, k in enumerate(hot_keys)
            ]
            try:
                t0 = time.perf_counter()
                oks = ring.lookup(entries)
                _libcudart.cudaDeviceSynchronize()
                t1 = time.perf_counter()
                failed = [ok for ok in oks if not ok]
                if failed:
                    result.errors.append(
                        f"hot lookup failed: {len(failed)} keys"
                    )
                result.hot_latencies.append((t1 - t0) / num_objects)
            except RingError as e:
                result.errors.append(f"hot lookup error: {str(e)}")
    else:
        # Pipeline pipeline_depth lookups concurrently: each worker thread claims
        # its own shmq channel and does one blocking ring.lookup, timing inside.
        def do_hot_iter(_iter_idx):
            entries = [
                (k, [single_region(hot_lookup_handles[i], gpu_id, BLOCK_SIZE)])
                for i, k in enumerate(hot_keys)
            ]
            t0 = time.perf_counter()
            try:
                oks = ring.lookup(entries)
                _libcudart.cudaDeviceSynchronize()
                t1 = time.perf_counter()
                failed = sum(1 for ok in oks if not ok)
                return failed, (t1 - t0) / num_objects, None
            except RingError as e:
                return num_objects, None, str(e)

        with _futures.ThreadPoolExecutor(max_workers=pipeline_depth) as ex:
            for failed, lat, err in ex.map(do_hot_iter, range(iterations)):
                if err is not None:
                    result.errors.append(f"hot lookup error: {err}")
                elif failed:
                    result.errors.append(f"hot lookup failed: {failed} keys")
                if lat is not None:
                    result.hot_latencies.append(lat)

    result.hot_end = time.perf_counter()
    result.hot_objects = num_objects * iterations

    # --- Phase 3: Cold lookups (SSD-tier) ---
    # Clear the server's memory-tier so lookups must go to SSD.
    # The initial FlushToSsd after populate already ensured all data is on NAND.
    barrier.wait()
    if client_id == 0:
        try:
            ring.clear_memory_tier()
        except RingError as e:
            result.errors.append(f"ClearMemoryTier failed: {str(e)}")
    barrier.wait()

    # Cold lookups use batched requests with SEPARATE IPC handles per key.
    # A shared IPC handle allows the server to skip SSD reads for entries whose
    # GPU buffer will be overwritten — distinct buffers force real SSD reads.
    # Pipelined: keep pipeline_depth requests in flight concurrently (iperf -P).
    barrier.wait()
    result.cold_start = time.perf_counter()

    def do_cold_iter(iter_idx):
        cold_start = iter_idx * num_objects
        cold_keys = [base_key + cold_start + i for i in range(num_objects)]
        entries = [
            (k, [single_region(cold_lookup_handles[i], gpu_id, BLOCK_SIZE)])
            for i, k in enumerate(cold_keys)
        ]
        t0 = time.perf_counter()
        try:
            oks = ring.lookup(entries)
            t1 = time.perf_counter()
            failed = sum(1 for ok in oks if not ok)
            return iter_idx, failed, (t1 - t0) / num_objects, None
        except RingError as e:
            return iter_idx, num_objects, None, str(e)

    with _futures.ThreadPoolExecutor(max_workers=pipeline_depth) as ex:
        for iter_idx, failed, lat, err in ex.map(do_cold_iter, range(iterations)):
            if err is not None:
                result.errors.append(f"cold lookup error: {err}")
            elif failed:
                result.errors.append(
                    f"cold lookup iter {iter_idx} failed: {failed} keys"
                )
            if lat is not None:
                result.cold_latencies.append(lat)

    result.cold_end = time.perf_counter()
    result.cold_objects = num_objects * iterations

    # --- Cleanup ---
    all_cleanup_keys = list(range(base_key, base_key + total_objects))
    for batch_start in range(0, len(all_cleanup_keys), batch_size):
        batch_end = min(batch_start + batch_size, len(all_cleanup_keys))
        try:
            ring.remove(all_cleanup_keys[batch_start:batch_end])
        except RingError:
            pass

    # Free lookup GPU buffers. The shmq ring is shared and closed by main.
    for ptr in hot_lookup_ptrs:
        _cuda_free(ptr)
    for ptr in cold_lookup_ptrs:
        _cuda_free(ptr)


def print_stats(label, all_latencies, num_clients, wall_aggregate_gbps=None):
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

    print(
        f"  {label:<20} "
        f"avg={avg*1e6:>9.1f} us  "
        f"p50={p50*1e6:>9.1f} us  "
        f"p99={p99*1e6:>9.1f} us  "
        f"min={mn*1e6:>9.1f} us  "
        f"max={mx*1e6:>9.1f} us"
    )
    # Throughput from wall-clock measurement (total bytes / elapsed time).
    if wall_aggregate_gbps is not None:
        tp_per_client = wall_aggregate_gbps / max(num_clients, 1)
        warn = " (!)" if wall_aggregate_gbps > 32 else ""
        print(
            f"  {'':20} "
            f"per-client={tp_per_client:>6.2f} GB/s  "
            f"aggregate={wall_aggregate_gbps:>6.2f} GB/s{warn}"
        )


def main():
    parser = argparse.ArgumentParser(
        description="Certus multi-client throughput/latency benchmark"
    )
    add_shm_arg(parser)
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
        "--block-size",
        type=parse_size,
        default=None,
        help="Block size (e.g. 4M, 128K, 1G). Defaults to 4M.",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=10,
        help="Number of requests per batch/RPC call per client (default: 10)",
    )
    parser.add_argument(
        "--gpus",
        type=int,
        default=1,
        help="Number of GPUs to spread clients across (default: 1). "
        "Clients are assigned round-robin to GPUs 0..N-1.",
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
    parser.add_argument(
        "--pipeline-depth",
        type=int,
        default=4,
        help="Concurrent RPCs in flight per client (default: 4). "
        "Keeps multiple requests in flight like iperf -P to saturate the GPU PCIe link.",
    )
    parser.add_argument(
        "--writes-settle",
        type=float,
        default=30.0,
        help="Seconds to wait after populate for write-through to drain (default: 30). "
        "Set to 0 to skip.",
    )
    parser.add_argument(
        "--memory-tier-size",
        type=parse_size,
        default=None,
        help="Memory-tier pool size assumption (e.g. 4G, 2G). For multi-client runs, "
        "set to HALF the server's --memory-tier-size to avoid cross-client eviction. "
        "Defaults to 2G.",
    )
    parser.add_argument(
        "--sequential-hot",
        action="store_true",
        help="Use sequential (non-pipelined) RPCs for hot lookups. "
        "Measures per-request latency instead of throughput.",
    )
    parser.add_argument(
        "--continuous",
        action="store_true",
        help="Run continuously in a loop. Populate runs once on the first round; "
        "subsequent rounds repeat only the lookup phases. Ctrl-C to stop.",
    )
    args = parser.parse_args()

    global BLOCK_SIZE, MEMORY_TIER_SIZE
    if args.block_size is not None:
        BLOCK_SIZE = args.block_size
    if args.memory_tier_size is not None:
        MEMORY_TIER_SIZE = args.memory_tier_size

    num_clients = args.clients
    num_objects = args.num_objects
    iterations = args.iterations
    batch_size = args.batch_size
    num_gpus = args.gpus

    available_gpus = torch.cuda.device_count()
    if num_gpus > available_gpus:
        print(
            f"ERROR: --gpus {num_gpus} requested but only {available_gpus} GPU(s) available",
            file=sys.stderr,
        )
        sys.exit(1)

    pool_capacity = MEMORY_TIER_SIZE // BLOCK_SIZE
    client_pool_share = max(1, pool_capacity // num_clients)
    cold_per_client = num_objects * iterations
    total_per_client = client_pool_share + cold_per_client

    print(f"{'='*70}")
    print(f"Certus Multi-Client Benchmark")
    print(f"{'='*70}")
    print(f"  Server:            {args.shm_path}")
    print(f"  Clients:           {num_clients}")
    print(f"  GPUs:              {num_gpus}")
    print(f"  Block size:        {BLOCK_SIZE // (1024*1024)} MiB")
    print(f"  Batch size:        {batch_size}")
    print(f"  Objects/batch:     {num_objects}")
    print(f"  Iterations:        {iterations}")
    pool_mib = MEMORY_TIER_SIZE // (1024 * 1024)
    print(f"  Pool capacity:     {pool_capacity} objects ({pool_mib} MiB) / {client_pool_share} per client")
    print(f"  Total per client:  {total_per_client} objects")
    print(f"  Cold per client:   {cold_per_client} objects")
    print(f"  Pipeline depth:    {args.pipeline_depth}")
    print(f"  (server needs --channels >= clients*pipeline = "
          f"{num_clients * args.pipeline_depth})")
    print()

    # Each client gets a non-overlapping key range.
    key_range_size = 10_000_000
    base_keys = [
        random.randint(1_000_000, 100_000_000) + i * key_range_size
        for i in range(num_clients)
    ]

    continuous = args.continuous
    round_num = 0
    pipeline_depth = args.pipeline_depth

    # One shared shmq client for all client threads; each thread (and each of its
    # pipeline ThreadPoolExecutor workers) auto-claims a distinct sticky channel.
    ring = connect(args.shm_path)
    needed_channels = num_clients * pipeline_depth
    if ring.channel_count < needed_channels:
        print(f"  WARNING: server exposes {ring.channel_count} channels but "
              f"clients*pipeline-depth = {needed_channels}; extra threads will "
              f"error. Launch certus-server with --channels >= {needed_channels}.")

    while True:
        round_num += 1

        if continuous and round_num > 1:
            print(f"\n{'='*70}")
            print(f"  Round {round_num}")
            print(f"{'='*70}")

        barrier = threading.Barrier(num_clients)
        results = [ClientResult(i) for i in range(num_clients)]
        threads = []

        if round_num == 1:
            print(f"  Starting {num_clients} client(s)...")
        t_total_start = time.perf_counter()

        for i in range(num_clients):
            gpu_id = i % num_gpus
            t = threading.Thread(
                target=run_client,
                args=(
                    i,
                    ring,
                    num_objects,
                    iterations,
                    base_keys[i],
                    num_clients,
                    batch_size,
                    barrier,
                    results[i],
                    gpu_id,
                    pipeline_depth,
                    args.writes_settle,
                    args.sequential_hot,
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

        # Compute true wall-clock aggregate throughput:
        pop_wall_agg = None
        hot_wall_agg = None
        cold_wall_agg = None
        active_pop = [r for r in results if r.populate_objects > 0]
        active_hot = [r for r in results if r.hot_objects > 0]
        active_cold = [r for r in results if r.cold_objects > 0]
        if active_pop:
            pop_elapsed = max(r.populate_end for r in active_pop) - min(r.populate_start for r in active_pop)
            pop_total_bytes = sum(r.populate_objects for r in active_pop) * BLOCK_SIZE
            pop_wall_agg = (pop_total_bytes / pop_elapsed / 1e9) if pop_elapsed > 0 else 0
        if active_hot:
            hot_elapsed = max(r.hot_end for r in active_hot) - min(r.hot_start for r in active_hot)
            hot_total_bytes = sum(r.hot_objects for r in active_hot) * BLOCK_SIZE
            hot_wall_agg = (hot_total_bytes / hot_elapsed / 1e9) if hot_elapsed > 0 else 0
        if active_cold:
            cold_elapsed = max(r.cold_end for r in active_cold) - min(r.cold_start for r in active_cold)
            cold_total_bytes = sum(r.cold_objects for r in active_cold) * BLOCK_SIZE
            cold_wall_agg = (cold_total_bytes / cold_elapsed / 1e9) if cold_elapsed > 0 else 0

        # Wall-clock elapsed times for reporting
        pop_elapsed_s = (max(r.populate_end for r in active_pop) - min(r.populate_start for r in active_pop)) if active_pop else 0
        hot_elapsed_s = (max(r.hot_end for r in active_hot) - min(r.hot_start for r in active_hot)) if active_hot else 0
        cold_elapsed_s = (max(r.cold_end for r in active_cold) - min(r.cold_start for r in active_cold)) if active_cold else 0

        print(f"\n{'='*70}")
        print(f"Results ({num_clients} client(s), {BLOCK_SIZE//(1024*1024)} MiB blocks, round {round_num})")
        print(f"{'='*70}")
        print()
        print("  Latency per object (all clients combined):")
        print()
        print_stats("Populate", all_populate, num_clients, pop_wall_agg)
        if pop_elapsed_s > 0:
            print(f"  {'':20} wall={pop_elapsed_s*1000:.1f} ms")
        print()
        print_stats("Lookup (hot)", all_hot, num_clients, hot_wall_agg)
        if hot_elapsed_s > 0:
            print(f"  {'':20} wall={hot_elapsed_s*1000:.1f} ms")
        print()
        print_stats("Lookup (cold)", all_cold, num_clients, cold_wall_agg)
        if cold_elapsed_s > 0:
            print(f"  {'':20} wall={cold_elapsed_s*1000:.1f} ms")
        print()

        if all_hot and all_cold:
            hot_avg = statistics.mean(all_hot)
            cold_avg = statistics.mean(all_cold)
            if hot_avg > 0:
                print(f"  Cold/Hot ratio:    {cold_avg/hot_avg:.1f}x latency")

        print(f"\n  Total wall time:   {t_total_end - t_total_start:.2f}s")

        # Per-client summary
        print(f"\n  Per-client breakdown:")
        print(
            f"  {'Client':<8} {'GPU':<5} {'Hot avg (us)':<14} {'Cold avg (us)':<14} {'Errors':<8}"
        )
        print(f"  {'-'*49}")
        for r in results:
            gpu_id = r.client_id % num_gpus
            hot_avg = statistics.mean(r.hot_latencies) * 1e6 if r.hot_latencies else 0
            cold_avg = (
                statistics.mean(r.cold_latencies) * 1e6 if r.cold_latencies else 0
            )
            print(
                f"  {r.client_id:<8} {gpu_id:<5} {hot_avg:<14.1f} {cold_avg:<14.1f} {len(r.errors):<8}"
            )

        print()

        # --- Integrity verification (optional, first round only) ---
        integrity_ok = True
        if args.verify_integrity and round_num == 1:
            integrity_ok = run_integrity_check(ring, args.integrity_objects)

        if not continuous:
            ring.close()
            sys.exit(1 if (all_errors or not integrity_ok) else 0)


if __name__ == "__main__":
    main()
