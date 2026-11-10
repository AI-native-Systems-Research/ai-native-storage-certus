#!/usr/bin/env python3
"""GPU Handle Test Client.

Allocates GPU memory, serializes the CUDA IPC handle as base64, and
sends it to the Rust server via Unix domain socket.

Uses CuPy if available, otherwise falls back to raw ctypes CUDA calls.
"""

import base64
import ctypes
import ctypes.util
import socket
import struct
import sys

SOCKET_PATH = "/tmp/gpu-services-ipc.sock"
BUFFER_SIZE = 4096


def allocate_cupy():
    """Allocate GPU memory and get IPC handle via CuPy."""
    import cupy as cp

    gpu_buf = cp.zeros(BUFFER_SIZE, dtype=cp.uint8)
    gpu_buf[:] = cp.arange(BUFFER_SIZE, dtype=cp.uint8)
    mem_handle = cp.cuda.runtime.ipcGetMemHandle(gpu_buf.data.ptr)
    return bytes(mem_handle), gpu_buf


def allocate_ctypes():
    """Allocate GPU memory and get IPC handle via ctypes CUDA runtime."""
    cudart = ctypes.CDLL("libcudart.so")

    devptr = ctypes.c_void_p()
    err = cudart.cudaMalloc(ctypes.byref(devptr), ctypes.c_size_t(BUFFER_SIZE))
    if err != 0:
        print(f"ERROR: cudaMalloc failed (error {err})")
        sys.exit(1)

    handle = (ctypes.c_byte * 64)()
    err = cudart.cudaIpcGetMemHandle(ctypes.byref(handle), devptr)
    if err != 0:
        cudart.cudaFree(devptr)
        print(f"ERROR: cudaIpcGetMemHandle failed (error {err})")
        sys.exit(1)

    return bytes(handle), (cudart, devptr)


def free_ctypes(ctx):
    """Free GPU memory allocated via ctypes."""
    cudart, devptr = ctx
    cudart.cudaFree(devptr)


def main():
    # Try CuPy first, fall back to ctypes
    backend = None
    handle_bytes = None
    ctx = None

    try:
        import cupy  # noqa: F401
        handle_bytes, ctx = allocate_cupy()
        backend = "cupy"
    except (ImportError, RuntimeError) as e:
        print(f"CuPy unavailable ({e}), using ctypes fallback")
        handle_bytes, ctx = allocate_ctypes()
        backend = "ctypes"

    print(f"Allocated {BUFFER_SIZE} bytes of GPU memory (backend: {backend})")
    print(f"IPC handle obtained ({len(handle_bytes)} bytes)")

    # Serialize: 64 bytes handle + 8 bytes LE u64 size
    payload_raw = handle_bytes + struct.pack("<Q", BUFFER_SIZE)
    assert len(payload_raw) == 72, f"Expected 72 bytes, got {len(payload_raw)}"

    payload_b64 = base64.b64encode(payload_raw)
    print(f"Base64 payload: {len(payload_b64)} bytes")

    # Connect to Rust server
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        try:
            sock.connect(SOCKET_PATH)
        except (ConnectionRefusedError, FileNotFoundError):
            print(f"ERROR: Cannot connect to {SOCKET_PATH}")
            print("       Start the server first: cargo run -p gpu-handle-test-server")
            sys.exit(1)
        print(f"Connected to {SOCKET_PATH}")

        # Send length-prefixed payload
        length_prefix = struct.pack("<I", len(payload_b64))
        sock.sendall(length_prefix + payload_b64)
        print("Payload sent, waiting for response...")

        # Read response
        status = sock.recv(1)
        if not status:
            print("ERROR: Server closed connection without response")
            sys.exit(1)

        if status == b"\x01":
            print("SUCCESS: Server acknowledged IPC handle")
        elif status == b"\x00":
            err_len_bytes = sock.recv(4)
            if len(err_len_bytes) == 4:
                err_len = struct.unpack("<I", err_len_bytes)[0]
                err_msg = sock.recv(err_len).decode("utf-8", errors="replace")
                print(f"ERROR from server: {err_msg}")
            else:
                print("ERROR: Server reported failure (no message)")
            sys.exit(1)
        else:
            print(f"ERROR: Unexpected response byte: {status.hex()}")
            sys.exit(1)

    finally:
        sock.close()

    # Free GPU memory
    if backend == "ctypes":
        free_ctypes(ctx)
    else:
        del ctx

    print("Done.")


if __name__ == "__main__":
    main()
