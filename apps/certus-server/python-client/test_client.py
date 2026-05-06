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


def main():
    parser = argparse.ArgumentParser(description="Certus gRPC dispatcher test client")
    parser.add_argument(
        "--server", default="localhost:50051", help="Server address (default: localhost:50051)"
    )
    parser.add_argument(
        "--skip-large-batch", action="store_true", help="Skip the 1000-entry large batch test"
    )
    args = parser.parse_args()

    print(f"Testing certus-server gRPC dispatcher at {args.server}...")

    channel = grpc.insecure_channel(args.server)
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

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

    if results.summary():
        print("All tests passed.")
        sys.exit(0)
    else:
        sys.exit(1)


if __name__ == "__main__":
    main()
