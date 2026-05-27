#!/usr/bin/env python3
"""GPU P2P client: allocate GPU memory, export CUDA IPC handle to server.

Usage:
  python3 gpu_client_p2p.py <size_bytes> [<socket_path>] [--iterations N]

This script simulates a PyTorch/CUDA application that:
1. Allocates GPU device memory (cudaMalloc)
2. Exports the CUDA IPC handle
3. Sends handle + size to the P2P server which performs NVMe DMA

Modes:
  - Without socket_path: writes base64 payload to stdout, blocks on stdin (subprocess mode)
  - With socket_path: connects to Unix socket, sends payload, awaits response (server mode)

Benchmark mode (--iterations N):
  Repeats the transfer N times, reports per-transfer latency and throughput.

Payload format (single line, base64-encoded):
    cuda_ipc_handle[64] + size_le[8] = 72 bytes
"""

import sys
import time
import base64
import struct
import ctypes
import ctypes.util
import socket


def load_cudart():
    for name in ["libcudart.so", "libcudart.so.12", "libcudart.so.11"]:
        try:
            return ctypes.CDLL(name)
        except OSError:
            continue
    path = ctypes.util.find_library("cudart")
    if path:
        return ctypes.CDLL(path)
    return None


def do_transfer(socket_path, b64):
    """Connect, send payload, receive response. Returns (response_str, elapsed_sec)."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(socket_path)

    t0 = time.perf_counter()
    sock.sendall((b64 + "\n").encode("ascii"))

    response = b""
    while b"\n" not in response:
        chunk = sock.recv(4096)
        if not chunk:
            break
        response += chunk
    elapsed = time.perf_counter() - t0

    sock.close()
    return response.decode("ascii").strip(), elapsed


def main():
    args = sys.argv[1:]
    iterations = 1

    if "--iterations" in args:
        idx = args.index("--iterations")
        iterations = int(args[idx + 1])
        args = args[:idx] + args[idx + 2:]

    if len(args) < 1:
        print(f"usage: {sys.argv[0]} <size_bytes> [<socket_path>] [--iterations N]", file=sys.stderr)
        sys.exit(1)

    size = int(args[0])
    socket_path = args[1] if len(args) > 1 else None

    assert size > 0, "size must be positive"

    cuda = load_cudart()
    if cuda is None:
        print("ERROR: cannot load libcudart", file=sys.stderr)
        sys.exit(1)

    # Step 1: cudaMalloc
    dev_ptr = ctypes.c_void_p(0)
    err = cuda.cudaMalloc(ctypes.byref(dev_ptr), ctypes.c_size_t(size))
    if err != 0:
        print(f"ERROR: cudaMalloc failed (err={err})", file=sys.stderr)
        sys.exit(1)

    print(f"cudaMalloc: dev_ptr=0x{dev_ptr.value:x}, size={size}", file=sys.stderr)

    # Step 2: cudaIpcGetMemHandle
    ipc_handle = (ctypes.c_ubyte * 64)()
    err = cuda.cudaIpcGetMemHandle(ctypes.byref(ipc_handle), dev_ptr)
    if err != 0:
        print(f"ERROR: cudaIpcGetMemHandle failed (err={err})", file=sys.stderr)
        cuda.cudaFree(dev_ptr)
        sys.exit(1)

    # Step 3: Export payload.
    payload = bytes(ipc_handle) + struct.pack("<Q", size)
    b64 = base64.b64encode(payload).decode("ascii")

    if socket_path:
        if iterations == 1:
            resp, elapsed = do_transfer(socket_path, b64)
            size_mb = size / (1024 * 1024)
            throughput = size_mb / elapsed
            print(f"server response: {resp}", file=sys.stderr)
            print(f"latency: {elapsed*1000:.2f} ms, throughput: {throughput:.1f} MB/s ({size_mb:.2f} MB)", file=sys.stderr)
        else:
            # Benchmark mode: warmup + timed iterations.
            print(f"Benchmarking {iterations} x {size/(1024*1024):.2f} MB transfers...", file=sys.stderr)

            # Warmup
            resp, _ = do_transfer(socket_path, b64)
            if not resp.startswith("OK"):
                print(f"ERROR on warmup: {resp}", file=sys.stderr)
                cuda.cudaFree(dev_ptr)
                sys.exit(1)

            latencies = []
            for i in range(iterations):
                resp, elapsed = do_transfer(socket_path, b64)
                if not resp.startswith("OK"):
                    print(f"ERROR on iteration {i}: {resp}", file=sys.stderr)
                    cuda.cudaFree(dev_ptr)
                    sys.exit(1)
                latencies.append(elapsed)

            size_mb = size / (1024 * 1024)
            total_time = sum(latencies)
            avg_lat = total_time / len(latencies)
            min_lat = min(latencies)
            max_lat = max(latencies)
            total_mb = size_mb * iterations
            throughput = total_mb / total_time

            print(f"\n{'='*60}", file=sys.stderr)
            print(f"  GPU P2P DMA Benchmark: {size_mb:.2f} MB x {iterations} iterations", file=sys.stderr)
            print(f"{'='*60}", file=sys.stderr)
            print(f"  Throughput:    {throughput:.1f} MB/s", file=sys.stderr)
            print(f"  Avg latency:   {avg_lat*1000:.2f} ms", file=sys.stderr)
            print(f"  Min latency:   {min_lat*1000:.2f} ms", file=sys.stderr)
            print(f"  Max latency:   {max_lat*1000:.2f} ms", file=sys.stderr)
            print(f"  Total data:    {total_mb:.1f} MB in {total_time:.3f} s", file=sys.stderr)
            print(f"{'='*60}\n", file=sys.stderr)
    else:
        # Subprocess mode: write to stdout, block on stdin.
        print(b64, flush=True)
        try:
            sys.stdin.read()
        except (KeyboardInterrupt, EOFError):
            pass

    # Cleanup
    cuda.cudaFree(dev_ptr)


if __name__ == "__main__":
    main()
