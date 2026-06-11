#!/usr/bin/env python3
"""replay_offloading_traces.py — replay captured offloading traces.

Reads JSONL traces produced by tracing_offloading_manager.TracingOffloadingManager
(manager trace) and TracingOffloadingHandler (handler trace), merges them by
timestamp, and replays the interleaved event stream against a storage connector.

Connectors (--connector):

  cpu      vLLM's CPUOffloadingManager — in-memory DRAM. Default.
  fs       NVMe via llmd_fs_backend (XFS mount).
  certus   CXL DRAM via certus_native (SPDK + vfio-pci).

Usage:
  python replay_offloading_traces.py \
      --trace traces/sharegpt-multiturn/500convs-64g \
      --connector cpu --num-blocks 32768

  python replay_offloading_traces.py \
      --trace traces/sharegpt-multiturn/500convs-64g \
      --connector fs --num-blocks 32768

  python replay_offloading_traces.py \
      --trace traces/sharegpt-multiturn/500convs-64g \
      --connector certus --num-blocks 32768
"""

from __future__ import annotations

import argparse
import glob
import gzip
import importlib
import json
import sys
from collections import Counter, OrderedDict
from dataclasses import dataclass
from pathlib import Path


def open_trace(path: Path):
    """Open a trace file, transparently handling .gz compression."""
    if path.suffix == ".gz":
        return gzip.open(path, "rt")
    return open(path)


# ── Target protocol + built-in implementations ─────────────────────────────

@dataclass
class PrepareStoreOutput:
    block_hashes_to_store: list
    block_hashes_evicted: list


class SimpleLRUTarget:
    """Pure-Python LRU-cache target. Keys are treated as opaque bytes/str.

    Matches the shape of vLLM's CPUOffloadingManager for the 6 methods the
    replayer calls — enough to exercise an offloading policy layer without
    pulling in vLLM at import time.
    """

    def __init__(self, num_blocks: int, block_size: int = 16, **_ignored):
        self.capacity = num_blocks
        self.block_size = block_size
        self._cache: "OrderedDict[object, None]" = OrderedDict()  # MRU at tail
        self._pending: set = set()  # prepare_store reserved, not yet complete

    def lookup(self, keys: list) -> int:
        """Count leading keys present in cache (prefix match)."""
        n = 0
        for k in keys:
            if k in self._cache:
                n += 1
            else:
                break
        return n

    def touch(self, keys: list) -> None:
        for k in keys:
            if k in self._cache:
                self._cache.move_to_end(k)

    def prepare_load(self, keys: list) -> None:
        for k in keys:
            if k not in self._cache:
                raise KeyError(f"prepare_load miss: {k!r}")
            self._cache.move_to_end(k)

    def complete_load(self, keys: list) -> None:
        return

    def prepare_store(self, keys: list) -> PrepareStoreOutput | None:
        to_store = [k for k in keys if k not in self._cache and k not in self._pending]
        if len(to_store) > self.capacity:
            return None  # request bigger than cache
        space = self.capacity - len(self._cache) - len(self._pending)
        evicted: list = []
        need = len(to_store) - space
        if need > 0:
            for k in list(self._cache.keys()):
                if need == 0:
                    break
                evicted.append(k)
                del self._cache[k]
                need -= 1
        self._pending.update(to_store)
        return PrepareStoreOutput(block_hashes_to_store=to_store,
                                  block_hashes_evicted=evicted)

    def complete_store(self, keys: list, success: bool = True) -> None:
        for k in keys:
            self._pending.discard(k)
            if success:
                self._cache[k] = None
                self._cache.move_to_end(k)


def _make_cpu_manager_target(num_blocks: int, block_size: int = 16,
                             policy: str = "lru", **_ignored):
    """vLLM-backed target. Imports vLLM lazily; wraps key conversion."""
    from vllm.v1.core.kv_cache_utils import BlockHash
    from vllm.v1.kv_offload.abstract import ReqContext
    from vllm.v1.kv_offload.cpu.manager import CPUOffloadingManager

    inner = CPUOffloadingManager(
        num_blocks=num_blocks,
        cache_policy=policy,
        enable_events=False,
    )
    _rc = ReqContext()

    def _bh(k):
        return BlockHash(bytes.fromhex(k)) if isinstance(k, str) else k

    class _Wrapper:
        def lookup(self, keys):
            count = 0
            for k in keys:
                hit = inner.lookup(_bh(k), _rc)
                if hit:
                    count += 1
                else:
                    break
            return count

        def touch(self, keys):
            inner.touch([_bh(k) for k in keys])

        def prepare_load(self, keys):
            inner.prepare_load([_bh(k) for k in keys], _rc)

        def complete_load(self, keys):
            inner.complete_load([_bh(k) for k in keys])

        def prepare_store(self, keys):
            out = inner.prepare_store([_bh(k) for k in keys], _rc)
            if out is None:
                return None
            return PrepareStoreOutput(
                block_hashes_to_store=list(out.keys_to_store),
                block_hashes_evicted=list(out.evicted_keys),
            )

        def complete_store(self, keys, success=True):
            inner.complete_store([_bh(k) for k in keys], success=success)

    return _Wrapper()


def _make_fs_backend_target(
    root_dir: str = "/tmp/kv-fs-replay",
    model_name: str = "replay",
    num_gpu_blocks: int = 4096,
    per_block_bytes: int = 16 * 1024,
    gpu_block_size: int = 16,
    gpu_blocks_per_file: int = 1,
    threads_per_gpu: int = 8,
    dtype_name: str = "int8",
    extra_config: dict | None = None,
    **_ignored,
):
    """Real llmd_fs_backend target.

    Moves actual (zero-filled) bytes from a fabricated GPU KV tensor to
    disk via storage_offload. lookup goes through the real manager, which
    checks file existence — so hits materialize only after
    complete_store() has drained the corresponding transfer_async.

    Requirements: vllm, torch with CUDA, llmd_fs_backend, storage_offload.
    Side effect: real files under root_dir; clean up with `rm -rf`.
    """
    import os
    import time
    import torch  # noqa: F401  — lazy
    import storage_offload  # noqa: F401

    from vllm.v1.core.kv_cache_utils import BlockHash
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
    from vllm.v1.kv_offload.spec import (
        CanonicalKVCacheRef,
        CanonicalKVCacheTensor,
        CanonicalKVCaches,
    )
    from llmd_fs_backend.file_mapper import FileMapper
    from llmd_fs_backend.manager import SharedStorageOffloadingManager
    from llmd_fs_backend.mediums import SharedStorageLoadStoreSpec
    from llmd_fs_backend.worker import StorageOffloadingHandlers

    if not torch.cuda.is_available():
        raise RuntimeError("fs-backend target requires a CUDA device")

    os.makedirs(root_dir, exist_ok=True)

    kv = torch.zeros(num_gpu_blocks, per_block_bytes,
                     dtype=torch.int8, device="cuda")
    kv_caches = CanonicalKVCaches(
        tensors=[CanonicalKVCacheTensor(tensor=kv,
                                        page_size_bytes=per_block_bytes)],
        group_data_refs=[[CanonicalKVCacheRef(tensor_idx=0,
                                              page_size_bytes=per_block_bytes)]],
    )

    file_mapper = FileMapper(
        root_dir=root_dir, model_name=model_name,
        gpu_block_size=gpu_block_size,
        gpu_blocks_per_file=gpu_blocks_per_file,
        tp_size=1, pp_size=1, pcp_size=1, rank=0, dtype=dtype_name,
    )
    manager = SharedStorageOffloadingManager(file_mapper)
    handlers = StorageOffloadingHandlers(
        kv_caches=kv_caches, file_mapper=file_mapper,
        gpu_block_size=gpu_block_size,
        gpu_blocks_per_file=gpu_blocks_per_file,
        threads_per_gpu=threads_per_gpu,
        extra_config=extra_config or {},
    )
    put_handler = handlers.gpu_to_storage_handler

    block_cursor = [0]
    next_job = [1]
    pending: set[int] = set()

    def _bh(k):
        return BlockHash(bytes.fromhex(k)) if isinstance(k, str) else k

    def _take_gpu_block_ids(n):
        ids = [(block_cursor[0] + i) % num_gpu_blocks for i in range(n)]
        block_cursor[0] = (block_cursor[0] + n) % num_gpu_blocks
        return ids

    def _drain_finished():
        for r in put_handler.get_finished():
            pending.discard(r.job_id)

    def _drain_until_empty(timeout_s: float = 30.0):
        deadline = time.perf_counter() + timeout_s
        while pending and time.perf_counter() < deadline:
            _drain_finished()
            if pending:
                time.sleep(0.001)
        if pending:
            raise TimeoutError(f"fs-backend: {len(pending)} transfers did not "
                               f"complete within {timeout_s}s")

    class _Wrapper:
        def lookup(self, keys):
            # Drain completions so recently-finished stores show up as hits.
            _drain_finished()
            return manager.lookup([_bh(k) for k in keys]) or 0

        def touch(self, keys):
            manager.touch([_bh(k) for k in keys])

        def prepare_load(self, keys):
            manager.prepare_load([_bh(k) for k in keys])

        def complete_load(self, keys):
            manager.complete_load([_bh(k) for k in keys])

        def prepare_store(self, keys):
            hashes = [_bh(k) for k in keys]
            out = manager.prepare_store(hashes)
            if out is None:
                return None
            to_store = list(out.block_hashes_to_store)
            if to_store:
                block_ids = _take_gpu_block_ids(len(to_store))
                src = GPULoadStoreSpec(block_ids=block_ids,
                                       group_sizes=[len(to_store)],
                                       block_indices=[0])
                dst = SharedStorageLoadStoreSpec(to_store)
                jid = next_job[0]
                next_job[0] += 1
                if put_handler.transfer_async(jid, (src, dst)):
                    pending.add(jid)
            return PrepareStoreOutput(
                block_hashes_to_store=[bytes(h).hex() for h in to_store],
                block_hashes_evicted=[bytes(h).hex()
                                       for h in out.block_hashes_evicted],
            )

        def complete_store(self, keys, success=True):
            # Block until all in-flight writes have finished so the files
            # are on disk before subsequent lookups run.
            _drain_until_empty()
            manager.complete_store([_bh(k) for k in keys], success=success)

    return _Wrapper()


def _make_certus_connector_target(extra_config: dict | None = None,
                                   gpu_block_size: int = 16,
                                   **_ignored):
    """Real Certus connector package — via the production CertusOffloadingSpec.

    Builds minimal VllmConfig / KVCacheConfig stand-ins, instantiates
    CertusOffloadingSpec, and calls spec.get_manager() to get the manager
    that vLLM's OffloadingConnector would pick up when wired with
    kv_connector_extra_config.spec_name="CertusOffloadingSpec". This
    exercises the spec's extra_config plumbing (slab_size_bytes,
    dram_cache_bytes, use_native, tiering config) — not just the bare
    NativeCertusOffloadingManager.

    extra_config kwargs are merged into the spec's extra_config dict and
    shape how the spec builds the engine / manager.
    """
    import sys as _sys
    from types import SimpleNamespace
    import certus_native  # noqa: F401
    import vllm.v1.kv_offload.abstract as _base

    # Shim: certus-connector may import the old name "base" (renamed to "abstract" in 0.20).
    _sys.modules.setdefault("vllm.v1.kv_offload.base", _base)

    _pkg = "/home/bdh/kvconn-trace/ai-native-storage-certus/certus-connector"
    _shadow = "/home/bdh/kvconn-trace"
    if _pkg not in _sys.path:
        _sys.path.insert(0, _pkg)
    _saved_path = list(_sys.path)
    _sys.path[:] = [p for p in _sys.path if p != _shadow]
    _sys.modules.pop("certus_connector", None)
    try:
        from certus_connector.spec import CertusOffloadingSpec  # noqa
    finally:
        _sys.path[:] = _saved_path

    cfg = {
        "data_pci_addrs": ["0000:61:00.0"],
        "metadata_pci_addr": "0000:62:00.0",
        "slab_size_bytes": 131072,
        "dram_cache_bytes": 1 << 30,
        "io_queue_depth": 1024,
        "use_native": True,
    }
    if extra_config:
        cfg.update(extra_config)

    # Minimal vLLM-config stand-ins — just the attributes the spec reads.
    vllm_config = SimpleNamespace(
        kv_transfer_config=SimpleNamespace(kv_connector_extra_config=cfg),
        parallel_config=SimpleNamespace(
            decode_context_parallel_size=1,
            prefill_context_parallel_size=1,
            tensor_parallel_size=1,
            pipeline_parallel_size=1,
            rank=0,
            world_size=1,
        ),
        cache_config=SimpleNamespace(
            block_size=gpu_block_size,
            cache_dtype="float16",
        ),
        model_config=SimpleNamespace(model="replay"),
        kv_events_config=SimpleNamespace(enable_kv_cache_events=False),
    )
    kv_cache_config = SimpleNamespace(
        kv_cache_groups=[SimpleNamespace(
            kv_cache_spec=SimpleNamespace(block_size=gpu_block_size),
        )],
    )

    spec_obj = CertusOffloadingSpec(vllm_config, kv_cache_config)
    mgr = spec_obj.get_manager()

    def _k(hex_str):
        """Hex trace key → OffloadKey (bytes). certus-connector uses the
        first 8 bytes as a u64 CacheKey."""
        return bytes.fromhex(hex_str)

    hex_by_bytes: dict[bytes, str] = {}

    class _W:
        def lookup(self, keys):
            bs = [_k(k) for k in keys]
            for k, b in zip(keys, bs):
                hex_by_bytes[b] = k
            return mgr.lookup(bs) or 0

        def touch(self, keys):
            bs = [_k(k) for k in keys]
            for k, b in zip(keys, bs):
                hex_by_bytes[b] = k
            mgr.touch(bs)

        def prepare_load(self, keys):
            try:
                mgr.prepare_load([_k(k) for k in keys])
            except Exception:
                pass

        def complete_load(self, keys):
            try:
                mgr.complete_load([_k(k) for k in keys])
            except Exception:
                pass

        def prepare_store(self, keys):
            bs = [_k(k) for k in keys]
            for k, b in zip(keys, bs):
                hex_by_bytes[b] = k
            out = mgr.prepare_store(bs)
            if out is None:
                return None
            to_store = list(out.keys_to_store)
            evicted = list(out.evicted_keys)
            return PrepareStoreOutput(
                block_hashes_to_store=[hex_by_bytes[b] for b in to_store],
                block_hashes_evicted=[hex_by_bytes.get(b, b.hex())
                                       for b in evicted],
            )

        def complete_store(self, keys, success=True):
            bs = [_k(k) for k in keys]
            try:
                mgr.complete_store(bs, success)
            except TypeError:
                mgr.complete_store(bs, success=success)

        def shutdown(self):
            try:
                mgr.shutdown()
            except Exception:
                pass

    return _W()


# ── Handler-side targets (real workers) ────────────────────────────────────
#
# A handler target exposes:
#   transfer_async(job_id: int, n_blocks: int, direction: str) -> bool
#     direction is 'out' (GPU → backend) or 'in' (backend → GPU)
#   wait(job_ids: set[int]) -> None
#   get_finished() -> list — each item has .job_id (or is a (jid, success) tuple)
#   shutdown() -> None   (optional; called at end of replay)
#
# The handler trace records transfers by size (block count) and direction but
# not by content — the replay driver can therefore synthesize destination
# identifiers (block hashes for FS, u64 keys for Certus) per request and
# remember them for any subsequent 'in' direction transfers.

def _make_fs_handler_target(
    root_dir: str = "/tmp/kv-fs-handler-replay",
    model_name: str = "replay",
    num_gpu_blocks: int = 4096,
    per_block_bytes: int = 16 * 1024,
    gpu_block_size: int = 16,
    gpu_blocks_per_file: int = 1,
    threads_per_gpu: int = 8,
    dtype_name: str = "int8",
    extra_config: dict | None = None,
    **_ignored,
):
    """Real llmd_fs_backend worker (GPUToStorage + StorageToGPU handlers)."""
    import os
    import torch  # noqa: F401
    import storage_offload  # noqa: F401

    from vllm.v1.core.kv_cache_utils import BlockHash
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
    from vllm.v1.kv_offload.spec import (
        CanonicalKVCacheRef,
        CanonicalKVCacheTensor,
        CanonicalKVCaches,
    )
    from llmd_fs_backend.file_mapper import FileMapper
    from llmd_fs_backend.mediums import SharedStorageLoadStoreSpec
    from llmd_fs_backend.worker import StorageOffloadingHandlers

    if not torch.cuda.is_available():
        raise RuntimeError("fs-backend handler target requires a CUDA device")

    os.makedirs(root_dir, exist_ok=True)
    kv = torch.zeros(num_gpu_blocks, per_block_bytes,
                     dtype=torch.int8, device="cuda")
    kv_caches = CanonicalKVCaches(
        tensors=[CanonicalKVCacheTensor(tensor=kv, page_size_bytes=per_block_bytes)],
        group_data_refs=[[CanonicalKVCacheRef(tensor_idx=0,
                                              page_size_bytes=per_block_bytes)]],
    )
    file_mapper = FileMapper(
        root_dir=root_dir, model_name=model_name,
        gpu_block_size=gpu_block_size,
        gpu_blocks_per_file=gpu_blocks_per_file,
        tp_size=1, pp_size=1, pcp_size=1, rank=0, dtype=dtype_name,
    )
    handlers = StorageOffloadingHandlers(
        kv_caches=kv_caches, file_mapper=file_mapper,
        gpu_block_size=gpu_block_size,
        gpu_blocks_per_file=gpu_blocks_per_file,
        threads_per_gpu=threads_per_gpu,
        extra_config=extra_config or {},
    )
    put_handler = handlers.gpu_to_storage_handler
    get_handler = handlers.storage_to_gpu_handler

    block_cursor = [0]
    next_hash = [0]
    stored_hashes: list = []  # ring of recently-written hashes for 'in' direction

    def _take_gpu_block_ids(n):
        ids = [(block_cursor[0] + i) % num_gpu_blocks for i in range(n)]
        block_cursor[0] = (block_cursor[0] + n) % num_gpu_blocks
        return ids

    def _fresh_hashes(n):
        out = []
        for _ in range(n):
            next_hash[0] += 1
            out.append(BlockHash(next_hash[0].to_bytes(32, "big")))
        return out

    _pbb = per_block_bytes

    class _HT:
        per_block_bytes = _pbb  # for replay-loop stats

        def transfer_async(self, job_id, n_blocks, direction):
            block_ids = _take_gpu_block_ids(n_blocks)
            if direction == "out":
                hashes = _fresh_hashes(n_blocks)
                stored_hashes.extend(hashes)
                src = GPULoadStoreSpec(block_ids=block_ids,
                                        group_sizes=[n_blocks],
                                        block_indices=[0])
                dst = SharedStorageLoadStoreSpec(hashes)
                return put_handler.transfer_async(job_id, (src, dst))
            else:
                if len(stored_hashes) < n_blocks:
                    return False
                hashes = stored_hashes[-n_blocks:]
                src = SharedStorageLoadStoreSpec(hashes)
                dst = GPULoadStoreSpec(block_ids=block_ids,
                                        group_sizes=[n_blocks],
                                        block_indices=[0])
                return get_handler.transfer_async(job_id, (src, dst))

        def wait(self, job_ids):
            put_handler.wait(set(job_ids))

        def get_finished(self):
            # Both handlers share _pending_jobs and the engine's completion queue.
            # Polling either drains everything.
            return put_handler.get_finished() + get_handler.get_finished()

        def shutdown(self):
            pass

    return _HT()


def _make_certus_connector_handler_target(extra_config: dict | None = None,
                                          gpu_block_size: int = 16,
                                          **_ignored):
    """Real certus-connector handler path: drives NativeCertusOffloadingManager's
    underlying certus_native.CertusEngine for actual GPU→NVMe transfers.

    Uses CertusOffloadingSpec.get_manager() to obtain the manager, then
    extracts its `_engine` for store_async / load_async / wait_job calls.
    This exercises real SPDK IO without needing the separate (not-built)
    CertusTransferEngine class.
    """
    import sys as _sys
    from types import SimpleNamespace
    import certus_native  # noqa: F401
    import vllm.v1.kv_offload.abstract as _base

    _sys.modules.setdefault("vllm.v1.kv_offload.base", _base)

    _pkg = "/home/bdh/kvconn-trace/ai-native-storage-certus/certus-connector"
    _shadow = "/home/bdh/kvconn-trace"
    if _pkg not in _sys.path:
        _sys.path.insert(0, _pkg)
    _saved_path = list(_sys.path)
    _sys.path[:] = [p for p in _sys.path if p != _shadow]
    _sys.modules.pop("certus_connector", None)
    try:
        from certus_connector.spec import CertusOffloadingSpec  # noqa
    finally:
        _sys.path[:] = _saved_path

    from certus_offload_manager import PinnedBlockPool, NATIVE_BLOCK_BYTES

    cfg = {
        "data_pci_addrs": ["0000:61:00.0"],
        "metadata_pci_addr": "0000:62:00.0",
        "slab_size_bytes": NATIVE_BLOCK_BYTES,
        "dram_cache_bytes": 1 << 30,
        "io_queue_depth": 1024,
        "use_native": True,
    }
    if extra_config:
        cfg.update(extra_config)

    vllm_config = SimpleNamespace(
        kv_transfer_config=SimpleNamespace(kv_connector_extra_config=cfg),
        parallel_config=SimpleNamespace(
            decode_context_parallel_size=1,
            prefill_context_parallel_size=1,
            tensor_parallel_size=1,
            pipeline_parallel_size=1,
            rank=0, world_size=1,
        ),
        cache_config=SimpleNamespace(
            block_size=gpu_block_size, cache_dtype="float16",
        ),
        model_config=SimpleNamespace(model="replay"),
        kv_events_config=SimpleNamespace(enable_kv_cache_events=False),
    )
    kv_cache_config = SimpleNamespace(
        kv_cache_groups=[SimpleNamespace(
            kv_cache_spec=SimpleNamespace(block_size=gpu_block_size),
        )],
    )

    spec_obj = CertusOffloadingSpec(vllm_config, kv_cache_config)
    mgr = spec_obj.get_manager()   # NativeCertusOffloadingManager
    # Post-b32ec5f the spec routes handlers through the same CertusEngine the
    # manager uses, so get_handlers() yields real workers (not a mock).
    handlers_iter = list(spec_obj.get_handlers(kv_caches=None))
    gpu_to_certus = None
    certus_to_gpu = None
    for src_t, dst_t, handler in handlers_iter:
        if src_t.__name__ == "GPULoadStoreSpec":
            gpu_to_certus = handler
        else:
            certus_to_gpu = handler
    if gpu_to_certus is None or certus_to_gpu is None:
        raise RuntimeError(
            f"spec.get_handlers returned unexpected shape: {handlers_iter!r}")

    pool = PinnedBlockPool(512)

    # Import the mediums type we need to build CertusLoadStoreSpec. Same
    # shim + sys.path juggling as above.
    _sys.path.insert(0, _pkg)
    _saved = list(_sys.path)
    _sys.path[:] = [p for p in _sys.path if p != _shadow]
    try:
        from certus_connector.mediums import BlockLocation, CertusLoadStoreSpec  # noqa
    finally:
        _sys.path[:] = _saved

    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    next_key = [1]
    stored_keys: list[int] = []

    class _HT:
        per_block_bytes = NATIVE_BLOCK_BYTES

        def transfer_async(self, job_id, n_blocks, direction):
            gpu_ids = pool.take(n_blocks)
            if direction == "out":
                keys = list(range(next_key[0], next_key[0] + n_blocks))
                next_key[0] += n_blocks
                stored_keys.extend(keys)
                src = GPULoadStoreSpec(block_ids=gpu_ids,
                                        group_sizes=[n_blocks],
                                        block_indices=[0])
                dst = CertusLoadStoreSpec(
                    [BlockLocation(nvme_slab=k, dram_slot=None) for k in keys])
                try:
                    return bool(gpu_to_certus.transfer_async(job_id, (src, dst)))
                except Exception:
                    return False
            else:
                if len(stored_keys) < n_blocks:
                    return False
                keys = stored_keys[-n_blocks:]
                src = CertusLoadStoreSpec(
                    [BlockLocation(nvme_slab=k, dram_slot=None) for k in keys])
                dst = GPULoadStoreSpec(block_ids=gpu_ids,
                                        group_sizes=[n_blocks],
                                        block_indices=[0])
                try:
                    return bool(certus_to_gpu.transfer_async(job_id, (src, dst)))
                except Exception:
                    return False

        def wait(self, job_ids):
            gpu_to_certus.wait(set(job_ids))

        def get_finished(self):
            return (gpu_to_certus.get_finished()
                    + certus_to_gpu.get_finished())

        def shutdown(self):
            try:
                mgr.shutdown()
            except Exception:
                pass

    return _HT()


def load_handler_target(spec: str, target_args: dict):
    """Build a handler-side target: 'fs-backend', 'certus-connector',
    or 'module:Class'."""
    if spec == "fs-backend":
        return _make_fs_handler_target(**target_args)
    if spec == "certus-connector":
        return _make_certus_connector_handler_target(**target_args)
    if ":" not in spec:
        raise ValueError(
            f"--handler-target {spec!r} must be 'fs-backend', "
            f"'certus-connector', or 'module.path:ClassName'"
        )
    mod_path, cls_name = spec.split(":", 1)
    mod = importlib.import_module(mod_path)
    cls = getattr(mod, cls_name)
    return cls(**target_args)


def load_target(spec: str, target_args: dict):
    """Build a replay target. `spec` is one of the built-in names or
    'module.path:ClassName'."""
    if spec == "simple-lru":
        return SimpleLRUTarget(**target_args)
    if spec == "cpu-manager":
        return _make_cpu_manager_target(**target_args)
    if spec == "certus-connector":
        return _make_certus_connector_target(**target_args)
    if spec == "fs-backend":
        return _make_fs_backend_target(**target_args)
    if ":" not in spec:
        raise ValueError(
            f"--target {spec!r} must be one of 'simple-lru', 'cpu-manager', "
            f"'certus-connector', 'fs-backend', or 'module.path:ClassName'"
        )
    mod_path, cls_name = spec.split(":", 1)
    mod = importlib.import_module(mod_path)
    cls = getattr(mod, cls_name)
    return cls(**target_args)


# ── Replay loops ───────────────────────────────────────────────────────────





def _make_cpu_shared_targets(cpu_bytes: int = 64 * (1 << 30),
                             gpu_block_size: int = 16,
                             num_gpu_blocks: int = 16384,
                             **_ignored):
    """Build a CPU offloading manager + handler sharing one CPUOffloadingSpec."""
    import torch
    from types import SimpleNamespace
    from vllm.v1.kv_cache_interface import (
        KVCacheConfig, KVCacheTensor, KVCacheGroupSpec, FullAttentionSpec,
    )
    from vllm.v1.kv_offload.abstract import ReqContext
    from vllm.v1.kv_offload.cpu.spec import CPUOffloadingSpec
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
    from vllm.v1.kv_offload.spec import (
        CanonicalKVCacheRef, CanonicalKVCacheTensor, CanonicalKVCaches,
    )

    num_kv_heads = 8
    head_dim = 128
    num_layers = 32
    per_block_bytes = gpu_block_size * num_kv_heads * head_dim * 2 * 2
    total_kv_bytes = per_block_bytes * num_layers * num_gpu_blocks

    layer_names = [f"layer{i}" for i in range(num_layers)]
    spec = FullAttentionSpec(block_size=gpu_block_size,
                            num_kv_heads=num_kv_heads, head_size=head_dim,
                            dtype=torch.float16)
    group = KVCacheGroupSpec(layer_names=layer_names, kv_cache_spec=spec)
    tensor = KVCacheTensor(size=total_kv_bytes, shared_by=layer_names)
    kv_cache_config = KVCacheConfig(num_blocks=num_gpu_blocks,
                                    kv_cache_tensors=[tensor],
                                    kv_cache_groups=[group])

    vllm_config = SimpleNamespace(
        kv_transfer_config=SimpleNamespace(
            kv_connector_extra_config={
                "cpu_bytes_to_use": cpu_bytes,
                "eviction_policy": "lru",
            }
        ),
        cache_config=SimpleNamespace(block_size=gpu_block_size),
        parallel_config=SimpleNamespace(world_size=1),
        model_config=SimpleNamespace(model="replay"),
        kv_events_config=SimpleNamespace(enable_kv_cache_events=False),
    )

    cpu_spec = CPUOffloadingSpec(vllm_config, kv_cache_config)
    mgr = cpu_spec.get_manager()

    # Build kv_caches for get_handlers — needs CUDA tensor.
    # Use a small GPU block count to avoid exhausting GPU memory.
    replay_gpu_blocks = min(num_gpu_blocks, 256)
    page_bytes = per_block_bytes * num_layers
    kv = torch.zeros(replay_gpu_blocks * page_bytes, dtype=torch.int8, device="cuda")
    kv_caches = CanonicalKVCaches(
        tensors=[CanonicalKVCacheTensor(tensor=kv, page_size_bytes=page_bytes)],
        group_data_refs=[[CanonicalKVCacheRef(tensor_idx=0,
                                              page_size_bytes=page_bytes)]],
    )
    handlers_iter = list(cpu_spec.get_handlers(kv_caches))
    gpu_to_cpu = None
    cpu_to_gpu = None
    for src_t, dst_t, handler in handlers_iter:
        if src_t.__name__ == "GPULoadStoreSpec":
            gpu_to_cpu = handler
        else:
            cpu_to_gpu = handler

    _rc = ReqContext()
    from vllm.v1.core.kv_cache_utils import BlockHash

    def _bh(k):
        return BlockHash(bytes.fromhex(k)) if isinstance(k, str) else k

    store_key_queue: list[list] = []
    stored_keys: list = []

    class _MgrW:
        def lookup(self, keys):
            count = 0
            for k in keys:
                hit = mgr.lookup(_bh(k), _rc)
                if hit:
                    count += 1
                else:
                    break
            return count

        def touch(self, keys):
            mgr.touch([_bh(k) for k in keys])

        def prepare_load(self, keys):
            try:
                mgr.prepare_load([_bh(k) for k in keys], _rc)
            except Exception:
                pass

        def complete_load(self, keys):
            try:
                mgr.complete_load([_bh(k) for k in keys])
            except Exception:
                pass

        def prepare_store(self, keys):
            bkeys = [_bh(k) for k in keys]
            out = mgr.prepare_store(bkeys, _rc)
            if out is None:
                return None
            to_store = list(out.keys_to_store)
            evicted = list(out.evicted_keys)
            if to_store:
                store_key_queue.append(to_store)
            return PrepareStoreOutput(
                block_hashes_to_store=[k.hex() if isinstance(k, bytes) else str(k) for k in to_store],
                block_hashes_evicted=[k.hex() if isinstance(k, bytes) else str(k) for k in evicted],
            )

        def complete_store(self, keys, success=True):
            mgr.complete_store([_bh(k) for k in keys], success=success)

        def shutdown(self):
            pass

    block_cursor = [0]

    class _HandlerW:
        per_block_bytes = page_bytes

        def transfer_async(self, job_id, n_blocks, direction):
            if gpu_to_cpu is None:
                return True
            block_ids = [(block_cursor[0] + i) % replay_gpu_blocks for i in range(n_blocks)]
            block_cursor[0] = (block_cursor[0] + n_blocks) % replay_gpu_blocks
            if direction == "out":
                if store_key_queue:
                    bkeys = store_key_queue.pop(0)
                    if len(bkeys) != n_blocks:
                        bkeys = bkeys[:n_blocks] if len(bkeys) > n_blocks else bkeys + [bkeys[-1]] * (n_blocks - len(bkeys))
                else:
                    bkeys = [BlockHash(i.to_bytes(32, "big")) for i in range(n_blocks)]
                stored_keys.extend(bkeys)
                from vllm.v1.kv_offload.mediums import CPULoadStoreSpec
                src = GPULoadStoreSpec(block_ids=block_ids,
                                        group_sizes=[n_blocks],
                                        block_indices=[0])
                dst = CPULoadStoreSpec(block_ids=list(range(n_blocks)))
                try:
                    return bool(gpu_to_cpu.transfer_async(job_id, (src, dst)))
                except Exception:
                    return False
            else:
                from vllm.v1.kv_offload.mediums import CPULoadStoreSpec
                src = CPULoadStoreSpec(block_ids=list(range(n_blocks)))
                dst = GPULoadStoreSpec(block_ids=block_ids,
                                        group_sizes=[n_blocks],
                                        block_indices=[0])
                try:
                    return bool(cpu_to_gpu.transfer_async(job_id, (src, dst)))
                except Exception:
                    return False

        def wait(self, job_ids):
            if gpu_to_cpu:
                gpu_to_cpu.wait(set(job_ids))

        def get_finished(self):
            results = []
            if gpu_to_cpu:
                results.extend(gpu_to_cpu.get_finished())
            if cpu_to_gpu:
                results.extend(cpu_to_gpu.get_finished())
            return results

        def shutdown(self):
            pass

    return _MgrW(), _HandlerW()


def _make_certus_shared_targets(extra_config: dict | None = None,
                                gpu_block_size: int = 16,
                                **_ignored):
    """Build a certus manager + handler target that share one engine.

    Returns (mgr_target, handler_target). Both use the same
    CertusOffloadingSpec so dispatcher.populate() from the handler makes
    keys visible to dispatcher.check() from the manager.
    """
    import sys as _sys
    from types import SimpleNamespace
    import certus_native  # noqa: F401
    import vllm.v1.kv_offload.abstract as _base

    _sys.modules.setdefault("vllm.v1.kv_offload.base", _base)

    _pkg = "/home/bdh/kvconn-trace/ai-native-storage-certus/certus-connector"
    _shadow = "/home/bdh/kvconn-trace"
    if _pkg not in _sys.path:
        _sys.path.insert(0, _pkg)
    _saved_path = list(_sys.path)
    _sys.path[:] = [p for p in _sys.path if p != _shadow]
    _sys.modules.pop("certus_connector", None)
    try:
        from certus_connector.spec import CertusOffloadingSpec
        from certus_connector.mediums import BlockLocation, CertusLoadStoreSpec
    finally:
        _sys.path[:] = _saved_path

    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    cfg = {
        "data_pci_addrs": ["0000:61:00.0"],
        "metadata_pci_addr": "0000:62:00.0",
        "slab_size_bytes": 131072,
        "dram_cache_bytes": 1 << 30,
        "io_queue_depth": 1024,
        "use_native": True,
    }
    if extra_config:
        cfg.update(extra_config)

    vllm_config = SimpleNamespace(
        kv_transfer_config=SimpleNamespace(kv_connector_extra_config=cfg),
        parallel_config=SimpleNamespace(
            decode_context_parallel_size=1, prefill_context_parallel_size=1,
            tensor_parallel_size=1, pipeline_parallel_size=1,
            rank=0, world_size=1,
        ),
        cache_config=SimpleNamespace(block_size=gpu_block_size, cache_dtype="float16"),
        model_config=SimpleNamespace(model="replay"),
        kv_events_config=SimpleNamespace(enable_kv_cache_events=False),
    )
    kv_cache_config = SimpleNamespace(
        kv_cache_groups=[SimpleNamespace(
            kv_cache_spec=SimpleNamespace(block_size=gpu_block_size),
        )],
    )

    spec_obj = CertusOffloadingSpec(vllm_config, kv_cache_config)
    mgr = spec_obj.get_manager()

    # Get handlers from the same spec
    handlers_iter = list(spec_obj.get_handlers(kv_caches=None))
    gpu_to_certus = None
    certus_to_gpu = None
    for src_t, dst_t, handler in handlers_iter:
        if src_t.__name__ == "GPULoadStoreSpec":
            gpu_to_certus = handler
        else:
            certus_to_gpu = handler
    if gpu_to_certus is None or certus_to_gpu is None:
        raise RuntimeError(
            f"spec.get_handlers returned unexpected shape: {handlers_iter!r}")

    hex_by_bytes: dict[bytes, str] = {}
    # Queue of u64 keys from prepare_store for the handler to use in store_async.
    # Each entry is a list of u64s corresponding to one prepare_store batch.
    store_key_queue: list[list[int]] = []
    # Keys that have been stored (for load direction)
    stored_u64_keys: list[int] = []

    def _k(hex_str):
        return bytes.fromhex(hex_str)

    def _key_to_u64(key_bytes):
        return int.from_bytes(key_bytes[:8], "big")

    # Manager target wrapper
    class _MgrW:
        def lookup(self, keys):
            bs = [_k(k) for k in keys]
            for k, b in zip(keys, bs):
                hex_by_bytes[b] = k
            return mgr.lookup(bs) or 0

        def touch(self, keys):
            bs = [_k(k) for k in keys]
            for k, b in zip(keys, bs):
                hex_by_bytes[b] = k
            mgr.touch(bs)

        def prepare_load(self, keys):
            try:
                mgr.prepare_load([_k(k) for k in keys])
            except Exception:
                pass

        def complete_load(self, keys):
            try:
                mgr.complete_load([_k(k) for k in keys])
            except Exception:
                pass

        def prepare_store(self, keys):
            bs = [_k(k) for k in keys]
            for k, b in zip(keys, bs):
                hex_by_bytes[b] = k
            out = mgr.prepare_store(bs)
            if out is None:
                return None
            to_store = list(out.keys_to_store)
            evicted = list(out.evicted_keys)
            # Enqueue the u64 keys for the handler to use in store_async
            u64_keys = [_key_to_u64(k) for k in to_store]
            if u64_keys:
                store_key_queue.append(u64_keys)
            return PrepareStoreOutput(
                block_hashes_to_store=[hex_by_bytes[b] for b in to_store],
                block_hashes_evicted=[hex_by_bytes.get(b, b.hex())
                                       for b in evicted],
            )

        def complete_store(self, keys, success=True):
            bs = [_k(k) for k in keys]
            try:
                mgr.complete_store(bs, success)
            except TypeError:
                mgr.complete_store(bs, success=success)

        def shutdown(self):
            try:
                mgr.shutdown()
            except Exception:
                pass

    # Handler target wrapper
    from certus_offload_manager import PinnedBlockPool, NATIVE_BLOCK_BYTES

    pool = PinnedBlockPool(512)

    class _HandlerW:
        per_block_bytes = NATIVE_BLOCK_BYTES

        def transfer_async(self, job_id, n_blocks, direction):
            gpu_ids = pool.take(n_blocks)
            if direction == "out":
                # Pop keys from the queue that prepare_store enqueued
                if store_key_queue:
                    u64_keys = store_key_queue.pop(0)
                    # The queue entry should match n_blocks
                    if len(u64_keys) != n_blocks:
                        # Size mismatch — use what we have
                        u64_keys = u64_keys[:n_blocks] if len(u64_keys) > n_blocks else u64_keys + [u64_keys[-1]] * (n_blocks - len(u64_keys))
                else:
                    # Fallback: shouldn't happen in well-formed traces
                    u64_keys = list(range(n_blocks))
                stored_u64_keys.extend(u64_keys)
                src = GPULoadStoreSpec(block_ids=gpu_ids,
                                        group_sizes=[n_blocks],
                                        block_indices=[0])
                dst = CertusLoadStoreSpec(
                    [BlockLocation(nvme_slab=k, dram_slot=None) for k in u64_keys])
                try:
                    return bool(gpu_to_certus.transfer_async(job_id, (src, dst)))
                except Exception:
                    return False
            else:
                if len(stored_u64_keys) < n_blocks:
                    return False
                u64_keys = stored_u64_keys[-n_blocks:]
                src = CertusLoadStoreSpec(
                    [BlockLocation(nvme_slab=k, dram_slot=None) for k in u64_keys])
                dst = GPULoadStoreSpec(block_ids=gpu_ids,
                                        group_sizes=[n_blocks],
                                        block_indices=[0])
                try:
                    return bool(certus_to_gpu.transfer_async(job_id, (src, dst)))
                except Exception:
                    return False

        def wait(self, job_ids):
            gpu_to_certus.wait(set(job_ids))

        def get_finished(self):
            return (gpu_to_certus.get_finished()
                    + certus_to_gpu.get_finished())

        def shutdown(self):
            pass

    return _MgrW(), _HandlerW()


# ── Interleaved replay ────────────────────────────────────────────────────

def replay_interleaved(mgr_path: Path, handler_path: Path,
                       mgr_target, handler_target,
                       per_block_bytes: int | None = None) -> dict:
    """Merge mgr + handler traces on timestamp and replay in order.

    This ensures that handler transfer_async calls (which call
    dispatcher.populate and make keys visible) execute before subsequent
    manager lookups that depend on them.
    """
    import heapq
    import time as _time

    if per_block_bytes is None:
        per_block_bytes = getattr(handler_target, "per_block_bytes", 131072)

    # Load and tag events
    events = []
    for line in open_trace(mgr_path):
        if not line.strip():
            continue
        r = json.loads(line)
        r["_source"] = "mgr"
        events.append((r["ts"], 0, r))
    for line in open_trace(handler_path):
        if not line.strip():
            continue
        r = json.loads(line)
        r["_source"] = "handler"
        events.append((r["ts"], 1, r))

    events.sort(key=lambda x: (x[0], x[1]))

    # Manager state
    mgr_counts: Counter = Counter()
    lookup_calls = lookup_blocks_req = lookup_blocks_hit = 0
    ps_calls = ps_rejected = 0
    ps_blocks_req = ps_blocks_admitted = ps_blocks_evicted = 0
    pending_store: set = set()

    # Handler state
    handler_counts: Counter = Counter()
    h_pending: set = set()
    h_submit_times: dict[int, float] = {}
    h_done_times: dict[int, float] = {}
    h_bytes_per_job: dict[int, int] = {}
    h_submit_fail = 0

    # Per-method latency
    lat_ms: dict[str, list[float]] = {
        "lookup": [], "touch": [], "prepare_load": [], "complete_load": [],
        "prepare_store": [], "complete_store": [],
        "transfer_async": [], "get_finished": [], "wait": [],
    }

    def drain_handler(block: bool = False, timeout_s: float = 60.0):
        deadline = _time.perf_counter() + timeout_s
        while True:
            results = handler_target.get_finished() or []
            now = _time.perf_counter()
            for r in results:
                jid = r.job_id if hasattr(r, "job_id") else r[0]
                if jid in h_pending:
                    h_pending.discard(jid)
                    h_done_times[jid] = now
            if not block or not h_pending:
                return
            if _time.perf_counter() > deadline:
                return
            _time.sleep(0.0002)

    t_wall_start = _time.perf_counter()

    for _, _, r in events:
        source = r["_source"]
        m = r["method"]
        t0 = _time.perf_counter()

        if source == "mgr":
            mgr_counts[m] += 1
            keys = r.get("keys", [])

            if m == "lookup":
                lookup_calls += 1
                lookup_blocks_req += len(keys)
                hit = mgr_target.lookup(keys) or 0
                lookup_blocks_hit += hit
            elif m == "touch":
                mgr_target.touch(keys)
            elif m == "prepare_load":
                try:
                    mgr_target.prepare_load(keys)
                except (AssertionError, KeyError):
                    pass
            elif m == "complete_load":
                try:
                    mgr_target.complete_load(keys)
                except (AssertionError, KeyError):
                    pass
            elif m == "prepare_store":
                ps_calls += 1
                ps_blocks_req += len(keys)
                out = mgr_target.prepare_store(keys)
                if out is None:
                    ps_rejected += 1
                else:
                    ps_blocks_admitted += len(out.block_hashes_to_store)
                    ps_blocks_evicted += len(out.block_hashes_evicted)
                    pending_store.update(out.block_hashes_to_store)
            elif m == "complete_store":
                success = bool(r.get("success", True))
                reservable = [k for k in keys if k in pending_store]
                if reservable:
                    mgr_target.complete_store(reservable, success=success)
                    pending_store.difference_update(reservable)

            dt_ms = (_time.perf_counter() - t0) * 1000.0
            if m in lat_ms:
                lat_ms[m].append(dt_ms)

        else:  # handler
            handler_counts[m] += 1

            if m == "transfer_async":
                jid = r["job_id"]
                n = len(r["src"].get("block_ids", []))
                direction = "out" if r.get("transfer_type", "").startswith("GPU") else "in"
                h_submit_times[jid] = _time.perf_counter()
                ok = handler_target.transfer_async(jid, n, direction)
                if ok:
                    h_pending.add(jid)
                    h_bytes_per_job[jid] = n * per_block_bytes
                else:
                    h_submit_fail += 1
                    h_submit_times.pop(jid, None)
            elif m == "wait":
                if h_pending and hasattr(handler_target, "wait"):
                    handler_target.wait(set(h_pending))
                    _now = _time.perf_counter()
                    for jid in list(h_pending):
                        h_done_times[jid] = _now
                    h_pending.clear()
                drain_handler(block=False)
            elif m == "get_finished":
                drain_handler(block=False)

            dt_ms = (_time.perf_counter() - t0) * 1000.0
            if m in lat_ms:
                lat_ms[m].append(dt_ms)

    # Drain remaining handler jobs
    if h_pending:
        if hasattr(handler_target, "wait"):
            handler_target.wait(set(h_pending))
            _now = _time.perf_counter()
            for jid in list(h_pending):
                h_done_times[jid] = _now
            h_pending.clear()
        else:
            drain_handler(block=True)

    wall_s = _time.perf_counter() - t_wall_start

    if hasattr(handler_target, "shutdown"):
        try:
            handler_target.shutdown()
        except Exception:
            pass
    if hasattr(mgr_target, "shutdown"):
        try:
            mgr_target.shutdown()
        except Exception:
            pass

    # Compute handler latencies
    h_latencies_ms = sorted((h_done_times[j] - h_submit_times[j]) * 1000
                            for j in h_submit_times if j in h_done_times)
    total_bytes = sum(h_bytes_per_job.values())
    n_h = len(h_latencies_ms)

    def pct(samples, q):
        if not samples:
            return 0.0
        s = sorted(samples) if samples is not h_latencies_ms else samples
        return s[min(len(s) - 1, int(len(s) * q))]

    def method_stats(name):
        samples = lat_ms.get(name, [])
        if not samples:
            return None
        return {
            "count": len(samples),
            "mean_ms": sum(samples) / len(samples),
            "p50_ms": pct(samples, 0.50),
            "p95_ms": pct(samples, 0.95),
            "p99_ms": pct(samples, 0.99),
            "max_ms": max(samples),
        }

    return {
        "mode": "interleaved",
        "wall_s": wall_s,
        "manager": {
            "calls": dict(mgr_counts),
            "lookup": {
                "calls": lookup_calls,
                "blocks_requested": lookup_blocks_req,
                "blocks_hit": lookup_blocks_hit,
                "hit_ratio": lookup_blocks_hit / lookup_blocks_req if lookup_blocks_req else 0.0,
            },
            "prepare_store": {
                "calls": ps_calls,
                "rejected": ps_rejected,
                "blocks_requested": ps_blocks_req,
                "blocks_admitted": ps_blocks_admitted,
                "blocks_evicted": ps_blocks_evicted,
            },
        },
        "handler": {
            "calls": dict(handler_counts),
            "submits": len(h_bytes_per_job),
            "submit_failures": h_submit_fail,
            "total_bytes": total_bytes,
            "throughput_mbps": (total_bytes / (1 << 20)) / wall_s if wall_s else 0.0,
            "latency_ms": {
                "count": n_h,
                "mean": sum(h_latencies_ms) / n_h if n_h else 0.0,
                "p50": pct(h_latencies_ms, 0.5),
                "p95": pct(h_latencies_ms, 0.95),
                "p99": pct(h_latencies_ms, 0.99),
                "max": h_latencies_ms[-1] if n_h else 0.0,
            },
        },
        "latency_ms": {name: method_stats(name) for name in lat_ms if method_stats(name)},
    }


# ── CLI ────────────────────────────────────────────────────────────────────

def _resolve_trace(prefix: str) -> tuple[Path, Path]:
    """Resolve a trace prefix to (mgr_path, handler_path).

    Accepts: a prefix like 'traces/sharegpt-multiturn/500convs-64g'
    and finds <prefix>.mgr.jsonl[.gz] and <prefix>.handler.jsonl[.gz].
    """
    base = Path(prefix)
    for ext in (".mgr.jsonl.gz", ".mgr.jsonl"):
        candidate = Path(str(base) + ext)
        if candidate.exists():
            mgr = candidate
            break
    else:
        raise FileNotFoundError(f"no manager trace found for prefix {prefix}")
    for ext in (".handler.jsonl.gz", ".handler.jsonl"):
        candidate = Path(str(base) + ext)
        if candidate.exists():
            handler = candidate
            break
    else:
        raise FileNotFoundError(f"no handler trace found for prefix {prefix}")
    return mgr, handler


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--trace", type=str, required=True,
                    help="trace prefix (e.g. traces/sharegpt-multiturn/500convs-64g)")
    ap.add_argument("--connector", default="cpu",
                    choices=["cpu", "fs", "certus"],
                    help="storage backend: cpu (DRAM), fs (NVMe), certus (CXL)")
    ap.add_argument("--connector-args", type=str, default="{}",
                    help="JSON dict of extra kwargs for the connector "
                         "(e.g. root_dir, per_block_bytes, dram_cache_bytes)")
    ap.add_argument("--num-blocks", type=int, default=16384,
                    help="capacity (blocks) for the offload tier")
    ap.add_argument("--policy", default="lru",
                    help="eviction policy (cpu/fs connectors)")
    ap.add_argument("--block-size", type=int, default=16)
    ap.add_argument("--output-json", type=Path, default=None)
    args = ap.parse_args()

    mgr_path, h_path = _resolve_trace(args.trace)
    print(f"[replay] manager trace: {mgr_path}", file=sys.stderr)
    print(f"[replay] handler trace: {h_path}", file=sys.stderr)
    print(f"[replay] connector: {args.connector}", file=sys.stderr)

    extra = json.loads(args.connector_args)

    if args.connector == "cpu":
        mgr_target, handler_target = _make_cpu_shared_targets(
            gpu_block_size=args.block_size,
            num_gpu_blocks=args.num_blocks)
    elif args.connector == "certus":
        if not extra.get("dram_cache_bytes"):
            extra["dram_cache_bytes"] = 4 * (1 << 30)
        mgr_target, handler_target = _make_certus_shared_targets(
            extra_config=extra or None,
            gpu_block_size=args.block_size)
    elif args.connector == "fs":
        target_args = {
            "num_blocks": args.num_blocks,
            "block_size": args.block_size,
            "policy": args.policy,
        }
        mgr_target = load_target("cpu-manager", target_args)
        h_args = {"root_dir": "/mnt/fs-backend-bench", "per_block_bytes": 2097152}
        h_args.update(extra)
        handler_target = load_handler_target("fs-backend", h_args)
    report = replay_interleaved(
        mgr_path, h_path, mgr_target, handler_target)

    # Print summary
    M = report["manager"]
    H = report["handler"]
    L = M["lookup"]
    print(f"\n=== replay ===")
    print(f"  wall: {report['wall_s']:.3f}s")
    print(f"  lookup: calls={L['calls']}  hit={L['blocks_hit']}/{L['blocks_requested']} "
          f"({L['hit_ratio']:.2%})")
    P = M["prepare_store"]
    print(f"  prepare_store: calls={P['calls']}  admitted={P['blocks_admitted']}")
    print(f"  handler: submits={H['submits']} failures={H['submit_failures']}  "
          f"throughput={H['throughput_mbps']:.1f} MB/s")
    HL = H["latency_ms"]
    print(f"  handler latency: p50={HL['p50']:.2f}ms  p95={HL['p95']:.2f}ms  "
          f"p99={HL['p99']:.2f}ms  max={HL['max']:.2f}ms")
    lat = report.get("latency_ms", {})
    if lat:
        print("  mgr latency_ms (per method):")
        print(f"    {'method':<15} {'n':>6} {'mean':>8} {'p50':>8} {'p95':>8} {'p99':>8} {'max':>8}")
        for name in ("lookup", "touch", "prepare_load", "complete_load",
                     "prepare_store", "complete_store"):
            if name not in lat:
                continue
            x = lat[name]
            print(f"    {name:<15} {x['count']:>6} {x['mean_ms']:>8.3f} "
                  f"{x['p50_ms']:>8.3f} {x['p95_ms']:>8.3f} "
                  f"{x['p99_ms']:>8.3f} {x['max_ms']:>8.3f}")

    if args.output_json:
        args.output_json.write_text(json.dumps(report, indent=2))
        print(f"\n[replay] wrote {args.output_json}", file=sys.stderr)


if __name__ == "__main__":
    main()
