#!/usr/bin/env python3
"""replay_offloading_traces.py — replay captured offloading traces.

Reads JSONL traces produced by tracing_offloading_manager.TracingOffloadingManager
(manager trace) and TracingOffloadingHandler (handler trace), and replays them
against a pluggable target.

The manager trace replay drives an OffloadingManager-shaped target. Four
built-in targets:

  simple-lru    pure-Python LRU cache, no external deps. Default.
  cpu-manager   vLLM's CPUOffloadingManager (lazy-imported).
  certus        Real Certus SPDK+NVMe engine via certus_native
                (lazy-imported; requires NVMe device + SPDK).
  fs-backend    Real llmd_fs_backend: SharedStorageOffloadingManager +
                StorageOffloadingHandlers wired to storage_offload, with
                a fabricated zero-filled GPU KV tensor. prepare_store
                triggers real disk writes; lookup checks real files.
                Requires torch+CUDA, vllm, llmd_fs_backend, storage_offload.
                Creates files under root_dir (default /tmp/kv-fs-replay);
                the storage_offload engine pads each file to its staging
                buffer, so disk usage can exceed payload bytes by a large
                factor. Clean up with `rm -rf`.

Plug in a custom target with --target 'module.path:ClassName'. The class must
expose: lookup, touch, prepare_load, complete_load, prepare_store,
complete_store. Constructor kwargs come from --target-args JSON.

The handler trace replay is simulated (per-block latency model) and does not
require vLLM.

Usage:
  # Default: pure-Python LRU, no vLLM needed
  python replay_offloading_traces.py \
      --manager-trace offloading_mgr_*.jsonl \
      --num-blocks 128

  # Against vLLM's CPUOffloadingManager
  python replay_offloading_traces.py \
      --manager-trace offloading_mgr_*.jsonl \
      --target cpu-manager --policy lru --num-blocks 128

  # Against a custom target
  python replay_offloading_traces.py \
      --manager-trace offloading_mgr_*.jsonl \
      --target mypkg.mymod:MyTarget \
      --target-args '{"capacity": 4096}'
"""

from __future__ import annotations

import argparse
import glob
import importlib
import json
import sys
from collections import Counter, OrderedDict
from dataclasses import dataclass
from pathlib import Path


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
    import inspect
    from vllm.v1.core.kv_cache_utils import BlockHash
    from vllm.v1.kv_offload.cpu.manager import CPUOffloadingManager

    ctor_kwargs = {
        "num_blocks": num_blocks,
        "cache_policy": policy,
        "enable_events": False,
    }
    sig = inspect.signature(CPUOffloadingManager.__init__)
    if "block_size" in sig.parameters:
        ctor_kwargs["block_size"] = block_size
    inner = CPUOffloadingManager(**ctor_kwargs)

    def _bh(k):
        return BlockHash(bytes.fromhex(k)) if isinstance(k, str) else k

    class _Wrapper:
        def lookup(self, keys):
            return inner.lookup([_bh(k) for k in keys]) or 0

        def touch(self, keys):
            inner.touch([_bh(k) for k in keys])

        def prepare_load(self, keys):
            inner.prepare_load([_bh(k) for k in keys])

        def complete_load(self, keys):
            inner.complete_load([_bh(k) for k in keys])

        def prepare_store(self, keys):
            out = inner.prepare_store([_bh(k) for k in keys])
            if out is None:
                return None
            return PrepareStoreOutput(
                block_hashes_to_store=list(out.block_hashes_to_store),
                block_hashes_evicted=list(out.block_hashes_evicted),
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
    try:
        from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
    except ModuleNotFoundError:
        from vllm.v1.kv_offload.base import GPULoadStoreSpec  # vLLM >= 0.20
    try:
        from vllm.v1.kv_offload.spec import (
            CanonicalKVCacheRef,
            CanonicalKVCacheTensor,
            CanonicalKVCaches,
        )
    except ModuleNotFoundError:
        from vllm.v1.kv_offload.base import (  # vLLM >= 0.20
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

    # vLLM 0.20 API detection: lookup is singular + ReqContext; OffloadKey is
    # bytes = block_hash + 4-byte group_idx. Pre-0.20 uses BlockHash + list API.
    import inspect as _inspect
    _lookup_sig = _inspect.signature(type(manager).lookup)
    _v020 = "req_context" in _lookup_sig.parameters
    if _v020:
        from vllm.v1.kv_offload.abstract import ReqContext, OffloadKey
        _req_ctx = ReqContext()
        def _bh_ok(k):  # OffloadKey = block_hash || group_idx (0, 4 bytes)
            return OffloadKey(bytes.fromhex(k) + (0).to_bytes(4, "big"))
    else:
        _bh_ok = _bh

    class _Wrapper:
        def lookup(self, keys):
            _drain_finished()
            if _v020:
                total = 0
                for k in keys:
                    if manager.lookup(_bh_ok(k), _req_ctx):
                        total += 1
                    else:
                        break  # prefix semantics: first miss ends the run
                return total
            return manager.lookup([_bh(k) for k in keys]) or 0

        def touch(self, keys):
            manager.touch([_bh_ok(k) for k in keys])

        def prepare_load(self, keys):
            ks = [_bh_ok(k) for k in keys]
            if _v020:
                manager.prepare_load(ks, _req_ctx)
            else:
                manager.prepare_load(ks)

        def complete_load(self, keys):
            manager.complete_load([_bh_ok(k) for k in keys])

        def prepare_store(self, keys):
            hashes = [_bh_ok(k) for k in keys]
            out = (manager.prepare_store(hashes, _req_ctx) if _v020
                   else manager.prepare_store(hashes))
            if out is None:
                return None
            to_store = list(getattr(out, "keys_to_store",
                                     getattr(out, "block_hashes_to_store", [])))
            evicted = list(getattr(out, "evicted_keys",
                                    getattr(out, "block_hashes_evicted", [])))
            if to_store:
                block_ids = _take_gpu_block_ids(len(to_store))
                _gpu_kwargs = {"block_ids": block_ids,
                               "group_sizes": [len(to_store)]}
                if "block_indices" in _inspect.signature(
                        GPULoadStoreSpec.__init__).parameters:
                    _gpu_kwargs["block_indices"] = [0]  # single group starts at block 0
                src = GPULoadStoreSpec(**_gpu_kwargs)
                dst = SharedStorageLoadStoreSpec(to_store)
                jid = next_job[0]
                next_job[0] += 1
                if put_handler.transfer_async(jid, (src, dst)):
                    pending.add(jid)
            return PrepareStoreOutput(
                block_hashes_to_store=[bytes(h).hex() for h in to_store],
                block_hashes_evicted=[bytes(h).hex() for h in evicted],
            )

        def complete_store(self, keys, success=True):
            _drain_until_empty()
            manager.complete_store([_bh_ok(k) for k in keys], success=success)

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
    import vllm.v1.kv_offload.base as _base

    # Shim the renamed 0.20 layout back to what certus-connector expects.
    for alias in ("vllm.v1.kv_offload.abstract",
                  "vllm.v1.kv_offload.mediums",
                  "vllm.v1.kv_offload.spec"):
        _sys.modules.setdefault(alias, _base)

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
        "io_queue_depth": 128,
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
    try:
        from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
    except ModuleNotFoundError:
        from vllm.v1.kv_offload.base import GPULoadStoreSpec  # vLLM >= 0.20
    try:
        from vllm.v1.kv_offload.spec import (
            CanonicalKVCacheRef,
            CanonicalKVCacheTensor,
            CanonicalKVCaches,
        )
    except ModuleNotFoundError:
        from vllm.v1.kv_offload.base import (  # vLLM >= 0.20
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

    import inspect as _inspect_h
    _needs_bidx = "block_indices" in _inspect_h.signature(
        GPULoadStoreSpec.__init__).parameters

    def _gpu_spec(block_ids, n_blocks):
        kw = {"block_ids": block_ids, "group_sizes": [n_blocks]}
        if _needs_bidx:
            kw["block_indices"] = [0]
        return GPULoadStoreSpec(**kw)

    class _HT:
        per_block_bytes = _pbb  # for replay-loop stats

        def transfer_async(self, job_id, n_blocks, direction):
            block_ids = _take_gpu_block_ids(n_blocks)
            if direction == "out":
                hashes = _fresh_hashes(n_blocks)
                stored_hashes.extend(hashes)
                src = _gpu_spec(block_ids, n_blocks)
                dst = SharedStorageLoadStoreSpec(hashes)
                return put_handler.transfer_async(job_id, (src, dst))
            else:
                if len(stored_hashes) < n_blocks:
                    return False
                hashes = stored_hashes[-n_blocks:]
                src = SharedStorageLoadStoreSpec(hashes)
                dst = _gpu_spec(block_ids, n_blocks)
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
    import vllm.v1.kv_offload.base as _base

    for alias in ("vllm.v1.kv_offload.abstract",
                  "vllm.v1.kv_offload.mediums",
                  "vllm.v1.kv_offload.spec"):
        _sys.modules.setdefault(alias, _base)

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
        "io_queue_depth": 128,
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

    pool = PinnedBlockPool(4096)

    # Import the mediums type we need to build CertusLoadStoreSpec. Same
    # shim + sys.path juggling as above.
    _sys.path.insert(0, _pkg)
    _saved = list(_sys.path)
    _sys.path[:] = [p for p in _sys.path if p != _shadow]
    try:
        from certus_connector.mediums import BlockLocation, CertusLoadStoreSpec  # noqa
    finally:
        _sys.path[:] = _saved

    from vllm.v1.kv_offload.base import GPULoadStoreSpec

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

def replay_manager(trace_path: Path, target, target_desc: str) -> dict:
    import time as _time

    counts: Counter = Counter()
    lookup_calls = lookup_blocks_req = lookup_blocks_hit = 0
    ps_calls = ps_rejected = 0
    ps_blocks_req = ps_blocks_admitted = ps_blocks_evicted = 0
    cs_calls = cs_blocks = 0
    unique_evicted: set = set()
    pending_store: set = set()

    # Per-method latency samples (ms).
    lat_ms: dict[str, list[float]] = {
        "lookup": [], "touch": [], "prepare_load": [], "complete_load": [],
        "prepare_store": [], "complete_store": [],
    }

    t_start = _time.perf_counter()
    for line in open(trace_path):
        if not line.strip():
            continue
        r = json.loads(line)
        m = r["method"]
        counts[m] += 1
        keys = r.get("keys", [])  # hex strings

        t0 = _time.perf_counter()

        if m == "lookup":
            lookup_calls += 1
            lookup_blocks_req += len(keys)
            hit = target.lookup(keys) or 0
            lookup_blocks_hit += hit
        elif m == "touch":
            target.touch(keys)
        elif m == "prepare_load":
            try:
                target.prepare_load(keys)
            except (AssertionError, KeyError):
                pass
        elif m == "complete_load":
            try:
                target.complete_load(keys)
            except (AssertionError, KeyError):
                pass
        elif m == "prepare_store":
            ps_calls += 1
            ps_blocks_req += len(keys)
            out = target.prepare_store(keys)
            if out is None:
                ps_rejected += 1
            else:
                ps_blocks_admitted += len(out.block_hashes_to_store)
                ps_blocks_evicted += len(out.block_hashes_evicted)
                unique_evicted.update(out.block_hashes_evicted)
                pending_store.update(out.block_hashes_to_store)
        elif m == "complete_store":
            cs_calls += 1
            cs_blocks += len(keys)
            success = bool(r.get("success", True))
            reservable = [k for k in keys if k in pending_store]
            if reservable:
                target.complete_store(reservable, success=success)
                pending_store.difference_update(reservable)

        dt_ms = (_time.perf_counter() - t0) * 1000.0
        if m in lat_ms:
            lat_ms[m].append(dt_ms)

    wall_s = _time.perf_counter() - t_start

    def pct(samples: list[float], q: float) -> float:
        if not samples:
            return 0.0
        s = sorted(samples)
        return s[min(len(s) - 1, int(len(s) * q))]

    per_method = {}
    for m, samples in lat_ms.items():
        if not samples:
            continue
        per_method[m] = {
            "count": len(samples),
            "mean_ms": sum(samples) / len(samples),
            "p50_ms": pct(samples, 0.50),
            "p95_ms": pct(samples, 0.95),
            "p99_ms": pct(samples, 0.99),
            "max_ms": max(samples),
        }

    return {
        "target": target_desc,
        "wall_s": wall_s,
        "calls": dict(counts),
        "lookup": {
            "calls": lookup_calls,
            "blocks_requested": lookup_blocks_req,
            "blocks_hit": lookup_blocks_hit,
            "hit_ratio": lookup_blocks_hit / lookup_blocks_req if lookup_blocks_req else 0.0,
        },
        "prepare_store": {
            "calls": ps_calls,
            "rejected": ps_rejected,
            "rejection_rate": ps_rejected / ps_calls if ps_calls else 0.0,
            "blocks_requested": ps_blocks_req,
            "blocks_admitted": ps_blocks_admitted,
            "blocks_evicted": ps_blocks_evicted,
            "unique_evicted": len(unique_evicted),
        },
        "complete_store": {"calls": cs_calls, "blocks": cs_blocks},
        "latency_ms": per_method,
        "throughput": {
            "ops_per_s": sum(counts.values()) / wall_s if wall_s else 0.0,
            "admitted_blocks_per_s": ps_blocks_admitted / wall_s if wall_s else 0.0,
        },
    }


def replay_handler_real(trace_path: Path, handler_target,
                         per_block_bytes: int | None = None) -> dict:
    """Drive a real worker against the captured handler trace.

    For each transfer_async event, synthesizes a destination of the same
    size as the trace record and calls handler_target.transfer_async.
    At wait/get_finished events in the trace, polls handler_target.get_finished.
    Measures real submit→done latency and throughput.
    """
    import time as _time

    if per_block_bytes is None:
        per_block_bytes = getattr(handler_target, "per_block_bytes", 256 * 1024)

    t_submit: dict[int, float] = {}
    t_done: dict[int, float] = {}
    bytes_per_job: dict[int, int] = {}
    pending: set[int] = set()
    submit_fail = 0
    counts: Counter = Counter()

    def drain(block: bool, timeout_s: float = 60.0):
        deadline = _time.perf_counter() + timeout_s
        while True:
            results = handler_target.get_finished() or []
            now = _time.perf_counter()
            for r in results:
                jid = r.job_id if hasattr(r, "job_id") else r[0]
                if jid in pending:
                    pending.discard(jid)
                    t_done[jid] = now
            if not block or not pending:
                return
            if _time.perf_counter() > deadline:
                raise TimeoutError(
                    f"handler replay: {len(pending)} jobs did not finish "
                    f"within {timeout_s}s")
            _time.sleep(0.0002)

    t0 = _time.perf_counter()
    for line in open(trace_path):
        if not line.strip():
            continue
        r = json.loads(line)
        m = r["method"]
        counts[m] += 1
        if m == "transfer_async":
            jid = r["job_id"]
            n = len(r["src"].get("block_ids", []))
            direction = "out" if r.get("transfer_type", "").startswith("GPU") else "in"
            t_submit[jid] = _time.perf_counter()
            ok = handler_target.transfer_async(jid, n, direction)
            if ok:
                pending.add(jid)
                bytes_per_job[jid] = n * per_block_bytes
            else:
                submit_fail += 1
                t_submit.pop(jid, None)
        elif m == "wait":
            if pending and hasattr(handler_target, "wait"):
                handler_target.wait(set(pending))
                # wait() is authoritative: mark every pending job done now,
                # in case the target's get_finished doesn't echo completions.
                _now = _time.perf_counter()
                for jid in list(pending):
                    t_done[jid] = _now
                pending.clear()
            drain(block=False)
        elif m == "get_finished":
            drain(block=False)

    # Drain remainder
    if pending:
        if hasattr(handler_target, "wait"):
            handler_target.wait(set(pending))
            _now = _time.perf_counter()
            for jid in list(pending):
                t_done[jid] = _now
            pending.clear()
        else:
            drain(block=True)
    wall = _time.perf_counter() - t0

    if hasattr(handler_target, "shutdown"):
        try:
            handler_target.shutdown()
        except Exception:
            pass

    latencies_ms = sorted((t_done[j] - t_submit[j]) * 1000
                          for j in t_submit if j in t_done)
    total_bytes = sum(bytes_per_job.values())
    total_blocks = sum(b // per_block_bytes for b in bytes_per_job.values())
    n = len(latencies_ms)

    def pct(q):
        return latencies_ms[min(n - 1, int(n * q))] if n else 0.0

    return {
        "config": {"per_block_bytes": per_block_bytes,
                   "backend": type(handler_target).__name__},
        "calls": dict(counts),
        "submits": len(bytes_per_job),
        "submit_failures": submit_fail,
        "real_wall_s": wall,
        "total_blocks": total_blocks,
        "total_bytes": total_bytes,
        "real_throughput_mbps": (total_bytes / (1 << 20)) / wall if wall else 0.0,
        "latency_ms": {
            "count": n,
            "mean": sum(latencies_ms) / n if n else 0.0,
            "p50": pct(0.5),
            "p95": pct(0.95),
            "p99": pct(0.99),
            "max": latencies_ms[-1] if n else 0.0,
        },
    }


def replay_handler(trace_path: Path, per_block_ms: float) -> dict:
    """Simulated handler replay: per-block copy cost, single-queue service."""
    submits: list[dict] = []
    completions: dict[int, float] = {}
    waits: list[dict] = []
    reaps: list[dict] = []

    counts: Counter = Counter()
    for line in open(trace_path):
        if not line.strip():
            continue
        r = json.loads(line)
        counts[r["method"]] += 1
        if r["method"] == "transfer_async":
            submits.append(r)
        elif r["method"] == "wait":
            waits.append(r)
        elif r["method"] == "get_finished":
            reaps.append(r)

    prev_done = 0.0
    total_bytes = 0
    total_blocks = 0
    latencies_ms: list[float] = []
    bytes_per_block = 256 * 1024

    for s in submits:
        submit_ts = s["ts"]
        n = len(s["src"].get("block_ids", []))
        service_s = per_block_ms * n / 1000.0
        start = max(prev_done, submit_ts)
        done = start + service_s
        completions[s["job_id"]] = done
        prev_done = done
        total_blocks += n
        total_bytes += n * bytes_per_block
        latencies_ms.append((done - submit_ts) * 1000.0)

    wall = prev_done - (submits[0]["ts"] if submits else 0.0)
    latencies_ms.sort()
    n = len(latencies_ms)

    def pct(q):
        return latencies_ms[min(n - 1, int(n * q))] if n else 0.0

    return {
        "config": {"per_block_ms": per_block_ms, "bytes_per_block": bytes_per_block},
        "calls": dict(counts),
        "submits": len(submits),
        "waits": len(waits),
        "get_finished_polls": len(reaps),
        "sim_wall_s": wall,
        "total_blocks": total_blocks,
        "total_bytes": total_bytes,
        "sim_throughput_mbps": (total_bytes / (1 << 20)) / wall if wall else 0.0,
        "latency_ms": {
            "count": n,
            "mean": sum(latencies_ms) / n if n else 0.0,
            "p50": pct(0.5),
            "p95": pct(0.95),
            "p99": pct(0.99),
            "max": latencies_ms[-1] if n else 0.0,
        },
    }


# ── CLI ────────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manager-trace", type=str, default=None,
                    help="path to offloading_mgr_*.jsonl (glob ok)")
    ap.add_argument("--handler-trace", type=str, default=None,
                    help="path to offloading_handler_*.jsonl (glob ok)")
    ap.add_argument("--target", default="simple-lru",
                    help="'simple-lru' (default, no vLLM), 'cpu-manager' "
                         "(vLLM's CPUOffloadingManager), or 'module:Class'")
    ap.add_argument("--target-args", type=str, default="{}",
                    help="JSON dict of kwargs merged into the target "
                         "constructor (overrides --num-blocks/--policy/--block-size)")
    ap.add_argument("--num-blocks", type=int, default=16384,
                    help="capacity (blocks) — passed as num_blocks kwarg")
    ap.add_argument("--policy", default="lru",
                    help="eviction policy — passed as policy kwarg (cpu-manager)")
    ap.add_argument("--block-size", type=int, default=16)
    ap.add_argument("--per-block-ms", type=float, default=0.05,
                    help="simulated per-block transfer cost (GPU↔CPU); "
                         "ignored when --handler-target is set")
    ap.add_argument("--handler-target", default=None,
                    help="drive a real worker for the handler trace. One of "
                         "'fs-backend', 'certus', or 'module:Class'. If unset, "
                         "the handler trace is replayed against a simulated "
                         "per-block cost model.")
    ap.add_argument("--handler-target-args", type=str, default="{}",
                    help="JSON dict of kwargs for the handler target "
                         "(e.g. root_dir, per_block_bytes, engine_config)")
    ap.add_argument("--output-json", type=Path, default=None)
    args = ap.parse_args()

    target_args = {
        "num_blocks": args.num_blocks,
        "block_size": args.block_size,
        "policy": args.policy,
    }
    target_args.update(json.loads(args.target_args))

    report: dict = {}

    if args.manager_trace:
        paths = [Path(p) for p in glob.glob(args.manager_trace)]
        assert paths, f"no manager trace found for {args.manager_trace}"
        assert len(paths) == 1, f"ambiguous manager trace: {paths}"
        print(f"[replay] manager trace: {paths[0]}", file=sys.stderr)
        print(f"[replay] target: {args.target} {target_args}", file=sys.stderr)
        target = load_target(args.target, target_args)
        report["manager"] = replay_manager(
            paths[0], target,
            target_desc=f"{args.target} {target_args}")

    if args.handler_trace:
        paths = [Path(p) for p in glob.glob(args.handler_trace)]
        assert paths, f"no handler trace found for {args.handler_trace}"
        assert len(paths) == 1, f"ambiguous handler trace: {paths}"
        print(f"[replay] handler trace: {paths[0]}", file=sys.stderr)
        if args.handler_target:
            h_args = json.loads(args.handler_target_args)
            print(f"[replay] handler target: {args.handler_target} {h_args}",
                  file=sys.stderr)
            ht = load_handler_target(args.handler_target, h_args)
            report["handler"] = replay_handler_real(paths[0], ht)
            report["handler"]["mode"] = "real"
        else:
            report["handler"] = replay_handler(paths[0], per_block_ms=args.per_block_ms)
            report["handler"]["mode"] = "simulated"

    if "manager" in report:
        r = report["manager"]
        print("\n=== manager replay ===")
        print(f"  target: {r['target']}")
        L = r["lookup"]
        print(f"  lookup:       calls={L['calls']}  req={L['blocks_requested']} blocks  "
              f"hit={L['blocks_hit']} ({L['hit_ratio']:.2%})")
        P = r["prepare_store"]
        print(f"  prepare_store: calls={P['calls']}  rejected={P['rejected']} "
              f"({P['rejection_rate']:.2%})")
        print(f"                 admitted={P['blocks_admitted']}/{P['blocks_requested']}  "
              f"evicted={P['blocks_evicted']} ({P['unique_evicted']} unique)")
        C = r["complete_store"]
        print(f"  complete_store: calls={C['calls']}  blocks={C['blocks']}")
        T = r.get("throughput", {})
        print(f"  wall: {r.get('wall_s', 0):.3f}s  "
              f"ops/s={T.get('ops_per_s', 0):.0f}  "
              f"admitted_blocks/s={T.get('admitted_blocks_per_s', 0):.0f}")
        lat = r.get("latency_ms", {})
        if lat:
            print("  latency_ms (per method):")
            print(f"    {'method':<15} {'n':>6} {'mean':>8} {'p50':>8} {'p95':>8} {'p99':>8} {'max':>8}")
            for name in ("lookup", "touch", "prepare_load", "complete_load",
                         "prepare_store", "complete_store"):
                if name not in lat:
                    continue
                x = lat[name]
                print(f"    {name:<15} {x['count']:>6} {x['mean_ms']:>8.3f} "
                      f"{x['p50_ms']:>8.3f} {x['p95_ms']:>8.3f} "
                      f"{x['p99_ms']:>8.3f} {x['max_ms']:>8.3f}")

    if "handler" in report:
        r = report["handler"]
        L = r["latency_ms"]
        if r.get("mode") == "real":
            print("\n=== handler replay (real worker) ===")
            print(f"  backend: {r['config']['backend']}  "
                  f"per_block_bytes={r['config']['per_block_bytes']}")
            print(f"  submits={r['submits']} (failures={r['submit_failures']})")
            print(f"  total_blocks={r['total_blocks']}  "
                  f"total_bytes={r['total_bytes'] / (1<<20):.1f} MiB")
            print(f"  wall={r['real_wall_s']:.3f}s  "
                  f"throughput={r['real_throughput_mbps']:.1f} MB/s")
        else:
            print("\n=== handler replay (simulated) ===")
            print(f"  config: per_block_ms={r['config']['per_block_ms']}  "
                  f"bytes_per_block={r['config']['bytes_per_block']}")
            print(f"  submits={r['submits']}  waits={r['waits']}  "
                  f"get_finished={r['get_finished_polls']}")
            print(f"  total_blocks={r['total_blocks']}  "
                  f"total_bytes={r['total_bytes'] / (1<<20):.1f} MiB")
            print(f"  sim_wall={r['sim_wall_s']:.3f}s  "
                  f"sim_throughput={r['sim_throughput_mbps']:.1f} MB/s")
        print(f"  latency_ms: p50={L['p50']:.2f}  p95={L['p95']:.2f}  "
              f"p99={L['p99']:.2f}  max={L['max']:.2f}")

    if args.output_json:
        args.output_json.write_text(json.dumps(report, indent=2))
        print(f"\n[replay] wrote {args.output_json}", file=sys.stderr)


if __name__ == "__main__":
    main()
