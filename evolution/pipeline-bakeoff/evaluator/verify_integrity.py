#!/usr/bin/env python3
"""Quick data integrity check for pipeline bakeoff.

Populates one 4 MiB object with a known non-zero pattern via GPU,
evicts it to SSD by filling the memory tier, then does a cold lookup
and verifies the returned data matches byte-for-byte.

Returns exit code 0 if data is correct, 1 if corrupted.
Prints JSON: {"integrity": "pass"|"fail", "detail": "..."}
"""
import ctypes
import json
import sys
import time

import grpc
import torch

_libcudart = ctypes.CDLL("libcudart.so")
_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]

sys.path.insert(0, "/home/nara/certus/ai-native-storage-certus/apps/python")
import dispatcher_pb2
import dispatcher_pb2_grpc

BLOCK_SIZE = 4 * 1024 * 1024  # 4 MiB
SERVER = "localhost:50051"
VERIFY_KEY_BASE = int(time.time()) % 100000 + 800000  # Time-based to avoid collisions


def _get_cuda_ipc_handle(data_ptr):
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), data_ptr)
    if err != 0:
        raise RuntimeError(f"cudaIpcGetMemHandle failed with error {err}")
    return bytes(handle_buf)


def main():
    channel = grpc.insecure_channel(
        SERVER,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    # Use a single tensor for both populate and lookup (same IPC handle, like the benchmark)
    num_elements = BLOCK_SIZE // 4
    tensor = torch.full(
        (num_elements,), 42, dtype=torch.int32, device="cuda:0"
    )
    handle = _get_cuda_ipc_handle(tensor.data_ptr())
    ipc = dispatcher_pb2.IpcHandle(cuda_ipc_handle=handle, size=BLOCK_SIZE)

    # Step 1: Populate our verification object with pattern value 42
    verify_key = VERIFY_KEY_BASE
    try:
        resp = stub.Populate(
            dispatcher_pb2.BatchPopulateRequest(
                entries=[dispatcher_pb2.PopulateEntry(key=verify_key, ipc_handle=ipc)]
            )
        )
        if resp.results and not resp.results[0].success:
            print(json.dumps({"integrity": "fail", "detail": f"Populate failed: {resp.results[0].error_message}"}))
            return 1
    except grpc.RpcError as e:
        print(json.dumps({"integrity": "fail", "detail": f"Populate RPC error: {e.details()}"}))
        return 1

    # Step 2: Evict to SSD by filling memory tier (populate 80 dummy objects to overflow 64-slot pool)
    dummy_tensor = torch.zeros(num_elements, dtype=torch.int32, device="cuda:0")
    dummy_handle = _get_cuda_ipc_handle(dummy_tensor.data_ptr())
    dummy_ipc = dispatcher_pb2.IpcHandle(cuda_ipc_handle=dummy_handle, size=BLOCK_SIZE)

    batch_size = 10
    for batch_start in range(0, 80, batch_size):
        entries = [
            dispatcher_pb2.PopulateEntry(key=VERIFY_KEY_BASE + 1000 + i, ipc_handle=dummy_ipc)
            for i in range(batch_start, min(batch_start + batch_size, 80))
        ]
        try:
            stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))
        except grpc.RpcError:
            pass

    # Wait for write-through to flush verification object to SSD
    time.sleep(5)

    # Step 3: Zero the tensor, then cold lookup — data should be restored from SSD
    tensor.fill_(0)
    torch.cuda.synchronize()

    try:
        resp = stub.Lookup(
            dispatcher_pb2.BatchLookupRequest(
                entries=[dispatcher_pb2.LookupEntry(key=verify_key, ipc_handle=ipc)]
            )
        )
        if resp.results and not resp.results[0].success:
            print(json.dumps({"integrity": "fail", "detail": f"Lookup failed: {resp.results[0].error_message}"}))
            return 1
    except grpc.RpcError as e:
        print(json.dumps({"integrity": "fail", "detail": f"Lookup RPC error: {e.details()}"}))
        return 1

    # Step 4: Verify data — tensor should now contain 42s again
    torch.cuda.synchronize()
    result_cpu = tensor.cpu()

    correct_count = (result_cpu == 42).sum().item()
    total = num_elements

    if correct_count == total:
        print(json.dumps({"integrity": "pass", "detail": "4 MiB cold lookup data verified (pattern=42, all correct)"}))
        return 0
    else:
        all_zero = (result_cpu == 0).all().item()
        detail = f"Data mismatch: {total - correct_count}/{total} elements wrong"
        if all_zero:
            detail += " (all zeros — DMA copy likely skipped)"
        elif correct_count > 0:
            detail += f" ({correct_count}/{total} correct — partial transfer)"
        print(json.dumps({"integrity": "fail", "detail": detail}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
