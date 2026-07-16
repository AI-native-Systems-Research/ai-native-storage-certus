#!/usr/bin/env python3
"""Integration test: background memory-tier threshold eviction.

Verifies that the MemoryTierEvictor proactively demotes DRAM entries to SSD
when memory-tier utilization exceeds the configured threshold. The server must
be started with --memory-tier-eviction-threshold set (e.g. 0.5) and a small
enough memory-tier pool that populating the test entries crosses the threshold.

Test sequence:
  1. Populate entries until the memory-tier is past the eviction threshold.
  2. Wait for the background evictor to run (threshold check + demotion).
  3. Drain eviction events via TakeEvents — expect DEMOTED events.
  4. Verify demoted keys are still accessible via Lookup (served from SSD).

Prerequisites:
  - certus-server or certus-server-yaml running with:
      --memory-tier-eviction-threshold 0.5  (or similar)
      a small memory-tier pool (e.g. 64M) so the test can fill past 50%
  - GPU available for IPC DMA

Usage:
    python test-memory-tier-eviction.py --server localhost:50051
    python test-memory-tier-eviction.py --server localhost:50051 --block-size 2M --num-entries 40
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

EVICTION_REASON_DEMOTED = 1
EVICTION_REASON_REMOVED = 2


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
        description="Integration test: background memory-tier threshold eviction")
    parser.add_argument("--server", default="localhost:50051")
    parser.add_argument("--block-size", type=parse_size, default=2 * 1024 * 1024,
                        help="Object size (default: 2M)")
    parser.add_argument("--num-entries", type=int, default=40,
                        help="Number of entries to populate (must exceed pool threshold)")
    parser.add_argument("--wait-secs", type=float, default=8.0,
                        help="Seconds to wait for background evictor (default: 8)")
    parser.add_argument("--gpu", type=int, default=0,
                        help="GPU device index (default: 0)")
    args = parser.parse_args()

    block_size = args.block_size
    n = args.num_entries
    keys = [90_000_000 + i for i in range(n)]

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 60)
    print("Memory-Tier Threshold Eviction — Integration Test")
    print("=" * 60)
    print(f"  Server:       {args.server}")
    print(f"  Block size:   {block_size // 1024} KiB")
    print(f"  Entries:      {n}")
    print(f"  Total data:   {n * block_size // (1024 * 1024)} MiB")
    print(f"  Wait time:    {args.wait_secs}s")
    print()

    # --- Allocate GPU buffers ---
    print("  Allocating GPU buffers for populate...")
    pop_ptrs = []
    pop_handles = []
    for _ in range(n):
        ptr, handle = cuda_alloc(block_size)
        pop_ptrs.append(ptr)
        pop_handles.append(handle)

    # Fill with deterministic patterns
    for i, key in enumerate(keys):
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

    errors = []

    try:
        # === Phase 1: Drain any pre-existing eviction events ===
        print("  Phase 1: Draining stale eviction events...")
        stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))

        # === Phase 2: Populate entries to exceed threshold ===
        print(f"  Phase 2: Populating {n} entries ({n * block_size // (1024*1024)} MiB)...")
        batch_size = 8
        populated = 0
        for batch_start in range(0, n, batch_size):
            batch_end = min(batch_start + batch_size, n)
            entries = [
                dispatcher_pb2.PopulateEntry(
                    key=keys[i],
                    ipc_handle=dispatcher_pb2.IpcHandle(
                        cuda_ipc_handle=pop_handles[i],
                        size=block_size,
                    ),
                )
                for i in range(batch_start, batch_end)
            ]
            resp = stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))
            for r in resp.results:
                if r.success:
                    populated += 1
                else:
                    errors.append(f"populate key={r.key}: {r.error_code} {r.error_message}")
            print(f"    Populated {populated}/{n}", end="\r")
        print(f"    Populated {populated}/{n} entries successfully")

        if populated == 0:
            errors.append("No entries populated — cannot test eviction")
            raise SystemExit(1)

        # === Phase 3: Wait for write-through + evictor cycle ===
        # The evictor needs entries to be evictable (write-through complete).
        # FlushToSsd ensures write-through completes, then we wait for the
        # evictor's periodic check to fire.
        print("  Phase 3: Flushing write-through to SSD...")
        stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())

        print(f"  Phase 3: Waiting {args.wait_secs}s for background evictor...")
        time.sleep(args.wait_secs)

        # === Phase 4: Drain eviction events ===
        print("  Phase 4: Draining eviction events...")
        resp = stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))
        events = list(resp.events)
        demoted_keys = set()
        removed_keys = set()
        for evt in events:
            if evt.reason == EVICTION_REASON_DEMOTED:
                demoted_keys.add(evt.key)
            elif evt.reason == EVICTION_REASON_REMOVED:
                removed_keys.add(evt.key)

        print(f"    Events received:  {len(events)}")
        print(f"    Demoted (DRAM→SSD): {len(demoted_keys)}")
        print(f"    Removed:            {len(removed_keys)}")
        print(f"    Dropped count:      {resp.dropped_count}")

        if len(demoted_keys) == 0:
            errors.append(
                "No DEMOTED eviction events received. "
                "Ensure --memory-tier-eviction-threshold is set on the server "
                "and the pool is small enough that populating crosses the threshold.")

        # === Phase 5: Verify demoted keys are still accessible (cold path) ===
        if demoted_keys:
            verify_keys = sorted(demoted_keys)[:min(8, len(demoted_keys))]
            print(f"  Phase 5: Verifying {len(verify_keys)} demoted keys via Lookup (cold path)...")

            lookup_ptrs = []
            lookup_handles = []
            for _ in verify_keys:
                ptr, handle = cuda_alloc(block_size)
                lookup_ptrs.append(ptr)
                lookup_handles.append(handle)

            lookup_entries = [
                dispatcher_pb2.LookupEntry(
                    key=k,
                    ipc_handle=dispatcher_pb2.IpcHandle(
                        cuda_ipc_handle=lookup_handles[i],
                        size=block_size,
                    ),
                )
                for i, k in enumerate(verify_keys)
            ]
            resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=lookup_entries))
            _libcudart.cudaDeviceSynchronize()

            verified = 0
            for i, r in enumerate(resp.results):
                key = verify_keys[i]
                if r.success:
                    actual = gpu_read(lookup_ptrs[i], block_size)
                    expected = make_pattern(key, block_size)
                    if actual == expected:
                        verified += 1
                    else:
                        errors.append(f"demoted key={key}: data integrity mismatch after cold read")
                else:
                    errors.append(
                        f"demoted key={key}: lookup failed with "
                        f"error_code={r.error_code} ({r.error_message})")

            print(f"    Cold-path verified: {verified}/{len(verify_keys)}")

            for ptr in lookup_ptrs:
                cuda_free(ptr)
        else:
            print("  Phase 5: Skipped (no demoted keys to verify)")

    except grpc.RpcError as e:
        errors.append(f"gRPC error: {e.code()} - {e.details()}")
    except SystemExit:
        pass
    finally:
        # === Cleanup ===
        print()
        print("  Cleaning up...")
        try:
            stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
        except grpc.RpcError:
            pass
        for ptr in pop_ptrs:
            cuda_free(ptr)
        channel.close()

    # === Summary ===
    print()
    print("=" * 60)
    print("Results")
    print("=" * 60)
    if errors:
        print()
        print("  Errors:")
        for e in errors:
            print(f"    - {e}")
        print()
        print("  FAIL: Memory-tier threshold eviction test failed")
        sys.exit(1)
    else:
        print()
        print("  PASS: Background memory-tier evictor working correctly")
        print(f"    - {len(demoted_keys)} entries proactively demoted from DRAM to SSD")
        if demoted_keys:
            print(f"    - Demoted entries verified accessible via cold (SSD) path")
        print()
        print("  Server must be configured with:")
        print("    --memory-tier-eviction-threshold <0.0-1.0>")
        print("    A memory-tier pool small enough that test entries exceed threshold")
        sys.exit(0)


if __name__ == "__main__":
    main()
