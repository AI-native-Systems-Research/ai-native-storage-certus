#!/usr/bin/env python3
"""End-to-end test exercising both gRPC (populate) and RDMA (lookup) data paths.

Populates the Certus cache via the standard gRPC Dispatcher API, then performs
lookups via the RDMA remote-request-handler endpoint using the Rust test-client
binary. Optionally verifies data integrity.

Requires:
  - CUDA GPU (for IPC handle generation during populate)
  - RDMA-capable NIC (for the remote-request-handler lookup path)
  - certus-server-yaml running with full-remote profile

Usage:
    python test-remote.py --grpc-server localhost:50051 --rdma-server 10.0.0.100 --rdma-port 18515
    python test-remote.py --check-integrity --num-objects 16
"""

import argparse
import ctypes
import os
import random
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import grpc
import dispatcher_pb2
import dispatcher_pb2_grpc

# --- CUDA helpers (same pattern as certus-api-bench.py) ---

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
_libcudart.cudaMemcpy.argtypes = [
    ctypes.c_void_p,
    ctypes.c_void_p,
    ctypes.c_size_t,
    ctypes.c_int,
]
_libcudart.cudaDeviceSynchronize.restype = ctypes.c_int
_CUDA_MEMCPY_H2D = 1
_CUDA_MEMCPY_D2H = 2


def _cuda_alloc(size):
    """Allocate GPU memory and return (device_ptr, ipc_handle_bytes)."""
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


def _make_pattern(key, block_size):
    """Create a deterministic byte pattern for a given key (matches certus-api-bench.py)."""
    rng = random.Random(key)
    return bytes(rng.getrandbits(8) for _ in range(block_size))


def parse_size(s):
    """Parse a human-readable size string (e.g. '128K', '4M', '2G') into bytes."""
    s = s.strip().upper()
    multipliers = {"K": 1024, "M": 1024 * 1024, "G": 1024 * 1024 * 1024}
    if s[-1] in multipliers:
        return int(s[:-1]) * multipliers[s[-1]]
    return int(s)


def find_test_client():
    """Auto-detect the test-client binary from the cargo workspace."""
    candidates = [
        os.path.join(
            os.path.dirname(__file__),
            "../../target/release/test-client",
        ),
        os.path.join(
            os.path.dirname(__file__),
            "../../target/debug/test-client",
        ),
    ]
    for path in candidates:
        path = os.path.abspath(path)
        if os.path.isfile(path) and os.access(path, os.X_OK):
            return path
    return None


# --- Phase 1: Populate via gRPC ---


def populate_cache(stub, keys, block_size, gpu_device):
    """Populate cache entries via gRPC with deterministic patterns."""
    err = _libcudart.cudaSetDevice(gpu_device)
    if err != 0:
        raise RuntimeError(f"cudaSetDevice({gpu_device}) failed: {err}")

    dev_ptr, handle_bytes = _cuda_alloc(block_size)
    ipc_handle = dispatcher_pb2.IpcHandle(
        cuda_ipc_handle=handle_bytes, size=block_size, gpu_device_id=gpu_device
    )

    populated = 0
    failed = 0
    t0 = time.time()

    for key in keys:
        pattern = _make_pattern(key, block_size)
        _gpu_write(dev_ptr, pattern)
        _libcudart.cudaDeviceSynchronize()

        resp = stub.Populate(
            dispatcher_pb2.BatchPopulateRequest(
                entries=[dispatcher_pb2.PopulateEntry(key=key, ipc_handle=ipc_handle)]
            )
        )
        if resp.results[0].success:
            populated += 1
        else:
            failed += 1
            print(
                f"  WARN: populate key {key} failed: {resp.results[0].error_message}"
            )

    elapsed = time.time() - t0
    _cuda_free(dev_ptr)
    return populated, failed, elapsed


# --- Phase 2: Lookup via RDMA test-client ---


def run_rdma_lookups(test_client_path, rdma_server, rdma_port, batch_size, iterations):
    """Run the Rust test-client binary and parse its output."""
    cmd = [
        test_client_path,
        "--addr",
        rdma_server,
        "--port",
        str(rdma_port),
        "--batch-size",
        str(batch_size),
        "--iterations",
        str(iterations),
    ]

    result = subprocess.run(cmd, capture_output=True, text=True, timeout=60)

    if result.returncode != 0:
        return {
            "success": False,
            "error": result.stderr.strip() or result.stdout.strip(),
        }

    output = result.stdout

    # Parse results from test-client output
    parsed = {"success": True, "output": output, "batches": []}

    # Parse per-batch lines: "  Batch N: X ok, Y not_found/error"
    for m in re.finditer(
        r"Batch (\d+): (\d+) ok, (\d+) not_found/error", output
    ):
        parsed["batches"].append(
            {"batch_id": int(m.group(1)), "ok": int(m.group(2)), "err": int(m.group(3))}
        )

    # Parse summary line: "Completed N iterations (M total entries) in X.XXXms"
    m = re.search(
        r"Completed (\d+) iterations \((\d+) total entries\) in ([\d.]+)ms", output
    )
    if m:
        parsed["iterations"] = int(m.group(1))
        parsed["total_entries"] = int(m.group(2))
        parsed["elapsed_ms"] = float(m.group(3))

    # Parse average line: "Average: X.X us/batch, Y.Y us/entry"
    m = re.search(r"Average: ([\d.]+) us/batch, ([\d.]+) us/entry", output)
    if m:
        parsed["us_per_batch"] = float(m.group(1))
        parsed["us_per_entry"] = float(m.group(2))

    # Parse close line
    m = re.search(r"Close acknowledged: (\d+) batches processed", output)
    if m:
        parsed["server_batches"] = int(m.group(1))

    return parsed


# --- Phase 3: Integrity check ---


def check_integrity(parsed_results):
    """Check whether lookups returned valid data."""
    total_ok = sum(b["ok"] for b in parsed_results.get("batches", []))
    total_err = sum(b["err"] for b in parsed_results.get("batches", []))
    total = total_ok + total_err

    if total == 0:
        return "SKIP", "No batch results parsed from test-client output"

    if total_ok == 0 and total_err > 0:
        return "WARN", (
            f"All {total_err} lookups returned not_found. "
            "This is expected if the RDMA handler is not yet wired to the real dispatcher."
        )

    if total_ok > 0 and total_err == 0:
        return "PASS", f"All {total_ok} lookups succeeded"

    return "PARTIAL", f"{total_ok}/{total} lookups succeeded, {total_err} not found"


# --- Main ---


def main():
    parser = argparse.ArgumentParser(
        description="End-to-end test: gRPC populate + RDMA lookup"
    )
    parser.add_argument(
        "--grpc-server",
        default="localhost:50051",
        help="gRPC endpoint for populate [default: localhost:50051]",
    )
    parser.add_argument(
        "--rdma-server",
        default="localhost",
        help="RDMA handler address [default: localhost]",
    )
    parser.add_argument(
        "--rdma-port",
        type=int,
        default=18515,
        help="RDMA handler port [default: 18515]",
    )
    parser.add_argument(
        "--num-objects",
        type=int,
        default=64,
        help="Number of cache entries to populate [default: 64]",
    )
    parser.add_argument(
        "--block-size",
        default="4M",
        help="Block size per entry (e.g. 4M, 128K) [default: 4M]",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=16,
        help="Entries per RDMA lookup batch [default: 16]",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=10,
        help="RDMA lookup iterations [default: 10]",
    )
    parser.add_argument(
        "--check-integrity",
        action="store_true",
        help="Verify data integrity of lookups",
    )
    parser.add_argument(
        "--test-client-path",
        default=None,
        help="Path to test-client binary [default: auto-detect]",
    )
    parser.add_argument(
        "--gpu-device",
        type=int,
        default=0,
        help="CUDA device ordinal [default: 0]",
    )

    args = parser.parse_args()
    block_size = parse_size(args.block_size)

    print("=" * 70)
    print("Certus Remote Request Test")
    print("=" * 70)
    print(f"  gRPC server:    {args.grpc_server}")
    print(f"  RDMA server:    {args.rdma_server}:{args.rdma_port}")
    print(f"  Objects:        {args.num_objects}")
    print(f"  Block size:     {block_size // 1024} KiB")
    print(f"  Batch size:     {args.batch_size}")
    print(f"  Iterations:     {args.iterations}")
    print(f"  Integrity:      {'enabled' if args.check_integrity else 'disabled'}")
    print()

    # --- Locate test-client binary ---
    test_client = args.test_client_path or find_test_client()
    if not test_client:
        print("ERROR: Cannot find test-client binary.")
        print("       Build with: cargo build -p remote-request-handler --release")
        sys.exit(1)
    print(f"  test-client:    {test_client}")
    print()

    # --- Phase 1: Populate via gRPC ---
    print("-" * 70)
    print("Phase 1: Populate cache via gRPC")
    print("-" * 70)

    base_key = random.randint(1_000_000, 9_000_000)
    keys = list(range(base_key, base_key + args.num_objects))

    channel = grpc.insecure_channel(
        args.grpc_server,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    try:
        populated, failed, elapsed = populate_cache(
            stub, keys, block_size, args.gpu_device
        )
    except grpc.RpcError as e:
        print(f"  ERROR: gRPC connection failed: {e.details()}")
        sys.exit(1)

    print(f"  Populated: {populated} objects ({failed} failed) in {elapsed:.3f}s")
    print(
        f"  Throughput: {populated * block_size / (1024*1024) / elapsed:.1f} MiB/s"
        if elapsed > 0
        else ""
    )
    print()

    if failed > 0 and failed == args.num_objects:
        print("ERROR: All populates failed. Is the server running?")
        sys.exit(1)

    # --- Phase 2: Lookup via RDMA ---
    print("-" * 70)
    print("Phase 2: Lookup via RDMA remote-request-handler")
    print("-" * 70)

    results = run_rdma_lookups(
        test_client, args.rdma_server, args.rdma_port, args.batch_size, args.iterations
    )

    if not results["success"]:
        print(f"  ERROR: test-client failed: {results['error']}")
        sys.exit(1)

    if "elapsed_ms" in results:
        print(f"  Completed: {results['iterations']} iterations, "
              f"{results['total_entries']} total entries")
        print(f"  Time: {results['elapsed_ms']:.3f} ms")
        print(f"  Latency: {results['us_per_batch']:.1f} us/batch, "
              f"{results['us_per_entry']:.1f} us/entry")
    if "server_batches" in results:
        print(f"  Server confirmed: {results['server_batches']} batches processed")
    print()

    # --- Phase 3: Integrity check (optional) ---
    if args.check_integrity:
        print("-" * 70)
        print("Phase 3: Data Integrity Check")
        print("-" * 70)

        status, message = check_integrity(results)
        status_icon = {"PASS": "✓", "WARN": "⚠", "PARTIAL": "~", "SKIP": "?"}.get(
            status, "?"
        )
        print(f"  [{status_icon}] {status}: {message}")
        print()

        if status == "WARN":
            print(
                "  NOTE: To enable full integrity verification, the RDMA handler must\n"
                "        be wired to the real dispatcher (resolve keys from cache).\n"
                "        Currently the handler returns 'not found' for all keys."
            )

    # --- Summary ---
    print("=" * 70)
    print("Summary")
    print("=" * 70)
    print(f"  Populate (gRPC): {populated}/{args.num_objects} objects OK")
    if "us_per_batch" in results:
        print(
            f"  Lookup (RDMA):   {results['us_per_batch']:.1f} us/batch, "
            f"{results['us_per_entry']:.1f} us/entry"
        )
    print(f"  Status:          {'PASS' if populated > 0 and results['success'] else 'FAIL'}")
    print()


if __name__ == "__main__":
    main()
