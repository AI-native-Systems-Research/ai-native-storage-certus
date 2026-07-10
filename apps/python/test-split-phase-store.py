#!/usr/bin/env python3
"""Hardware integration test for split-phase store APIs.

Tests the Reserve → CopyToStore → CommitStore lifecycle and the
AbortStore cancellation path. Verifies:
  1. Reserve allocates slots (Check returns false — not yet visible)
  2. CopyToStore transfers data from GPU into reserved DRAM slot
  3. CommitStore makes the entry visible (Check returns true)
  4. Lookup after commit returns correct data (integrity check)
  5. AbortStore cancels a reserved slot (entry never becomes visible)
  6. Double-commit is rejected (AlreadyExists or KeyNotFound)
  7. CommitStore without prior Reserve is rejected (KeyNotFound)

Usage:
    python test-split-phase-store.py --server localhost:50051 --block-size 64K --num-objects 8
"""

import argparse
import ctypes
import os
import random
import sys

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
_libcudart.cudaMemcpy.argtypes = [
    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int
]
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
    buf = (ctypes.c_ubyte * size)()
    err = _libcudart.cudaMemcpy(ctypes.byref(buf), dev_ptr, size, _CUDA_MEMCPY_D2H)
    if err != 0:
        raise RuntimeError(f"cudaMemcpy D2H failed: {err}")
    return bytes(buf)


def make_pattern(key, size):
    rng = random.Random(key)
    return bytes(rng.getrandbits(8) for _ in range(size))


def parse_size(s):
    s = s.strip()
    suffix = s[-1].upper()
    multipliers = {"K": 1024, "M": 1024 * 1024, "G": 1024 * 1024 * 1024}
    if suffix in multipliers:
        return int(s[:-1]) * multipliers[suffix]
    return int(s)


# --- Test helpers ---

def check_exists(stub, keys):
    resp = stub.Check(dispatcher_pb2.BatchCheckRequest(keys=keys))
    return {r.key: r.exists for r in resp.results}


def assert_all_success(resp, op_name):
    for r in resp.results:
        if not r.success:
            raise AssertionError(
                f"{op_name} failed for key={r.key}: {r.error_message}"
            )


def assert_result_error(resp, key, expected_code, op_name):
    for r in resp.results:
        if r.key == key:
            if r.success:
                raise AssertionError(
                    f"{op_name} key={key}: expected error {expected_code}, got success"
                )
            return
    raise AssertionError(f"{op_name}: key={key} not found in results")


# --- Tests ---

def test_happy_path(stub, keys, block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles):
    """Reserve → CopyToStore → CommitStore → Lookup with integrity check."""
    print("\n  [TEST] Happy path: Reserve → CopyToStore → CommitStore → Lookup")

    # Write unique patterns to GPU
    patterns = {}
    for i, key in enumerate(keys):
        pattern = make_pattern(key, block_size)
        patterns[key] = pattern
        gpu_write(pop_ptrs[i], pattern)

    # Step 1: Reserve
    entries = [
        dispatcher_pb2.ReserveEntry(key=k, size=block_size)
        for k in keys
    ]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve")
    print("    Reserve:     OK")

    # Step 2: Verify NOT visible yet
    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after Reserve (should not be)")
    print("    Not visible: OK (entries not in dispatch-map yet)")

    # Step 3: CopyToStore
    copy_entries = [
        dispatcher_pb2.CopyToStoreEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=pop_handles[i],
                size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=copy_entries))
    assert_all_success(resp, "CopyToStore")
    print("    CopyToStore: OK")

    # Step 4: Still NOT visible (DMA done, but not committed)
    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after CopyToStore (should not be)")
    print("    Still hidden: OK (not committed yet)")

    # Step 5: CommitStore
    resp = stub.CommitStore(dispatcher_pb2.BatchCommitStoreRequest(keys=keys))
    assert_all_success(resp, "CommitStore")
    print("    CommitStore: OK")

    # Step 6: Now visible
    exists = check_exists(stub, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} not visible after CommitStore")
    print("    Visible:     OK (entries in dispatch-map)")

    # Step 7: Lookup and verify data integrity
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
    resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=lookup_entries))
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(resp, "Lookup")

    integrity_ok = True
    for i, k in enumerate(keys):
        actual = gpu_read(lookup_ptrs[i], block_size)
        if actual != patterns[k]:
            first_bad = next(
                (j for j in range(len(actual)) if actual[j] != patterns[k][j]), "?"
            )
            print(f"    INTEGRITY FAIL: key={k}, first mismatch at byte {first_bad}")
            integrity_ok = False

    if integrity_ok:
        print(f"    Integrity:   OK ({len(keys)} objects verified)")
    else:
        raise AssertionError("Data integrity check failed")

    # Cleanup
    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:     OK")


def test_abort_path(stub, keys, block_size, pop_ptrs, pop_handles):
    """Reserve → AbortStore → verify entry never becomes visible."""
    print("\n  [TEST] Abort path: Reserve → AbortStore → verify invisible")

    # Write data to GPU
    for i, key in enumerate(keys):
        gpu_write(pop_ptrs[i], make_pattern(key, block_size))

    # Reserve
    entries = [
        dispatcher_pb2.ReserveEntry(key=k, size=block_size)
        for k in keys
    ]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve")
    print("    Reserve:     OK")

    # CopyToStore (data in DRAM but not committed)
    copy_entries = [
        dispatcher_pb2.CopyToStoreEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=pop_handles[i],
                size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=copy_entries))
    assert_all_success(resp, "CopyToStore")
    print("    CopyToStore: OK")

    # Abort instead of commit
    resp = stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=keys))
    assert_all_success(resp, "AbortStore")
    print("    AbortStore:  OK")

    # Verify NOT visible
    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after AbortStore (should not be)")
    print("    Invisible:   OK (entries correctly discarded)")


def test_abort_without_copy(stub, keys, block_size):
    """Reserve → AbortStore (skip CopyToStore) → verify invisible."""
    print("\n  [TEST] Abort without copy: Reserve → AbortStore (no DMA)")

    entries = [
        dispatcher_pb2.ReserveEntry(key=k, size=block_size)
        for k in keys
    ]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve")
    print("    Reserve:     OK")

    resp = stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=keys))
    assert_all_success(resp, "AbortStore")
    print("    AbortStore:  OK")

    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after AbortStore")
    print("    Invisible:   OK")


def test_commit_without_reserve(stub, block_size):
    """CommitStore without prior Reserve should fail with KeyNotFound."""
    print("\n  [TEST] CommitStore without Reserve → expect KeyNotFound")

    bogus_key = 0xDEAD_BEEF_0001
    resp = stub.CommitStore(dispatcher_pb2.BatchCommitStoreRequest(keys=[bogus_key]))
    assert_result_error(resp, bogus_key, "KEY_NOT_FOUND", "CommitStore")
    print("    CommitStore: correctly rejected (KeyNotFound)")


def test_double_reserve(stub, block_size):
    """Reserve same key twice → second should fail with AlreadyExists."""
    print("\n  [TEST] Double Reserve → expect AlreadyExists")

    key = 0xDEAD_BEEF_0002
    entry = [dispatcher_pb2.ReserveEntry(key=key, size=block_size)]

    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entry))
    assert_all_success(resp, "Reserve (first)")
    print("    First Reserve:  OK")

    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entry))
    assert_result_error(resp, key, "ALREADY_EXISTS", "Reserve (second)")
    print("    Second Reserve: correctly rejected (AlreadyExists)")

    # Cleanup
    stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=[key]))
    print("    Cleanup:        OK")


def test_reserve_after_abort_reuse(stub, keys, block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles):
    """Reserve → Abort → Reserve again → full lifecycle (slot reuse)."""
    print("\n  [TEST] Slot reuse: Reserve → Abort → Reserve → CopyToStore → Commit")

    for i, key in enumerate(keys):
        gpu_write(pop_ptrs[i], make_pattern(key, block_size))

    # First reserve + abort
    entries = [dispatcher_pb2.ReserveEntry(key=k, size=block_size) for k in keys]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve (first)")
    resp = stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=keys))
    assert_all_success(resp, "AbortStore")
    print("    Reserve+Abort:  OK")

    # Second reserve + full lifecycle
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve (second)")

    copy_entries = [
        dispatcher_pb2.CopyToStoreEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=pop_handles[i],
                size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=copy_entries))
    assert_all_success(resp, "CopyToStore")

    resp = stub.CommitStore(dispatcher_pb2.BatchCommitStoreRequest(keys=keys))
    assert_all_success(resp, "CommitStore")
    print("    Re-Reserve+Commit: OK")

    # Verify visible + integrity
    exists = check_exists(stub, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} not visible after re-reserve+commit")

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
    resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=lookup_entries))
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(resp, "Lookup")

    for i, k in enumerate(keys):
        actual = gpu_read(lookup_ptrs[i], block_size)
        expected = make_pattern(k, block_size)
        if actual != expected:
            raise AssertionError(f"Integrity fail after slot reuse: key={k}")
    print("    Integrity:      OK")

    # Cleanup
    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:        OK")


def main():
    parser = argparse.ArgumentParser(
        description="Integration test for split-phase store APIs"
    )
    parser.add_argument("--server", default="localhost:50051")
    parser.add_argument(
        "--block-size", type=parse_size, default=64 * 1024,
        help="Object size (default: 64K)"
    )
    parser.add_argument(
        "--num-objects", type=int, default=8,
        help="Number of objects per test (default: 8)"
    )
    parser.add_argument("--gpu", type=int, default=0, help="GPU device index")
    args = parser.parse_args()

    block_size = args.block_size
    num_objects = args.num_objects
    base_key = 80_000_000

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 60)
    print("Split-Phase Store API Integration Test")
    print("=" * 60)
    print(f"  Server:      {args.server}")
    print(f"  Block size:  {block_size // 1024} KiB")
    print(f"  Objects:     {num_objects}")
    print(f"  GPU:         {args.gpu}")

    # Allocate GPU buffers
    pop_ptrs, pop_handles = [], []
    lookup_ptrs, lookup_handles = [], []
    for _ in range(num_objects):
        ptr, handle = cuda_alloc(block_size)
        pop_ptrs.append(ptr)
        pop_handles.append(handle)
        ptr, handle = cuda_alloc(block_size)
        lookup_ptrs.append(ptr)
        lookup_handles.append(handle)

    # Each test gets its own key range to avoid state leakage from
    # background write-through between tests.
    key_sets = [
        [base_key + (t * num_objects) + i for i in range(num_objects)]
        for t in range(6)
    ]

    # Connect
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
    tests = [
        ("happy_path", lambda: test_happy_path(
            stub, key_sets[0], block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles
        )),
        ("abort_path", lambda: test_abort_path(
            stub, key_sets[1], block_size, pop_ptrs, pop_handles
        )),
        ("abort_without_copy", lambda: test_abort_without_copy(
            stub, key_sets[2], block_size
        )),
        ("commit_without_reserve", lambda: test_commit_without_reserve(
            stub, block_size
        )),
        ("double_reserve", lambda: test_double_reserve(stub, block_size)),
        ("slot_reuse", lambda: test_reserve_after_abort_reuse(
            stub, key_sets[5], block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles
        )),
    ]

    all_keys = [k for ks in key_sets for k in ks]
    for name, test_fn in tests:
        try:
            test_fn()
            passed += 1
        except (AssertionError, grpc.RpcError) as e:
            print(f"    FAILED: {e}")
            failed += 1
            # Attempt cleanup on failure
            try:
                stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=all_keys))
            except Exception:
                pass
            try:
                stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=all_keys))
            except Exception:
                pass

    # Final cleanup
    for ptr in pop_ptrs:
        cuda_free(ptr)
    for ptr in lookup_ptrs:
        cuda_free(ptr)
    channel.close()

    # Summary
    print("\n" + "=" * 60)
    print(f"Results: {passed} passed, {failed} failed, {passed + failed} total")
    print("=" * 60)

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()
