"""
Certus Storage Backend for KVCache Benchmark

Implements the Storage ABC using the certus-server shmq transport.
Data moves GPU<->DRAM<->SSD via CUDA IPC handles; the shmq ring carries
only the control-plane opcode (~68 bytes per request).

GPU buffer pool is allocated once at init and reused across ops, matching
the pattern certus-fio uses. This is how a real inference engine (vLLM)
uses Certus — the KV-cache tensor is a single long-lived GPU allocation.
"""

import ctypes
import os
import sys
import time
from pathlib import Path
from typing import Dict, Any

from .interface import Storage

# ── Locate certus-shmq-connector and helpers ──

_THIS_DIR = Path(__file__).resolve().parent.parent
_REPO_ROOT = _THIS_DIR.parent.parent
_CONNECTOR_ROOT = _REPO_ROOT / "certus-shmq-connector"
_APPS_PYTHON = _REPO_ROOT / "apps" / "python"

if str(_CONNECTOR_ROOT) not in sys.path:
    sys.path.insert(0, str(_CONNECTOR_ROOT))
if str(_APPS_PYTHON) not in sys.path:
    sys.path.insert(0, str(_APPS_PYTHON))

from certus_shmq_helpers import Ring, RingError, connect, single_region  # noqa: E402


# ── CUDA helpers (raw cudaMalloc, NOT PyTorch — same as certus-fio) ──

_libcudart = None


def _load_cudart():
    global _libcudart
    if _libcudart is not None:
        return
    lib = ctypes.CDLL("libcudart.so")
    lib.cudaSetDevice.restype = ctypes.c_int
    lib.cudaSetDevice.argtypes = [ctypes.c_int]
    lib.cudaIpcGetMemHandle.restype = ctypes.c_int
    lib.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    lib.cudaMalloc.restype = ctypes.c_int
    lib.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
    lib.cudaFree.restype = ctypes.c_int
    lib.cudaFree.argtypes = [ctypes.c_void_p]
    lib.cudaDeviceSynchronize.restype = ctypes.c_int
    lib.cudaGetDevice.restype = ctypes.c_int
    lib.cudaGetDevice.argtypes = [ctypes.POINTER(ctypes.c_int)]
    _libcudart = lib


def _cuda_alloc(size):
    """Allocate GPU memory and return (dev_ptr, ipc_handle_bytes[64])."""
    _load_cudart()
    dev_ptr = ctypes.c_void_p()
    err = _libcudart.cudaMalloc(ctypes.byref(dev_ptr), size)
    if err != 0:
        raise RuntimeError(f"cudaMalloc({size}) failed: {err}")
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), dev_ptr)
    if err != 0:
        _libcudart.cudaFree(dev_ptr)
        raise RuntimeError(f"cudaIpcGetMemHandle failed: {err}")
    return dev_ptr, bytes(handle_buf)


def _cuda_free(dev_ptr):
    _load_cudart()
    _libcudart.cudaFree(dev_ptr)


def _cuda_current_device():
    _load_cudart()
    dev = ctypes.c_int()
    err = _libcudart.cudaGetDevice(ctypes.byref(dev))
    if err != 0:
        raise RuntimeError(f"cudaGetDevice failed: {err}")
    return dev.value


def calc_percentiles(data):
    """Calculate latency percentiles"""
    if not data:
        return {"avg_ms": 0, "p50_ms": 0, "p95_ms": 0, "p99_ms": 0}
    import statistics

    sorted_data = sorted(data)

    def get_percentile(p):
        if len(sorted_data) == 1:
            return sorted_data[0]
        rank = (len(sorted_data) - 1) * (p / 100)
        lower = int(rank)
        upper = min(lower + 1, len(sorted_data) - 1)
        weight = rank - lower
        return sorted_data[lower] * (1.0 - weight) + sorted_data[upper] * weight

    return {
        "avg_ms": statistics.mean(data),
        "p50_ms": get_percentile(50),
        "p95_ms": get_percentile(95),
        "p99_ms": get_percentile(99),
    }


class CertusStorage(Storage):
    """Certus storage backend via shmq shared-memory transport.

    Data plane: GPU buffers with CUDA IPC handles, DMA'd by certus-server.
    Control plane: lock-free /dev/shm mailbox (~1 µs per round-trip).

    A fixed GPU buffer pool is allocated once at init and reused across ops,
    exactly as a real vLLM inference engine reuses its KV-cache tensor. The
    per-op overhead is just the shmq control message round-trip.

    Page mapping: each page_id maps to a certus key. populate() stores data,
    lookup() loads data. check() probes existence.
    """

    def __init__(
        self,
        page_size: int,
        max_pages: int = 100000,
        shm_path: str = "/dev/shm/certus-shmq",
        gpu_device: int = 0,
        pool_buffers: int = 4,
    ):
        """Initialize Certus storage backend.

        Args:
            page_size: Size of each page in bytes.
            max_pages: Maximum number of pages (modulo mapping if trace is larger).
            shm_path: Path to the certus-server shmq mailbox.
            gpu_device: CUDA device to allocate buffers on.
            pool_buffers: Number of GPU buffers to pre-allocate (reused round-robin).
        """
        self.page_size = page_size
        self.max_pages = max_pages
        self.shm_path = shm_path
        self.gpu_device = gpu_device

        # Connect to certus-server
        print(f"  [CertusStorage] Connecting to {shm_path}")
        self.ring = connect(shm_path)
        print(
            f"  [CertusStorage] Connected: {self.ring.channel_count} channels, "
            f"cap_req={self.ring.cap_req}, cap_resp={self.ring.cap_resp}"
        )

        # Set CUDA device and allocate GPU buffer pool
        _load_cudart()
        err = _libcudart.cudaSetDevice(gpu_device)
        if err != 0:
            raise RuntimeError(f"cudaSetDevice({gpu_device}) failed: {err}")

        self._pool = []
        print(
            f"  [CertusStorage] Allocating {pool_buffers} GPU buffers "
            f"({page_size} bytes each) on device {gpu_device}"
        )
        for i in range(pool_buffers):
            dev_ptr, handle = _cuda_alloc(page_size)
            self._pool.append((dev_ptr, handle))
        self._pool_idx = 0

        # Stats (same shape as DiskHashTable)
        self.stats = {
            "read_count": 0,
            "write_count": 0,
            "read_bytes": 0,
            "write_bytes": 0,
            "read_latencies_ms": [],
            "write_latencies_ms": [],
            "read_time_s": 0.0,
            "write_time_s": 0.0,
            "sync_count": 0,
            "hit": 0,
            "miss": 0,
        }
        self._written_pages: set = set()

    def _next_buffer(self):
        """Round-robin through the pre-allocated GPU buffer pool."""
        dev_ptr, handle = self._pool[self._pool_idx]
        self._pool_idx = (self._pool_idx + 1) % len(self._pool)
        return dev_ptr, handle

    def _map_page_id(self, page_id: int) -> int:
        """Map logical page_id to physical key using modulo (same as DiskHashTable)."""
        return page_id % self.max_pages

    def read(self, page_id: int, offset_in_page: int = 0, length: int = None) -> float:
        """Read page from Certus via shmq lookup.

        Args:
            page_id: Page ID (hash_id from trace).
            offset_in_page: Offset within the page (default: 0).
            length: Number of bytes to read (default: entire page).

        Returns:
            Read latency in milliseconds.
        """
        if length is None:
            length = self.page_size - offset_in_page

        key = self._map_page_id(page_id)
        _, handle = self._next_buffer()
        region = single_region(handle, self.gpu_device, length, offset_in_page)

        start = time.perf_counter()
        try:
            self.ring.lookup([(key, [region])])
            latency = (time.perf_counter() - start) * 1000.0
            self.stats["read_count"] += 1
            self.stats["read_bytes"] += length
            self.stats["read_latencies_ms"].append(latency)
            self.stats["read_time_s"] += latency / 1000.0
            self.stats["hit"] += 1
            return latency
        except RingError as e:
            print(f"Read error (page_id={page_id}): {e}")
            return None

    def write(self, page_id: int, offset_in_page: int = 0, length: int = None) -> float:
        """Write page to Certus via shmq populate.

        Args:
            page_id: Page ID (hash_id from trace).
            offset_in_page: Offset within the page (default: 0).
            length: Number of bytes to write (default: entire page).

        Returns:
            Write latency in milliseconds.
        """
        if length is None:
            length = self.page_size - offset_in_page

        key = self._map_page_id(page_id)
        _, handle = self._next_buffer()
        region = single_region(handle, self.gpu_device, length, offset_in_page)

        start = time.perf_counter()
        try:
            self.ring.populate([(key, [region])])
            latency = (time.perf_counter() - start) * 1000.0
            self._written_pages.add(page_id)
            self.stats["write_count"] += 1
            self.stats["write_bytes"] += length
            self.stats["write_latencies_ms"].append(latency)
            self.stats["write_time_s"] += latency / 1000.0
            self.stats["miss"] += 1
            return latency
        except RingError as e:
            print(f"Write error (page_id={page_id}): {e}")
            return None

    def exists(self, page_id: int) -> bool:
        """Check if page has been written (in-memory tracking, same as DiskHashTable)."""
        return page_id in self._written_pages

    def delete(self, page_id: int) -> bool:
        """Delete page from Certus."""
        key = self._map_page_id(page_id)
        try:
            self.ring.remove([key])
            self._written_pages.discard(page_id)
            return True
        except RingError:
            return False

    def get_stats(self) -> Dict[str, Any]:
        """Get statistics (same shape as DiskHashTable for consistent reporting)."""
        import statistics as st

        def calc_stats(latencies):
            if not latencies:
                return {"avg_ms": 0, "p50_ms": 0, "p95_ms": 0, "p99_ms": 0}
            return {
                "avg_ms": st.mean(latencies),
                **calc_percentiles(latencies),
            }

        return {
            "read": {
                "count": self.stats["read_count"],
                "mb": self.stats["read_bytes"] / 1024 / 1024,
                "time_s": self.stats["read_time_s"],
                **calc_stats(self.stats["read_latencies_ms"]),
            },
            "write": {
                "count": self.stats["write_count"],
                "mb": self.stats["write_bytes"] / 1024 / 1024,
                "time_s": self.stats["write_time_s"],
                **calc_stats(self.stats["write_latencies_ms"]),
            },
            "sync_count": self.stats["sync_count"],
            "max_pages": self.max_pages,
            "written_pages": len(self._written_pages),
            "page_hits": self.stats["hit"],
            "page_misses": self.stats["miss"],
        }

    def close(self, force_sync: bool = True):
        """Release GPU buffers and close the shmq ring."""
        for dev_ptr, _ in self._pool:
            _cuda_free(dev_ptr)
        self._pool.clear()
        if self.ring is not None:
            self.ring.close()
            self.ring = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False
