#!/usr/bin/env python3
"""Hardware integration test for Touch(promote=true).

Verifies that promotion actually moves data from SSD to the memory-tier
by comparing lookup latency before and after promotion. A successful
promotion should make subsequent lookups significantly faster (memory-tier
hit vs. SSD cold read).

Additionally verifies data integrity: each object is populated with a
unique per-key byte pattern. Both cold retrievals (from SSD) and warm
retrievals (post-promote, from memory tier) are checked to ensure the
data matches the original pattern byte-for-byte.

Usage:
    python test-promote.py --server localhost:50051 --block-size 2M --num-objects 10
"""

import argparse
import ctypes
import os
import random
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
_CUDA_MEMCPY_D2H = 2


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


def gpu_read(dev_ptr, size):
    """Copy bytes from GPU device memory back to host."""
    buf = (ctypes.c_ubyte * size)()
    err = _libcudart.cudaMemcpy(ctypes.byref(buf), dev_ptr, size, _CUDA_MEMCPY_D2H)
    if err != 0:
        raise RuntimeError(f"cudaMemcpy D2H failed: {err}")
    return bytes(buf)


def make_pattern(key, block_size):
    """Create a deterministic byte pattern unique to a given key."""
    rng = random.Random(key)
    return bytes(rng.getrandbits(8) for _ in range(block_size))


def parse_size(s):
    s = s.strip()
    suffix = s[-1].upper()
    multipliers = {"K": 1024, "M": 1024 * 1024, "G": 1024 * 1024 * 1024}
    if suffix in multipliers:
        return int(s[:-1]) * multipliers[suffix]
    return int(s)


def main():
    parser = argparse.ArgumentParser(
        description="Hardware integration test for Touch(promote=true)")
    parser.add_argument("--server", default="localhost:50051")
    parser.add_argument("--block-size", type=parse_size, default=2 * 1024 * 1024,
                        help="Object size (default: 2M)")
    parser.add_argument("--num-objects", type=int, default=10,
                        help="Number of objects to test with (default: 10)")
    parser.add_argument("--settle", type=float, default=5.0,
                        help="Seconds to wait for write-through to SSD (default: 5)")
    parser.add_argument("--promote-wait", type=float, default=2.0,
                        help="Seconds to wait for async promotion (default: 2)")
    parser.add_argument("--iterations", type=int, default=5,
                        help="Lookup iterations per phase (default: 5)")
    parser.add_argument("--gpu", type=int, default=0,
                        help="GPU device index (default: 0)")
    parser.add_argument("--threshold", type=float, default=0.9,
                        help="Max ratio warm/cold for PASS (default: 0.9)")
    args = parser.parse_args()

    block_size = args.block_size
    num_objects = args.num_objects
    base_key = 90_000_000

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 60)
    print("Touch(promote=true) Hardware Integration Test")
    print("=" * 60)
    print(f"  Server:       {args.server}")
    print(f"  Block size:   {block_size // 1024} KiB")
    print(f"  Objects:      {num_objects}")
    print(f"  Iterations:   {args.iterations}")
    print(f"  Threshold:    warm < cold * {args.threshold}")
    print()

    # Allocate GPU buffers
    print("  Allocating GPU buffers...")
    pop_ptrs, pop_handles = [], []
    lookup_ptrs, lookup_handles = [], []
    for _ in range(num_objects):
        ptr, handle = cuda_alloc(block_size)
        pop_ptrs.append(ptr)
        pop_handles.append(handle)
        ptr, handle = cuda_alloc(block_size)
        lookup_ptrs.append(ptr)
        lookup_handles.append(handle)

    keys = [base_key + i for i in range(num_objects)]

    # Fill populate buffers with unique per-key patterns
    patterns = []
    for i, ptr in enumerate(pop_ptrs):
        pattern = make_pattern(keys[i], block_size)
        patterns.append(pattern)
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

    # --- Phase 1: Populate ---
    print("  Populating objects...")
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
    resp = stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))
    failed = sum(1 for r in resp.results if not r.success)
    if failed:
        print(f"  ERROR: {failed}/{num_objects} populate failures")
        for r in resp.results:
            if not r.success:
                print(f"    key={r.key}: {r.error_message}")
        sys.exit(1)

    # --- Phase 2: Flush to SSD and wait ---
    print(f"  Flushing to SSD and waiting {args.settle}s...")
    stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())
    time.sleep(args.settle)

    # --- Phase 3: Clear memory tier (entries become cold) ---
    print("  Clearing memory tier...")
    stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())

    # --- Phase 4: Measure cold lookup latency + integrity ---
    # Each cold lookup also promotes the entry, so clear before each iteration.
    print("  Measuring cold lookup latency (first access from SSD)...")
    cold_latencies = []
    cold_integrity_pass = 0
    cold_integrity_fail = 0
    for it in range(args.iterations):
        # Clear before each cold measurement to ensure entries are on SSD
        if it > 0:
            stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
        lookup_entries = [
            dispatcher_pb2.LookupEntry(
                key=k,
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=lookup_handles[i],
                    size=block_size,
                ),
            )
            for i, k in enumerate(keys)
        ]
        req = dispatcher_pb2.BatchLookupRequest(entries=lookup_entries)
        t0 = time.perf_counter()
        resp = stub.Lookup(req)
        _libcudart.cudaDeviceSynchronize()
        t1 = time.perf_counter()
        failed = sum(1 for r in resp.results if not r.success)
        if failed == 0:
            cold_latencies.append((t1 - t0) * 1e6)

        # Verify data integrity on last successful cold iteration
        if it == args.iterations - 1 and failed == 0:
            for i, k in enumerate(keys):
                actual = gpu_read(lookup_ptrs[i], block_size)
                expected = make_pattern(k, block_size)
                if actual == expected:
                    cold_integrity_pass += 1
                else:
                    cold_integrity_fail += 1
                    first_bad = next(
                        (j for j in range(len(actual)) if actual[j] != expected[j]),
                        "?",
                    )
                    print(
                        f"    INTEGRITY FAIL (cold): key={k}, "
                        f"first mismatch at byte {first_bad}"
                    )

    if not cold_latencies:
        print("  ERROR: all cold lookups failed")
        sys.exit(1)

    cold_avg = statistics.mean(cold_latencies)
    print(f"    Cold lookup avg: {cold_avg:.0f} us")
    print(
        f"    Cold integrity:  {cold_integrity_pass} pass, "
        f"{cold_integrity_fail} fail"
    )

    # --- Phase 5: Clear memory tier again (re-cold for promote test) ---
    print("  Clearing memory tier again...")
    stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())

    # --- Phase 6: Touch with promote=true ---
    print("  Sending Touch(promote=true)...")
    touch_req = dispatcher_pb2.BatchTouchRequest(keys=keys, promote=True)
    t0 = time.perf_counter()
    touch_resp = stub.Touch(touch_req)
    t1 = time.perf_counter()
    touch_latency = (t1 - t0) * 1e6
    touch_failed = sum(1 for r in touch_resp.results if not r.success)
    print(f"    Touch RPC latency: {touch_latency:.0f} us (fire-and-forget)")
    if touch_failed:
        print(f"    WARNING: {touch_failed} keys failed touch")

    # --- Phase 7: Wait for async promotion to complete ---
    print(f"  Waiting {args.promote_wait}s for background promotion...")
    time.sleep(args.promote_wait)

    # --- Phase 8: Measure warm lookup latency (should hit memory-tier) ---
    # Do a warmup lookup first (first access after promote may have GPU init overhead)
    print("  Measuring warm lookup latency (post-promote)...")
    lookup_entries = [
        dispatcher_pb2.LookupEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=lookup_handles[i],
                size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    # Warmup (discard first measurement — GPU DMA path warmup)
    stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=lookup_entries))
    _libcudart.cudaDeviceSynchronize()

    warm_latencies = []
    warm_integrity_pass = 0
    warm_integrity_fail = 0
    for it in range(args.iterations):
        req = dispatcher_pb2.BatchLookupRequest(entries=lookup_entries)
        t0 = time.perf_counter()
        resp = stub.Lookup(req)
        _libcudart.cudaDeviceSynchronize()
        t1 = time.perf_counter()
        failed = sum(1 for r in resp.results if not r.success)
        if failed == 0:
            warm_latencies.append((t1 - t0) * 1e6)

        # Verify data integrity on last successful warm iteration
        if it == args.iterations - 1 and failed == 0:
            for i, k in enumerate(keys):
                actual = gpu_read(lookup_ptrs[i], block_size)
                expected = make_pattern(k, block_size)
                if actual == expected:
                    warm_integrity_pass += 1
                else:
                    warm_integrity_fail += 1
                    first_bad = next(
                        (j for j in range(len(actual)) if actual[j] != expected[j]),
                        "?",
                    )
                    print(
                        f"    INTEGRITY FAIL (warm): key={k}, "
                        f"first mismatch at byte {first_bad}"
                    )

    if not warm_latencies:
        print("  ERROR: all warm lookups failed")
        sys.exit(1)

    warm_avg = statistics.mean(warm_latencies)
    print(f"    Warm lookup avg: {warm_avg:.0f} us")
    print(
        f"    Warm integrity:  {warm_integrity_pass} pass, "
        f"{warm_integrity_fail} fail"
    )

    # --- Phase 9: Results ---
    print()
    print("=" * 60)
    print("Results")
    print("=" * 60)
    print(f"  Cold lookup avg:  {cold_avg:>8.0f} us  (SSD → GPU)")
    print(f"  Warm lookup avg:  {warm_avg:>8.0f} us  (DRAM → GPU, post-promote)")
    ratio = warm_avg / cold_avg if cold_avg > 0 else 1.0
    speedup = cold_avg / warm_avg if warm_avg > 0 else 0.0
    print(f"  Ratio (warm/cold): {ratio:.3f}")
    print(f"  Speedup:           {speedup:.1f}x")
    print()
    print("  Data Integrity:")
    print(f"    Cold path: {cold_integrity_pass}/{num_objects} correct")
    print(f"    Warm path: {warm_integrity_pass}/{num_objects} correct")
    integrity_ok = (cold_integrity_fail == 0 and warm_integrity_fail == 0)
    print()

    # --- Phase 10: Cleanup ---
    print("  Cleaning up...")
    remove_req = dispatcher_pb2.BatchRemoveRequest(keys=keys)
    stub.Remove(remove_req)
    for ptr in pop_ptrs:
        cuda_free(ptr)
    for ptr in lookup_ptrs:
        cuda_free(ptr)
    channel.close()

    # --- Verdict ---
    if not integrity_ok:
        print(f"  FAIL: data integrity errors detected")
        print(f"        Cold: {cold_integrity_fail} corrupted, Warm: {warm_integrity_fail} corrupted")
        sys.exit(1)
    elif ratio < args.threshold:
        print(f"  PASS: promotion reduced lookup latency (ratio={ratio:.3f} < {args.threshold})")
        print(f"        Data integrity verified for both hot and cold paths.")
        sys.exit(0)
    else:
        print(f"  FAIL: warm lookup not fast enough (ratio={ratio:.3f} >= {args.threshold})")
        print(f"        Expected promote to move data from SSD to DRAM.")
        sys.exit(1)


if __name__ == "__main__":
    main()
