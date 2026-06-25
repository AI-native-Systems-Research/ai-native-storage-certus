#!/usr/bin/env python3
"""Integration test: partial batch lookup spanning all data tiers.

Verifies that a single BatchLookup request can return mixed results when
the batch contains keys from different tiers:
  - Memory-tier (hot): recently populated, still in DRAM → success
  - SSD-tier (cold): flushed to SSD, evicted from DRAM → success
  - Remote lookup (non-existent): keys that don't exist locally, traverse
    the IRemoteLookup path → KeyNotFound (placeholder returns NotFound)

Usage:
    python test-tier-batch.py --server localhost:50051
"""

import argparse
import ctypes
import os
import random
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

ERROR_CODE_KEY_NOT_FOUND = 2
ERROR_CODE_IO_ERROR = 5


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
    buf = (ctypes.c_ubyte * size)()
    err = _libcudart.cudaMemcpy(ctypes.byref(buf), dev_ptr, size, _CUDA_MEMCPY_D2H)
    if err != 0:
        raise RuntimeError(f"cudaMemcpy D2H failed: {err}")
    return bytes(buf)


def make_pattern(key, block_size):
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
        description="Integration test: partial batch lookup across all tiers")
    parser.add_argument("--server", default="localhost:50051")
    parser.add_argument("--block-size", type=parse_size, default=2 * 1024 * 1024,
                        help="Object size (default: 2M)")
    parser.add_argument("--num-per-tier", type=int, default=4,
                        help="Number of objects per tier category (default: 4)")
    parser.add_argument("--settle", type=float, default=3.0,
                        help="Seconds to wait for write-through to SSD (default: 3)")
    parser.add_argument("--gpu", type=int, default=0,
                        help="GPU device index (default: 0)")
    args = parser.parse_args()

    block_size = args.block_size
    n = args.num_per_tier

    # Key ranges for each tier category
    hot_keys = [80_000_000 + i for i in range(n)]
    cold_keys = [80_001_000 + i for i in range(n)]
    remote_keys = [80_009_000 + i for i in range(n)]  # never populated

    all_populated_keys = hot_keys + cold_keys

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 60)
    print("Partial Batch Lookup — Multi-Tier Integration Test")
    print("=" * 60)
    print(f"  Server:       {args.server}")
    print(f"  Block size:   {block_size // 1024} KiB")
    print(f"  Per tier:     {n} objects")
    print(f"  Tiers:        hot (memory), cold (SSD), remote (non-existent)")
    print()

    # --- Allocate GPU buffers ---
    print("  Allocating GPU buffers...")
    pop_ptrs = []
    pop_handles = []
    for _ in range(len(all_populated_keys)):
        ptr, handle = cuda_alloc(block_size)
        pop_ptrs.append(ptr)
        pop_handles.append(handle)

    # One lookup buffer per batch entry (hot + cold + remote)
    total_batch = n * 3
    lookup_ptrs = []
    lookup_handles = []
    for _ in range(total_batch):
        ptr, handle = cuda_alloc(block_size)
        lookup_ptrs.append(ptr)
        lookup_handles.append(handle)

    # Fill populate buffers with deterministic patterns
    for i, key in enumerate(all_populated_keys):
        pattern = make_pattern(key, block_size)
        gpu_write(pop_ptrs[i], pattern)

    # --- Connect ---
    channel = grpc.insecure_channel(
        args.server,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    passed = 0
    failed = 0
    errors = []

    try:
        # === Phase 1: Populate cold keys first (they'll be evicted later) ===
        print("  Phase 1: Populating cold-tier keys...")
        cold_entries = [
            dispatcher_pb2.PopulateEntry(
                key=cold_keys[i],
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=pop_handles[n + i],
                    size=block_size,
                ),
            )
            for i in range(n)
        ]
        resp = stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=cold_entries))
        pop_failed = sum(1 for r in resp.results if not r.success)
        if pop_failed:
            errors.append(f"cold populate: {pop_failed}/{n} failures")
            for r in resp.results:
                if not r.success:
                    errors.append(f"  key={r.key}: {r.error_message}")

        # Flush cold keys to SSD and wait
        print(f"  Flushing to SSD and waiting {args.settle}s...")
        stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())
        time.sleep(args.settle)

        # Clear memory tier — cold keys now only on SSD
        print("  Clearing memory tier (cold keys now SSD-only)...")
        stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())

        # === Phase 2: Populate hot keys (stay in memory-tier) ===
        print("  Phase 2: Populating hot-tier keys...")
        hot_entries = [
            dispatcher_pb2.PopulateEntry(
                key=hot_keys[i],
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=pop_handles[i],
                    size=block_size,
                ),
            )
            for i in range(n)
        ]
        resp = stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=hot_entries))
        pop_failed = sum(1 for r in resp.results if not r.success)
        if pop_failed:
            errors.append(f"hot populate: {pop_failed}/{n} failures")
            for r in resp.results:
                if not r.success:
                    errors.append(f"  key={r.key}: {r.error_message}")

        # === Phase 3: Mixed batch lookup ===
        # Order: [hot_0..hot_n, cold_0..cold_n, remote_0..remote_n]
        print("  Phase 3: Sending mixed BatchLookup (hot + cold + remote)...")
        batch_keys = hot_keys + cold_keys + remote_keys
        lookup_entries = [
            dispatcher_pb2.LookupEntry(
                key=batch_keys[i],
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=lookup_handles[i],
                    size=block_size,
                ),
            )
            for i in range(total_batch)
        ]
        req = dispatcher_pb2.BatchLookupRequest(entries=lookup_entries)
        resp = stub.Lookup(req)
        _libcudart.cudaDeviceSynchronize()

        results = list(resp.results)

        # === Phase 4: Validate results ===
        print()
        print("  Validating results...")
        print(f"    Total entries in batch: {total_batch}")
        print(f"    Results returned:       {len(results)}")

        if len(results) != total_batch:
            errors.append(
                f"result count mismatch: expected {total_batch}, got {len(results)}")

        # --- Validate hot tier (first n entries) ---
        print()
        print("  [HOT TIER - Memory]")
        hot_results = results[:n]
        hot_pass = 0
        for i, r in enumerate(hot_results):
            key = hot_keys[i]
            if r.success:
                actual = gpu_read(lookup_ptrs[i], block_size)
                expected = make_pattern(key, block_size)
                if actual == expected:
                    hot_pass += 1
                else:
                    errors.append(f"hot key={key}: data integrity mismatch")
            else:
                errors.append(
                    f"hot key={key}: expected success, got error_code={r.error_code} "
                    f"({r.error_message})")
        print(f"    Success + integrity: {hot_pass}/{n}")
        passed += hot_pass
        failed += (n - hot_pass)

        # --- Validate cold tier (next n entries) ---
        print()
        print("  [COLD TIER - SSD]")
        cold_results = results[n:2*n]
        cold_pass = 0
        for i, r in enumerate(cold_results):
            key = cold_keys[i]
            if r.success:
                actual = gpu_read(lookup_ptrs[n + i], block_size)
                expected = make_pattern(key, block_size)
                if actual == expected:
                    cold_pass += 1
                else:
                    errors.append(f"cold key={key}: data integrity mismatch")
            else:
                errors.append(
                    f"cold key={key}: expected success, got error_code={r.error_code} "
                    f"({r.error_message})")
        print(f"    Success + integrity: {cold_pass}/{n}")
        passed += cold_pass
        failed += (n - cold_pass)

        # --- Validate remote tier (last n entries) ---
        # These keys were never populated. The dispatcher forwards them to
        # IRemoteLookup::batch_lookup, which (being a placeholder) returns
        # NotFound. The dispatcher wraps this as IoError("remote lookup: key
        # not found"), so the gRPC layer reports ERROR_CODE_IO_ERROR with a
        # message containing "remote lookup".
        print()
        print("  [REMOTE TIER - IRemoteLookup path]")
        remote_results = results[2*n:]
        remote_pass = 0
        for i, r in enumerate(remote_results):
            key = remote_keys[i]
            if not r.success and (
                r.error_code == ERROR_CODE_IO_ERROR
                and "remote lookup" in r.error_message
            ):
                remote_pass += 1
            elif not r.success and r.error_code == ERROR_CODE_KEY_NOT_FOUND:
                # Also accept direct KeyNotFound (if remote_lookup not bound)
                remote_pass += 1
            elif r.success:
                errors.append(
                    f"remote key={key}: expected failure, got success "
                    f"(remote placeholder should not resolve)")
            else:
                errors.append(
                    f"remote key={key}: unexpected error_code={r.error_code} "
                    f"({r.error_message})")
        print(f"    Not found (remote path): {remote_pass}/{n}")
        passed += remote_pass
        failed += (n - remote_pass)

    except grpc.RpcError as e:
        errors.append(f"gRPC error: {e.code()} - {e.details()}")
    finally:
        # === Cleanup ===
        print()
        print("  Cleaning up...")
        try:
            stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=all_populated_keys))
        except grpc.RpcError:
            pass
        for ptr in pop_ptrs:
            cuda_free(ptr)
        for ptr in lookup_ptrs:
            cuda_free(ptr)
        channel.close()

    # === Summary ===
    print()
    print("=" * 60)
    print("Results")
    print("=" * 60)
    print(f"  Passed: {passed}/{total_batch}")
    print(f"  Failed: {failed}/{total_batch}")
    if errors:
        print()
        print("  Errors:")
        for e in errors:
            print(f"    - {e}")
    print()

    if failed == 0 and not errors:
        print("  PASS: All tiers returned expected results")
        print("    - Hot (memory-tier):  success with correct data")
        print("    - Cold (SSD-tier):    success with correct data")
        print("    - Remote (not found): KeyNotFound via IRemoteLookup path")
        sys.exit(0)
    else:
        print("  FAIL: Some tier results did not match expectations")
        sys.exit(1)


if __name__ == "__main__":
    main()
