# SPDX-License-Identifier: Apache-2.0
"""CUDA IPC helpers for addressing KV-cache blocks over gRPC.

vLLM's KV cache is a single large device allocation; a block is a byte offset
into it. A CUDA IPC handle, however, always resolves to the *base* of the
containing allocation on the server side — the offset is not carried by the
handle. So we:

  1. take one IPC handle on the KV-cache tensor's allocation,
  2. find the true allocation base with ``cuMemGetAddressRange`` (the pointer
     the server's ``cudaIpcOpenMemHandle`` will resolve to), and
  3. send, per block, ``offset = (data_ptr - alloc_base) + block_id * stride``
     in the proto ``IpcHandle.offset`` field, which the server adds to the
     opened base before DMA.

If the KV-cache allocation is not IPC-exportable (some PyTorch caching-allocator
pointers are not), callers should fall back to a bounce-buffer pool; that path
is intentionally kept out of the manager/handler so only this module changes.
"""

from __future__ import annotations

import ctypes
from dataclasses import dataclass

# CUDA runtime (cudaIpcGetMemHandle) and driver (cuMemGetAddressRange) APIs.
_libcudart = ctypes.CDLL("libcudart.so")
_libcuda = ctypes.CDLL("libcuda.so")

_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_libcudart.cudaGetDevice.restype = ctypes.c_int
_libcudart.cudaGetDevice.argtypes = [ctypes.POINTER(ctypes.c_int)]

# Driver API: cuMemGetAddressRange(CUdeviceptr *base, size_t *size, CUdeviceptr dptr).
# CUdeviceptr is an unsigned integer the width of a pointer.
_libcuda.cuMemGetAddressRange_v2.restype = ctypes.c_int
_libcuda.cuMemGetAddressRange_v2.argtypes = [
    ctypes.POINTER(ctypes.c_ulonglong),
    ctypes.POINTER(ctypes.c_size_t),
    ctypes.c_ulonglong,
]

_IPC_HANDLE_BYTES = 64


@dataclass(frozen=True)
class KvCacheIpc:
    """One IPC handle for a KV-cache allocation, plus per-block offset math.

    ``handle_bytes`` is the 64-byte CUDA IPC handle for the allocation. The
    server opens it to the allocation base; ``block_offset(block_id)`` gives the
    byte offset from that base to the start of the block, accounting for the
    gap between the allocation base and the tensor's ``data_ptr``.
    """

    handle_bytes: bytes
    gpu_device_id: int
    stride_bytes: int
    base_delta: int  # data_ptr - alloc_base

    def block_offset(self, block_id: int) -> int:
        return self.base_delta + block_id * self.stride_bytes


def current_device() -> int:
    dev = ctypes.c_int()
    err = _libcudart.cudaGetDevice(ctypes.byref(dev))
    if err != 0:
        raise RuntimeError(f"cudaGetDevice failed: {err}")
    return dev.value


def _alloc_base(data_ptr: int) -> int:
    """Return the base device pointer of the allocation containing data_ptr."""
    base = ctypes.c_ulonglong(0)
    size = ctypes.c_size_t(0)
    err = _libcuda.cuMemGetAddressRange_v2(
        ctypes.byref(base), ctypes.byref(size), ctypes.c_ulonglong(data_ptr)
    )
    if err != 0:
        raise RuntimeError(f"cuMemGetAddressRange failed: {err}")
    return base.value


def _ipc_handle(base_ptr: int) -> bytes:
    """cudaIpcGetMemHandle on an allocation base pointer -> 64 raw bytes."""
    buf = (ctypes.c_ubyte * _IPC_HANDLE_BYTES)()
    err = _libcudart.cudaIpcGetMemHandle(
        ctypes.byref(buf), ctypes.c_void_p(base_ptr)
    )
    if err != 0:
        raise RuntimeError(
            f"cudaIpcGetMemHandle failed: {err} "
            "(KV-cache allocation may not be IPC-exportable; a bounce-buffer "
            "fallback is required)"
        )
    return bytes(buf)


def ipc_for_tensor(data_ptr: int, stride_bytes: int, gpu_device_id: int) -> KvCacheIpc:
    """Build a KvCacheIpc for a KV-cache tensor.

    ``data_ptr`` is the tensor base (``tensor.data_ptr()``); ``stride_bytes`` is
    the per-block stride (``tensor.stride(0) * tensor.element_size()``). We
    resolve the enclosing allocation base, take the IPC handle on *that* base,
    and record the delta so per-block offsets are relative to what the server
    resolves the handle to.
    """
    alloc_base = _alloc_base(data_ptr)
    handle = _ipc_handle(alloc_base)
    return KvCacheIpc(
        handle_bytes=handle,
        gpu_device_id=gpu_device_id,
        stride_bytes=stride_bytes,
        base_delta=data_ptr - alloc_base,
    )
