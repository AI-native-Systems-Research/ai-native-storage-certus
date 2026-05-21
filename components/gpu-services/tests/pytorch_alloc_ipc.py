#!/usr/bin/env python3
"""Allocate GPU memory via PyTorch and export a CUDA IPC handle.

Usage: python3 pytorch_alloc_ipc.py <size_bytes>

Outputs a single line to stdout: base64-encoded payload containing
the 64-byte cudaIpcMemHandle_t followed by an 8-byte little-endian size.
(Total 72 bytes before base64 encoding.)

The process stays alive (blocking on stdin) so the GPU allocation remains
valid for the Rust test to open via cudaIpcOpenMemHandle. Send any input
or close stdin to terminate.
"""

import sys
import os
import base64
import struct
import ctypes
import ctypes.util

def main():
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <size_bytes>", file=sys.stderr)
        sys.exit(1)

    size = int(sys.argv[1])
    assert size > 0, "size must be positive"

    # Load CUDA runtime.
    cuda_lib = None
    for name in ["libcudart.so", "libcudart.so.12", "libcudart.so.11"]:
        try:
            cuda_lib = ctypes.CDLL(name)
            break
        except OSError:
            continue
    if cuda_lib is None:
        cuda_path = ctypes.util.find_library("cudart")
        if cuda_path:
            cuda_lib = ctypes.CDLL(cuda_path)
    if cuda_lib is None:
        print("ERROR: cannot load libcudart", file=sys.stderr)
        sys.exit(1)

    # cudaMalloc
    dev_ptr = ctypes.c_void_p(0)
    err = cuda_lib.cudaMalloc(ctypes.byref(dev_ptr), ctypes.c_size_t(size))
    if err != 0:
        print(f"ERROR: cudaMalloc failed (err={err})", file=sys.stderr)
        sys.exit(1)

    # cudaIpcGetMemHandle — handle is 64 bytes opaque
    handle = (ctypes.c_ubyte * 64)()
    err = cuda_lib.cudaIpcGetMemHandle(ctypes.byref(handle), dev_ptr)
    if err != 0:
        print(f"ERROR: cudaIpcGetMemHandle failed (err={err})", file=sys.stderr)
        sys.exit(1)

    # Encode: 64 bytes handle + 8 bytes size (little-endian u64) = 72 bytes
    payload = bytes(handle) + struct.pack("<Q", size)
    b64 = base64.b64encode(payload).decode("ascii")

    # Output the base64 payload (Rust test reads this).
    print(b64, flush=True)

    # Keep process alive so the CUDA allocation remains valid.
    # The Rust test will close our stdin (or kill us) when done.
    try:
        sys.stdin.read()
    except (KeyboardInterrupt, EOFError):
        pass

    # Cleanup (process exit also frees, but be explicit).
    cuda_lib.cudaFree(dev_ptr)

if __name__ == "__main__":
    main()
