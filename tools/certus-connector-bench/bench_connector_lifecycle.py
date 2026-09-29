#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""End-to-end connector-path benchmark: full vLLM offloading lifecycle.

Unlike certus_fio and bench_connector_path (which call Ring.lookup() directly),
this benchmark exercises the actual connector classes that vLLM uses:

  - ShmqCertusOffloadingManager (manager.py): touch, lookup, prepare_store,
    complete_store, prepare_load, complete_load
  - CertusShmqWorker (handler.py): submit_store, submit_load, get_finished

The measured path includes every layer the production connector adds on top of
the raw ring transport: key namespacing (ns_key per TP rank), per-block region
construction (_regions_for_block), ThreadPoolExecutor dispatch, BLAKE2b content
hashing, and the lookup cache.

Usage (certus-server must be running):
    python bench_connector_lifecycle.py --shm-path /dev/shm/certus-shmq

    # Larger batches, longer run
    python bench_connector_lifecycle.py --bs 64 --num-blocks 256 --min-duration 10

    # Run the SAME pattern as certus_fio but through the connector
    python bench_connector_lifecycle.py --pattern cold_prefill_store
    python bench_connector_lifecycle.py --pattern warm_prefill_load_and_suffix_store

    # Simulate TP=2 (namespaced keys, 2× server entries per logical block)
    python bench_connector_lifecycle.py --tp 2

    # CSV output for automated collection
    python bench_connector_lifecycle.py --csv results.csv
"""

from __future__ import annotations

import argparse
import collections
import csv
import ctypes
import hashlib
import os
import random
import statistics
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field

_here = os.path.dirname(os.path.abspath(__file__))
_repo = os.path.join(_here, "..", "..")
sys.path.insert(0, os.path.join(_repo, "certus-shmq-connector"))
sys.path.insert(0, os.path.join(_repo, "apps", "python"))

import torch

assert torch.cuda.is_available(), "CUDA GPU required"


# ── vLLM compatibility shim ─────────────────────────────────────────────────
#
# The connector's handler.py imports vLLM symbols (GPULoadStoreSpec,
# TransferResult, worker base class) at module level via compat's __getattr__.
# When vLLM is not installed we must seed compat's symbol cache and stub its
# adapter functions BEFORE importing handler/manager, so the lazy resolution
# never fires.

def _has_vllm() -> bool:
    try:
        from importlib.metadata import version as _dv
        _dv("vllm")
        return True
    except Exception:
        return False


def _patch_compat_for_bench():
    """Seed compat with lightweight stubs so the connector imports without vLLM."""
    if "CERTUS_VLLM_VERSION" not in os.environ:
        os.environ["CERTUS_VLLM_VERSION"] = "0.24"

    from certus_shmq_connector import compat as _c
    from dataclasses import dataclass as _dc

    @_dc
    class _TransferResult:
        job_id: int
        success: bool
        transfer_size: int
        transfer_time: float
        transfer_type: object = None

    @_dc
    class _PrepareStoreOutput:
        keys_to_store: list
        store_spec: object
        evicted_keys: list

    class _FakeLoadStoreSpec:
        pass

    class _FakeGPULoadStoreSpec(_FakeLoadStoreSpec):
        pass

    class _FakeOffloadingManager:
        pass

    class _FakeOffloadingEvent:
        def __init__(self, **kw):
            self.__dict__.update(kw)

    _c._VLLM_SYMBOLS.update({
        "TransferResult": _TransferResult,
        "GPULoadStoreSpec": _FakeGPULoadStoreSpec,
        "LoadStoreSpec": _FakeLoadStoreSpec,
        "OffloadingManager": _FakeOffloadingManager,
        "OffloadingEvent": _FakeOffloadingEvent,
        "OffloadKey": object,
        "PrepareStoreOutput": _PrepareStoreOutput,
        "OffloadingSpec": object,
        "VllmConfig": object,
        "KVCacheConfig": object,
    })

    _orig_wbc = _c.worker_base_class
    def _stub_wbc():
        try:
            return _orig_wbc()
        except (ImportError, TypeError):
            return object
    _c.worker_base_class = _stub_wbc

    def _stub_mtr(job_id, success, transfer_size, transfer_time, transfer_type):
        return _TransferResult(
            job_id=job_id, success=success,
            transfer_size=transfer_size, transfer_time=transfer_time,
            transfer_type=transfer_type,
        )
    _c.make_transfer_result = _stub_mtr
    _c.lookup_result = lambda exists: bool(exists)


if not _has_vllm():
    _patch_compat_for_bench()


from certus_shmq_connector.ring import Ring  # noqa: E402
from certus_shmq_connector.manager import ShmqCertusOffloadingManager  # noqa: E402
from certus_shmq_connector.handler import _regions_for_block, worker_class  # noqa: E402
from certus_shmq_connector.gpu import KvCacheIpc  # noqa: E402
from certus_shmq_connector.mediums import CertusLoadStoreSpec  # noqa: E402

# handler.py imports make_transfer_result from compat at module level; if we
# patched compat, propagate the stub into the already-imported handler module.
if not _has_vllm():
    import certus_shmq_connector.handler as _h
    from certus_shmq_connector.compat import make_transfer_result as _mtr
    _h.make_transfer_result = _mtr

# Default block size: 2 MiB (Llama-3-8B: 16 tokens × 32 layers × 4096 bytes).
DEFAULT_BLOCK_BYTES = 2 * 1024 * 1024


# ── content-hashed keys (realistic vLLM pattern) ───────────────────────────

def make_content_keys(n: int, seed: int = 42) -> list[bytes]:
    """Generate n distinct 36-byte keys mimicking vLLM's content-hashed blocks.

    vLLM OffloadKeys are 36 bytes: a 32-byte block hash + 4-byte KV-cache-group
    index. We simulate this with BLAKE2b(seed || index) + a group suffix, giving
    realistic key distribution through the BLAKE2b fold in manager._key_to_u64.
    """
    keys = []
    for i in range(n):
        h = hashlib.blake2b(f"{seed}:{i}".encode(), digest_size=32).digest()
        group_idx = (i % 4).to_bytes(4, "big")
        keys.append(h + group_idx)
    return keys


# ── fake GPU region (CUDA IPC handle from a real allocation) ────────────────

def _get_cuda_ipc_handle(ptr: int) -> bytes:
    handle = (ctypes.c_byte * 64)()
    err = ctypes.CDLL("libcudart.so").cudaIpcGetMemHandle(
        ctypes.byref(handle), ctypes.c_void_p(ptr)
    )
    assert err == 0, f"cudaIpcGetMemHandle failed: {err}"
    return bytes(handle)


def _alloc_base(data_ptr: int) -> int:
    base = ctypes.c_ulonglong(0)
    size = ctypes.c_size_t(0)
    err = ctypes.CDLL("libcuda.so").cuMemGetAddressRange_v2(
        ctypes.byref(base), ctypes.byref(size), ctypes.c_ulonglong(data_ptr)
    )
    assert err == 0, f"cuMemGetAddressRange failed: {err}"
    return base.value


def make_kv_region(gpu_id: int, block_bytes: int, num_blocks: int = 128) -> tuple[KvCacheIpc, torch.Tensor]:
    """Allocate a GPU tensor and build the KvCacheIpc the worker uses.

    The tensor is sized to hold ``num_blocks`` blocks. The worker's
    ``_regions_for_block(regions, block_id)`` computes
    ``offset = base_delta + block_id * stride_bytes``, so block_ids up to
    ``num_blocks - 1`` must land within the allocation.
    """
    total_bytes = block_bytes * num_blocks
    tensor = torch.randint(
        0, 256, (total_bytes,), dtype=torch.uint8, device=f"cuda:{gpu_id}"
    )
    data_ptr = tensor.data_ptr()
    alloc = _alloc_base(data_ptr)
    handle = _get_cuda_ipc_handle(data_ptr)
    region = KvCacheIpc(
        handle_bytes=handle,
        gpu_device_id=gpu_id,
        stride_bytes=block_bytes,
        base_delta=data_ptr - alloc,
    )
    return region, tensor


# ── phase timing ────────────────────────────────────────────────────────────

@dataclass
class PhaseTiming:
    name: str
    samples: list[float] = field(default_factory=list)

    def record(self, seconds: float):
        self.samples.append(seconds)

    @property
    def p50_us(self) -> float:
        return statistics.median(self.samples) * 1e6 if self.samples else 0.0

    @property
    def p99_us(self) -> float:
        if not self.samples:
            return 0.0
        s = sorted(self.samples)
        return s[min(int(len(s) * 0.99), len(s) - 1)] * 1e6

    @property
    def mean_us(self) -> float:
        return statistics.mean(self.samples) * 1e6 if self.samples else 0.0


@dataclass
class BenchResult:
    label: str
    total_blocks: int
    total_bytes: int
    wall_seconds: float
    phases: dict[str, PhaseTiming]

    @property
    def throughput_gbs(self) -> float:
        return self.total_bytes / self.wall_seconds / 1e9 if self.wall_seconds > 0 else 0.0

    @property
    def iops(self) -> float:
        return self.total_blocks / self.wall_seconds if self.wall_seconds > 0 else 0.0

    def print_report(self):
        print(f"\n{'='*70}")
        print(f"  {self.label}")
        print(f"{'='*70}")
        print(f"  blocks={self.total_blocks}  "
              f"throughput={self.throughput_gbs:.2f} GB/s  "
              f"iops={self.iops:.0f} blk/s  "
              f"wall={self.wall_seconds:.3f}s")
        print(f"  Phase breakdown:")
        for name, timing in self.phases.items():
            if timing.samples:
                print(f"    {name:20s}  p50={timing.p50_us:8.1f}us  "
                      f"p99={timing.p99_us:8.1f}us  "
                      f"mean={timing.mean_us:8.1f}us  "
                      f"n={len(timing.samples)}")

    def csv_row(self, extra: dict | None = None) -> dict:
        row = {
            "label": self.label,
            "total_blocks": self.total_blocks,
            "total_bytes": self.total_bytes,
            "wall_seconds": f"{self.wall_seconds:.6f}",
            "throughput_gbs": f"{self.throughput_gbs:.4f}",
            "iops": f"{self.iops:.1f}",
        }
        for name, timing in self.phases.items():
            row[f"{name}_p50_us"] = f"{timing.p50_us:.1f}"
            row[f"{name}_p99_us"] = f"{timing.p99_us:.1f}"
            row[f"{name}_mean_us"] = f"{timing.mean_us:.1f}"
        if extra:
            row.update(extra)
        return row


# ── store lifecycle ─────────────────────────────────────────────────────────

def bench_store_lifecycle(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    keys: list[bytes],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
) -> BenchResult:
    """Full store lifecycle: touch → prepare_store → submit_store → get_finished → complete_store."""
    phases = {
        "touch": PhaseTiming("touch"),
        "prepare_store": PhaseTiming("prepare_store"),
        "submit_store": PhaseTiming("submit_store"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_store": PhaseTiming("complete_store"),
    }

    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 0

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for i in range(0, len(keys), batch_size):
            batch_keys = keys[i : i + batch_size]

            # 1. touch (updates LRU, populates lookup cache)
            t0 = time.perf_counter()
            manager.touch(batch_keys)
            phases["touch"].record(time.perf_counter() - t0)

            # 2. prepare_store (check + reserve)
            t0 = time.perf_counter()
            result = manager.prepare_store(batch_keys)
            phases["prepare_store"].record(time.perf_counter() - t0)

            if result is None or not result.keys_to_store:
                continue

            stored_keys = result.keys_to_store

            # Build GPU block ids (sequential; vLLM zips these with keys)
            gpu_block_ids = list(range(len(stored_keys)))

            # 3. submit_store (async GPU→Certus via ThreadPoolExecutor)
            t0 = time.perf_counter()
            job_id += 1
            worker.submit_store(
                job_id,
                _FakeGPUSpec(gpu_block_ids),
                result.store_spec,
            )
            phases["submit_store"].record(time.perf_counter() - t0)

            # 4. get_finished (reap completed futures)
            t0 = time.perf_counter()
            while not worker.get_finished():
                time.sleep(0.0001)
            phases["get_finished"].record(time.perf_counter() - t0)

            # 5. complete_store (commit)
            t0 = time.perf_counter()
            manager.complete_store(stored_keys, success=True)
            phases["complete_store"].record(time.perf_counter() - t0)

            total_blocks += len(stored_keys)

        iteration += 1
        # After iteration 0, the keys exist in the server. Remove them so the
        # next iteration's prepare_store actually reserves again.
        if (time.perf_counter() - wall_start) < min_duration:
            _remove_keys(manager._ring, keys, manager._world_size)

    wall = time.perf_counter() - wall_start
    return BenchResult(
        label="Store Lifecycle (touch → prepare_store → submit → finish → complete)",
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )


# ── load lifecycle ──────────────────────────────────────────────────────────

def bench_load_lifecycle(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    keys: list[bytes],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    cold: bool = False,
) -> BenchResult:
    """Full load lifecycle: touch → lookup → prepare_load → submit_load → get_finished → complete_load."""
    label_temp = "Cold" if cold else "Warm"
    phases = {
        "touch": PhaseTiming("touch"),
        "lookup": PhaseTiming("lookup"),
        "prepare_load": PhaseTiming("prepare_load"),
        "submit_load": PhaseTiming("submit_load"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_load": PhaseTiming("complete_load"),
    }

    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 1000

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        if cold and iteration > 0:
            try:
                manager._ring.flush_to_ssd()
                manager._ring.clear_memory_tier()
            except Exception:
                pass

        for i in range(0, len(keys), batch_size):
            batch_keys = keys[i : i + batch_size]

            # 1. touch (populates the lookup cache bitmap)
            t0 = time.perf_counter()
            manager.touch(batch_keys)
            phases["touch"].record(time.perf_counter() - t0)

            # 2. lookup (per-key, from cache or fallback Check)
            t0 = time.perf_counter()
            hits = []
            for k in batch_keys:
                if manager.lookup(k):
                    hits.append(k)
            phases["lookup"].record(time.perf_counter() - t0)

            if not hits:
                continue

            # 3. prepare_load (pin)
            t0 = time.perf_counter()
            load_spec = manager.prepare_load(hits)
            phases["prepare_load"].record(time.perf_counter() - t0)

            gpu_block_ids = list(range(len(hits)))

            # 4. submit_load (async Certus→GPU via ThreadPoolExecutor)
            t0 = time.perf_counter()
            job_id += 1
            worker.submit_load(
                job_id,
                load_spec,
                _FakeGPUSpec(gpu_block_ids),
            )
            phases["submit_load"].record(time.perf_counter() - t0)

            # 5. get_finished
            t0 = time.perf_counter()
            while not worker.get_finished():
                time.sleep(0.0001)
            phases["get_finished"].record(time.perf_counter() - t0)

            # 6. complete_load (unpin)
            t0 = time.perf_counter()
            manager.complete_load(hits)
            phases["complete_load"].record(time.perf_counter() - t0)

            total_blocks += len(hits)

        iteration += 1

    wall = time.perf_counter() - wall_start
    return BenchResult(
        label=f"{label_temp} Load Lifecycle (touch → lookup → prepare → submit → finish → complete)",
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )


# ── mixed store+load under eviction pressure ───────────────────────────────

def bench_mixed_eviction(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    working_set: int = 512,
    load_fraction: float = 0.5,
) -> BenchResult:
    """Interleaved store + load under memory-tier pressure.

    Simulates the production pattern: new blocks are continuously stored
    (pushing the LRU evictor), while earlier blocks are loaded back.  The
    working set is larger than what the tier can hold without eviction, so
    the server's evictor runs throughout.  ``take_events()`` is drained each
    iteration to surface eviction-event overhead.

    ``load_fraction`` controls what share of each iteration is loads vs stores
    (0.5 = half-and-half). Keys rotate across generations so every store is a
    new key (not a dedup hit).
    """
    phases = {
        "store_touch": PhaseTiming("store_touch"),
        "prepare_store": PhaseTiming("prepare_store"),
        "submit_store": PhaseTiming("submit_store"),
        "store_finish": PhaseTiming("store_finish"),
        "complete_store": PhaseTiming("complete_store"),
        "load_touch": PhaseTiming("load_touch"),
        "lookup": PhaseTiming("lookup"),
        "prepare_load": PhaseTiming("prepare_load"),
        "submit_load": PhaseTiming("submit_load"),
        "load_finish": PhaseTiming("load_finish"),
        "complete_load": PhaseTiming("complete_load"),
        "take_events": PhaseTiming("take_events"),
    }

    total_blocks = 0
    total_evictions = 0
    store_drops = 0
    load_misses = 0
    wall_start = time.perf_counter()
    generation = 0
    job_id = 100_000

    # Split the working set: the first load_fraction are "old" (will be loaded),
    # the rest are "new" (will be stored, pushing evictions).
    load_count = max(1, int(working_set * load_fraction))
    store_count = working_set - load_count

    print(f"\n  Eviction mode: working_set={working_set} "
          f"(store={store_count}/gen, load={load_count}/gen, "
          f"block_bytes={block_bytes})")
    print(f"  The tier fills after ~{(4 * 1024**3) // block_bytes // store_count} "
          f"generations (assuming 4G tier), then eviction kicks in.")

    # Start clean: clear DRAM and use a random base seed so keys from previous
    # runs (still on SSD in the dispatch-map) don't collide and get deduped.
    manager._ring.clear_memory_tier()
    io_before = manager._ring.get_io_stats()
    base_seed = random.randint(10_000_000, 99_000_000)

    # Seed the tier with an initial generation of loadable keys.
    seed_keys = make_content_keys(load_count, seed=base_seed)
    _populate_keys(manager, worker, seed_keys, batch_size)
    loadable_keys = list(seed_keys)

    while (time.perf_counter() - wall_start) < min_duration or generation < 3:
        generation += 1

        # ── store phase: push new keys, forcing eviction ──
        new_keys = make_content_keys(store_count, seed=base_seed + 1_000_000 * generation)
        for i in range(0, len(new_keys), batch_size):
            batch = new_keys[i : i + batch_size]

            t0 = time.perf_counter()
            manager.touch(batch)
            phases["store_touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            result = manager.prepare_store(batch)
            phases["prepare_store"].record(time.perf_counter() - t0)

            if result is None or not result.keys_to_store:
                store_drops += len(batch)
                continue

            stored = result.keys_to_store
            if len(stored) < len(batch):
                store_drops += len(batch) - len(stored)

            gpu_ids = list(range(len(stored)))
            job_id += 1

            t0 = time.perf_counter()
            worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
            phases["submit_store"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker.get_finished():
                time.sleep(0.0001)
            phases["store_finish"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager.complete_store(stored, success=True)
            phases["complete_store"].record(time.perf_counter() - t0)

            total_blocks += len(stored)

        # ── drain eviction events (the real scheduler does this) ──
        t0 = time.perf_counter()
        for ev in manager.take_events():
            total_evictions += len(ev.keys) if hasattr(ev, "keys") else 0
        phases["take_events"].record(time.perf_counter() - t0)

        # ── load phase: load from the previous generation's keys ──
        for i in range(0, len(loadable_keys), batch_size):
            batch = loadable_keys[i : i + batch_size]

            t0 = time.perf_counter()
            manager.touch(batch)
            phases["load_touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            hits = [k for k in batch if manager.lookup(k)]
            phases["lookup"].record(time.perf_counter() - t0)

            if not hits:
                load_misses += len(batch)
                continue
            load_misses += len(batch) - len(hits)

            t0 = time.perf_counter()
            load_spec = manager.prepare_load(hits)
            phases["prepare_load"].record(time.perf_counter() - t0)

            gpu_ids = list(range(len(hits)))
            job_id += 1

            t0 = time.perf_counter()
            worker.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
            phases["submit_load"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker.get_finished():
                time.sleep(0.0001)
            phases["load_finish"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager.complete_load(hits)
            phases["complete_load"].record(time.perf_counter() - t0)

            total_blocks += len(hits)

        # Rotate: this generation's stored keys become next generation's load targets.
        loadable_keys = list(new_keys)

        # Give the background writer time to flush write-throughs so entries
        # become evictable. Without this, the store loop outruns the bg writer
        # and every Reserve fails (entries never get ssd_offset set → can't be
        # demoted → AllocationFailed → store drops). 200ms is enough for the
        # bg writer to flush ~1024 entries across 4 NVMe drives.
        if store_drops > 0 and generation >= 3:
            time.sleep(0.2)

    wall = time.perf_counter() - wall_start
    # Collect SSD I/O stats delta to show write-through (demotion) and promote activity.
    io_after = manager._ring.get_io_stats()
    io_after = {k: io_after[k] - io_before.get(k, 0) for k in io_after}

    result = BenchResult(
        label=(
            f"Mixed Store+Load Under Eviction Pressure "
            f"(ws={working_set}, load_frac={load_fraction:.0%})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.print_report()
    print(f"  Eviction events (REMOVED): {total_evictions}  "
          f"Store drops (tier full): {store_drops}  "
          f"Load misses (evicted): {load_misses}")
    print(f"  SSD I/O: writes={io_after['write_ops']} ({io_after['write_bytes']/1e6:.0f} MB)  "
          f"reads={io_after['read_ops']} ({io_after['read_bytes']/1e6:.0f} MB)")
    if store_drops > 0 and io_after['write_ops'] == 0:
        print(f"  NOTE: Store drops with zero SSD writes means background DRAM→SSD")
        print(f"  write-through is not completing. Entries never become evictable.")
        print(f"  Check server --memory-tier-eviction-threshold and drive health.")
    return result


# ── mode: pipelined ──────────────────────────────────────────────────────────

def bench_pipelined(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    keys: list[bytes],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    pipeline_depth: int = 4,
    direction: str = "store",
) -> BenchResult:
    """Submit multiple batches before reaping to saturate the ThreadPoolExecutor.

    Instead of submit→spin→complete per batch, submit ``pipeline_depth``
    batches, then drain all completed jobs.  This reveals whether DMA
    operations actually overlap and whether the pool is the bottleneck.
    """
    is_store = direction == "store"
    label_dir = "Store" if is_store else "Load"
    phases = {
        "prepare": PhaseTiming("prepare"),
        "submit": PhaseTiming("submit"),
        "pipeline_drain": PhaseTiming("pipeline_drain"),
        "complete": PhaseTiming("complete"),
    }

    if not is_store:
        _populate_keys(manager, worker, keys, batch_size)

    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 600_000

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        batches = [keys[i : i + batch_size] for i in range(0, len(keys), batch_size)]
        in_flight: list[tuple[int, list]] = []  # (job_id, keys_for_complete)

        for batch_keys in batches:
            if is_store:
                t0 = time.perf_counter()
                manager.touch(batch_keys)
                result = manager.prepare_store(batch_keys)
                phases["prepare"].record(time.perf_counter() - t0)

                if result is None or not result.keys_to_store:
                    continue

                stored = result.keys_to_store
                gpu_ids = list(range(len(stored)))
                job_id += 1

                t0 = time.perf_counter()
                worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
                phases["submit"].record(time.perf_counter() - t0)

                in_flight.append((job_id, stored))
                total_blocks += len(stored)
            else:
                t0 = time.perf_counter()
                manager.touch(batch_keys)
                hits = [k for k in batch_keys if manager.lookup(k)]
                if not hits:
                    continue
                load_spec = manager.prepare_load(hits)
                phases["prepare"].record(time.perf_counter() - t0)

                gpu_ids = list(range(len(hits)))
                job_id += 1

                t0 = time.perf_counter()
                worker.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
                phases["submit"].record(time.perf_counter() - t0)

                in_flight.append((job_id, hits))
                total_blocks += len(hits)

            # Drain when pipeline is full.
            if len(in_flight) >= pipeline_depth:
                t0 = time.perf_counter()
                while in_flight:
                    finished = worker.get_finished()
                    if finished:
                        for _ in finished:
                            _, done_keys = in_flight.pop(0)
                            t_c = time.perf_counter()
                            if is_store:
                                manager.complete_store(done_keys, success=True)
                            else:
                                manager.complete_load(done_keys)
                            phases["complete"].record(time.perf_counter() - t_c)
                    else:
                        time.sleep(0.0001)
                phases["pipeline_drain"].record(time.perf_counter() - t0)

        # Drain remaining in-flight jobs.
        if in_flight:
            t0 = time.perf_counter()
            while in_flight:
                finished = worker.get_finished()
                if finished:
                    for _ in finished:
                        _, done_keys = in_flight.pop(0)
                        t_c = time.perf_counter()
                        if is_store:
                            manager.complete_store(done_keys, success=True)
                        else:
                            manager.complete_load(done_keys)
                        phases["complete"].record(time.perf_counter() - t_c)
                else:
                    time.sleep(0.0001)
            phases["pipeline_drain"].record(time.perf_counter() - t0)

        iteration += 1
        if is_store and (time.perf_counter() - wall_start) < min_duration:
            _remove_keys(manager._ring, keys, manager._world_size)

    wall = time.perf_counter() - wall_start
    return BenchResult(
        label=f"Pipelined {label_dir} (depth={pipeline_depth})",
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )


# ── mode: prefix-miss ────────────────────────────────────────────────────────

def bench_prefix_miss(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    hit_ratio: float = 0.75,
    num_blocks: int = 128,
) -> BenchResult:
    """Measure the cost of vLLM's _maximal_prefix_lookup at varying hit ratios.

    Pre-stores ``hit_ratio`` fraction of keys, then repeatedly runs
    touch(all) → per-key lookup breaking at the first miss.  Tracks
    lookup-cache-hit vs cache-miss-fallback latency separately.
    """
    all_keys = make_content_keys(num_blocks)
    present_count = max(0, min(num_blocks, int(num_blocks * hit_ratio)))
    present_keys = all_keys[:present_count]
    absent_keys = all_keys[present_count:]

    # Pre-store the "prefix" keys.
    if present_keys:
        _populate_keys(manager, worker, present_keys, batch_size)

    phases = {
        "touch": PhaseTiming("touch"),
        "lookup_hit": PhaseTiming("lookup_hit"),
        "lookup_miss": PhaseTiming("lookup_miss"),
        "prefix_scan": PhaseTiming("prefix_scan"),
        "load": PhaseTiming("load"),
        "store": PhaseTiming("store"),
    }

    total_scans = 0
    total_prefix_hits = 0
    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 700_000

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for batch_start in range(0, num_blocks, batch_size):
            batch = all_keys[batch_start : batch_start + batch_size]

            # Touch the full batch (populates lookup cache).
            t0 = time.perf_counter()
            manager.touch(batch)
            phases["touch"].record(time.perf_counter() - t0)

            # Maximal prefix scan: per-key lookup, break at first miss.
            t_scan = time.perf_counter()
            prefix_hits = []
            for k in batch:
                t_lk = time.perf_counter()
                hit = manager.lookup(k)
                elapsed = time.perf_counter() - t_lk
                if hit:
                    phases["lookup_hit"].record(elapsed)
                    prefix_hits.append(k)
                else:
                    phases["lookup_miss"].record(elapsed)
                    break
            phases["prefix_scan"].record(time.perf_counter() - t_scan)

            total_scans += 1
            total_prefix_hits += len(prefix_hits)

            # Complete the lifecycle: load hits, store the first miss.
            if prefix_hits:
                t0 = time.perf_counter()
                load_spec = manager.prepare_load(prefix_hits)
                gpu_ids = list(range(len(prefix_hits)))
                job_id += 1
                worker.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
                while not worker.get_finished():
                    time.sleep(0.0001)
                manager.complete_load(prefix_hits)
                phases["load"].record(time.perf_counter() - t0)
                total_blocks += len(prefix_hits)

        iteration += 1

    wall = time.perf_counter() - wall_start
    avg_prefix = total_prefix_hits / total_scans if total_scans else 0
    result = BenchResult(
        label=(
            f"Prefix-Miss Scan (hit_ratio={hit_ratio:.0%}, "
            f"avg_prefix={avg_prefix:.1f}/{batch_size})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.print_report()
    print(f"  Scans={total_scans}  avg_prefix_len={avg_prefix:.1f}  "
          f"lookup_cache_hits={len(phases['lookup_hit'].samples)}  "
          f"lookup_cache_misses={len(phases['lookup_miss'].samples)}")

    # Clean up stored keys.
    if present_keys:
        _remove_keys(manager._ring, present_keys, manager._world_size)

    return result


# ── mode: contention ─────────────────────────────────────────────────────────


def bench_contention(
    manager_store: ShmqCertusOffloadingManager,
    manager_load: ShmqCertusOffloadingManager,
    worker_store,
    worker_load,
    kv_regions: list[KvCacheIpc],
    ring: Ring,
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    working_set: int = 1024,
) -> list[BenchResult]:
    """Concurrent store + load streams on the same ring.

    Thread A stores new keys continuously.  Thread B loads keys that
    thread A has committed, received via a shared deque.  Both threads
    use separate manager/worker instances on the same Ring (thread-safe
    via per-thread channel assignment).
    """
    stop_event = threading.Event()
    committed_queue: collections.deque = collections.deque()
    barrier = threading.Barrier(2)

    store_phases = {
        "touch": PhaseTiming("touch"),
        "prepare_store": PhaseTiming("prepare_store"),
        "submit_store": PhaseTiming("submit_store"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_store": PhaseTiming("complete_store"),
    }
    load_phases = {
        "touch": PhaseTiming("touch"),
        "lookup": PhaseTiming("lookup"),
        "prepare_load": PhaseTiming("prepare_load"),
        "submit_load": PhaseTiming("submit_load"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_load": PhaseTiming("complete_load"),
    }

    store_state = {"total_blocks": 0, "drops": 0}
    load_state = {"total_blocks": 0, "misses": 0}

    base_seed = random.randint(10_000_000, 99_000_000)

    def _store_thread():
        barrier.wait()
        generation = 0
        job_id = 800_000
        while not stop_event.is_set():
            generation += 1
            batch_keys = make_content_keys(
                batch_size, seed=base_seed + 1_000_000 * generation,
            )

            t0 = time.perf_counter()
            manager_store.touch(batch_keys)
            store_phases["touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            result = manager_store.prepare_store(batch_keys)
            store_phases["prepare_store"].record(time.perf_counter() - t0)

            if result is None or not result.keys_to_store:
                store_state["drops"] += len(batch_keys)
                continue

            stored = result.keys_to_store
            if len(stored) < len(batch_keys):
                store_state["drops"] += len(batch_keys) - len(stored)

            gpu_ids = list(range(len(stored)))
            job_id += 1

            t0 = time.perf_counter()
            worker_store.submit_store(
                job_id, _FakeGPUSpec(gpu_ids), result.store_spec,
            )
            store_phases["submit_store"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker_store.get_finished():
                time.sleep(0.0001)
            store_phases["get_finished"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager_store.complete_store(stored, success=True)
            store_phases["complete_store"].record(time.perf_counter() - t0)

            store_state["total_blocks"] += len(stored)
            committed_queue.append(batch_keys)

    def _load_thread():
        barrier.wait()
        time.sleep(0.5)  # let stores seed some keys
        job_id = 900_000
        while not stop_event.is_set():
            try:
                batch_keys = committed_queue.popleft()
            except IndexError:
                time.sleep(0.001)
                continue

            t0 = time.perf_counter()
            manager_load.touch(batch_keys)
            load_phases["touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            hits = [k for k in batch_keys if manager_load.lookup(k)]
            load_phases["lookup"].record(time.perf_counter() - t0)

            if not hits:
                load_state["misses"] += len(batch_keys)
                continue
            load_state["misses"] += len(batch_keys) - len(hits)

            t0 = time.perf_counter()
            load_spec = manager_load.prepare_load(hits)
            load_phases["prepare_load"].record(time.perf_counter() - t0)

            gpu_ids = list(range(len(hits)))
            job_id += 1

            t0 = time.perf_counter()
            worker_load.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
            load_phases["submit_load"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker_load.get_finished():
                time.sleep(0.0001)
            load_phases["get_finished"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager_load.complete_load(hits)
            load_phases["complete_load"].record(time.perf_counter() - t0)

            load_state["total_blocks"] += len(hits)

    t_store = threading.Thread(target=_store_thread, name="contention-store")
    t_load = threading.Thread(target=_load_thread, name="contention-load")

    wall_start = time.perf_counter()
    t_store.start()
    t_load.start()

    time.sleep(min_duration)
    stop_event.set()

    t_store.join(timeout=10)
    t_load.join(timeout=10)
    wall = time.perf_counter() - wall_start

    # Drain events on main thread.
    total_evictions = 0
    for ev in manager_store.take_events():
        total_evictions += len(ev.keys) if hasattr(ev, "keys") else 0

    store_result = BenchResult(
        label="Contention: Store Thread",
        total_blocks=store_state["total_blocks"],
        total_bytes=store_state["total_blocks"] * block_bytes,
        wall_seconds=wall,
        phases=store_phases,
    )
    load_result = BenchResult(
        label="Contention: Load Thread",
        total_blocks=load_state["total_blocks"],
        total_bytes=load_state["total_blocks"] * block_bytes,
        wall_seconds=wall,
        phases=load_phases,
    )

    store_result.print_report()
    print(f"  Store drops: {store_state['drops']}")
    load_result.print_report()
    print(f"  Load misses: {load_state['misses']}  Evictions: {total_evictions}")

    return [store_result, load_result]


# ── mode: scheduler-step ─────────────────────────────────────────────────────

def bench_scheduler_step(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    ring: Ring,
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    requests_per_step: int = 8,
    pipeline_depth: int = 4,
    num_sessions: int = 4,
    num_blocks: int = 128,
) -> BenchResult:
    """Simulate continuous-batching scheduler steps with prefix sharing.

    Each step admits ``requests_per_step`` requests drawn from
    ``num_sessions`` conversations.  Earlier-stored keys are shared
    prefix (should hit on lookup); new keys are suffix (miss → store).
    Dispatches are pipelined through the worker.
    """
    keys_per_request = max(4, num_blocks // requests_per_step)
    prefix_len = keys_per_request // 2
    suffix_len = keys_per_request - prefix_len

    phases = {
        "touch_lookup": PhaseTiming("touch_lookup"),
        "prepare_store": PhaseTiming("prepare_store"),
        "prepare_load": PhaseTiming("prepare_load"),
        "submit": PhaseTiming("submit"),
        "drain_workers": PhaseTiming("drain_workers"),
        "complete": PhaseTiming("complete"),
        "take_events": PhaseTiming("take_events"),
    }

    # Seed each session's prefix into the server.
    session_prefixes: list[list[bytes]] = []
    for s in range(num_sessions):
        prefix_keys = make_content_keys(prefix_len, seed=s * 10_000)
        _populate_keys(manager, worker, prefix_keys, batch_size)
        session_prefixes.append(prefix_keys)

    total_blocks = 0
    total_steps = 0
    total_prefix_hits = 0
    total_suffix_stores = 0
    wall_start = time.perf_counter()
    job_id = 1_000_000

    while (time.perf_counter() - wall_start) < min_duration or total_steps < 3:
        step_start = time.perf_counter()

        # Build requests for this step.
        all_load_keys: list[bytes] = []
        all_store_keys: list[bytes] = []

        t_tl = time.perf_counter()
        for r in range(requests_per_step):
            s_idx = (total_steps * requests_per_step + r) % num_sessions
            prefix = session_prefixes[s_idx]
            suffix = make_content_keys(
                suffix_len,
                seed=s_idx * 10_000 + 5_000 + total_steps * 100 + r,
            )
            req_keys = prefix + suffix

            manager.touch(req_keys)

            # Maximal prefix scan.
            prefix_hits = []
            for k in req_keys:
                if manager.lookup(k):
                    prefix_hits.append(k)
                else:
                    break

            suffix_misses = req_keys[len(prefix_hits):]
            all_load_keys.extend(prefix_hits)
            all_store_keys.extend(suffix_misses)
            total_prefix_hits += len(prefix_hits)
            total_suffix_stores += len(suffix_misses)

        phases["touch_lookup"].record(time.perf_counter() - t_tl)

        # Prepare stores.
        store_result = None
        if all_store_keys:
            t0 = time.perf_counter()
            store_result = manager.prepare_store(all_store_keys)
            phases["prepare_store"].record(time.perf_counter() - t0)

        # Prepare loads.
        load_spec = None
        if all_load_keys:
            t0 = time.perf_counter()
            load_spec = manager.prepare_load(all_load_keys)
            phases["prepare_load"].record(time.perf_counter() - t0)

        # Dispatch (pipelined).
        t0 = time.perf_counter()
        stored_keys = None
        if store_result and store_result.keys_to_store:
            stored_keys = store_result.keys_to_store
            gpu_ids = list(range(len(stored_keys)))
            job_id += 1
            worker.submit_store(
                job_id, _FakeGPUSpec(gpu_ids), store_result.store_spec,
            )
        if load_spec and all_load_keys:
            gpu_ids = list(range(len(all_load_keys)))
            job_id += 1
            worker.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
        phases["submit"].record(time.perf_counter() - t0)

        # Drain workers.
        t0 = time.perf_counter()
        pending = (1 if stored_keys else 0) + (1 if (load_spec and all_load_keys) else 0)
        drained = 0
        while drained < pending:
            finished = worker.get_finished()
            if finished:
                drained += len(finished)
            else:
                time.sleep(0.0001)
        phases["drain_workers"].record(time.perf_counter() - t0)

        # Complete.
        t0 = time.perf_counter()
        if stored_keys:
            manager.complete_store(stored_keys, success=True)
            total_blocks += len(stored_keys)
        if all_load_keys:
            manager.complete_load(all_load_keys)
            total_blocks += len(all_load_keys)
        phases["complete"].record(time.perf_counter() - t0)

        # Drain events.
        t0 = time.perf_counter()
        for ev in manager.take_events():
            pass
        phases["take_events"].record(time.perf_counter() - t0)

        total_steps += 1

        # Remove suffix keys so next step's stores don't dedup.
        if all_store_keys:
            _remove_keys(ring, all_store_keys, manager._world_size)

    wall = time.perf_counter() - wall_start
    result = BenchResult(
        label=(
            f"Scheduler Step (reqs={requests_per_step}, "
            f"sessions={num_sessions}, keys/req={keys_per_request})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.print_report()
    print(f"  Steps={total_steps}  steps/s={total_steps/wall:.1f}  "
          f"prefix_hits={total_prefix_hits}  suffix_stores={total_suffix_stores}")

    # Clean up session prefixes.
    for prefix in session_prefixes:
        _remove_keys(ring, prefix, manager._world_size)

    return result


# ── pattern-driven mode (same YAML patterns as certus_fio) ──────────────────

def _load_pattern(name: str, overrides: dict | None = None):
    """Load a workload pattern YAML, reusing certus_fio's WorkloadPattern."""
    sys.path.insert(0, os.path.join(_repo, "tools", "certus-fio"))
    from certus_fio import WorkloadPattern, eval_expr
    patterns_dir = os.path.join(_repo, "knowledge", "workload_patterns")
    path = os.path.join(patterns_dir, f"{name}.yaml")
    if not os.path.exists(path):
        candidates = [f[:-5] for f in os.listdir(patterns_dir) if f.endswith(".yaml")]
        print(f"Pattern '{name}' not found. Available:", file=sys.stderr)
        for c in sorted(candidates):
            print(f"  {c}", file=sys.stderr)
        sys.exit(1)
    return WorkloadPattern(path, overrides), eval_expr


def bench_pattern(
    pattern,
    eval_expr_fn,
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    ring: Ring,
    batch_size: int,
    block_bytes_override: int | None,
    min_duration: float,
    tp: int,
) -> list[BenchResult]:
    """Run a certus_fio YAML pattern through the connector lifecycle.

    Translates each pattern phase's store/load/store_3phase/load_3phase ops
    into the full manager+worker lifecycle, using the pattern's keyspace
    sizing and parameters. Integer keys are used (same as certus_fio) so the
    numbers are directly comparable — the only difference is the code path.
    """
    results = []
    key_base = 10_000_000

    for phase_def in pattern.phases:
        phase_id = phase_def["id"]
        ops = phase_def.get("operations", [])

        for op_def in ops:
            op = op_def["op"]
            ks_name = op_def.get("keys", list(pattern.keyspaces.keys())[0])
            ks = pattern.keyspaces[ks_name]
            object_bytes = block_bytes_override or ks["object_bytes"]
            cardinality = ks["cardinality"]
            repeat = eval_expr_fn(op_def.get("repeat", cardinality), pattern.params)
            bs = eval_expr_fn(op_def.get("batch_size", batch_size), pattern.params)
            if bs > repeat:
                bs = repeat

            ks_offset = list(pattern.keyspaces.keys()).index(ks_name) * 10_000_000
            all_keys = list(range(key_base + ks_offset, key_base + ks_offset + cardinality))
            # Cycle keys to reach repeat count (same as certus_fio).
            import itertools
            keys_int = list(itertools.islice(itertools.cycle(all_keys), repeat))

            # Wrap ints as the manager expects (it calls _key_to_u64, which
            # passes ints through unchanged — same path as certus_fio's
            # integer keys, no BLAKE2b hashing overhead).
            keys = keys_int

            # Update manager block size to match pattern's object_bytes.
            manager.set_block_size_bytes(object_bytes)

            if op in ("store", "store_3phase"):
                # Precondition: keys should not exist. Clean up first.
                _remove_int_keys(ring, all_keys, tp)

                result = _bench_pattern_store(
                    manager, worker, keys, bs, object_bytes, min_duration, ring, tp,
                )
                result.label = f"[{pattern.id}] {phase_id}/{op} (connector lifecycle)"
                result.print_report()
                results.append(result)

            elif op in ("load", "load_3phase"):
                # Precondition check: if pattern says present_in_store, populate.
                for pc in pattern.preconditions:
                    if pc["subject"] == ks_name and pc["state"] == "present_in_store":
                        _populate_int_keys(manager, worker, all_keys, bs, ring, tp)
                        break

                # Check if this is a cold-load pattern (keys absent from local cache).
                is_cold = any(
                    pc["subject"] == ks_name and pc["state"] == "absent_from_local_cache"
                    for pc in pattern.preconditions
                )

                result = _bench_pattern_load(
                    manager, worker, keys, bs, object_bytes, min_duration,
                    cold=is_cold,
                )
                result.label = f"[{pattern.id}] {phase_id}/{op} {'cold' if is_cold else 'warm'} (connector lifecycle)"
                result.print_report()
                results.append(result)

        # Clean up between phases.
        for ks_name, ks in pattern.keyspaces.items():
            ks_offset = list(pattern.keyspaces.keys()).index(ks_name) * 10_000_000
            ks_keys = list(range(key_base + ks_offset, key_base + ks_offset + ks["cardinality"]))
            _remove_int_keys(ring, ks_keys, tp)

    return results


def _bench_pattern_store(manager, worker, keys, batch_size, block_bytes, min_duration, ring, tp):
    """Store keys through the full connector lifecycle, with certus_fio-compatible timing."""
    phases = {
        "touch": PhaseTiming("touch"),
        "prepare_store": PhaseTiming("prepare_store"),
        "submit_store": PhaseTiming("submit_store"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_store": PhaseTiming("complete_store"),
    }
    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 200_000

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for i in range(0, len(keys), batch_size):
            batch = keys[i : i + batch_size]

            t0 = time.perf_counter()
            manager.touch(batch)
            phases["touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            result = manager.prepare_store(batch)
            phases["prepare_store"].record(time.perf_counter() - t0)

            if result is None or not result.keys_to_store:
                continue

            stored = result.keys_to_store
            gpu_ids = list(range(len(stored)))
            job_id += 1

            t0 = time.perf_counter()
            worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
            phases["submit_store"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker.get_finished():
                time.sleep(0.0001)
            phases["get_finished"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager.complete_store(stored, success=True)
            phases["complete_store"].record(time.perf_counter() - t0)

            total_blocks += len(stored)

        iteration += 1
        if (time.perf_counter() - wall_start) < min_duration:
            _remove_int_keys(ring, keys, tp)

    wall = time.perf_counter() - wall_start
    return BenchResult(
        label="pattern store",
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )


def _bench_pattern_load(manager, worker, keys, batch_size, block_bytes, min_duration, cold=False):
    """Load keys through the full connector lifecycle."""
    phases = {
        "touch": PhaseTiming("touch"),
        "lookup": PhaseTiming("lookup"),
        "prepare_load": PhaseTiming("prepare_load"),
        "submit_load": PhaseTiming("submit_load"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_load": PhaseTiming("complete_load"),
    }
    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 300_000

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        if cold and iteration > 0:
            try:
                manager._ring.flush_to_ssd()
                manager._ring.clear_memory_tier()
            except Exception:
                pass

        for i in range(0, len(keys), batch_size):
            batch = keys[i : i + batch_size]

            t0 = time.perf_counter()
            manager.touch(batch)
            phases["touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            hits = [k for k in batch if manager.lookup(k)]
            phases["lookup"].record(time.perf_counter() - t0)

            if not hits:
                continue

            t0 = time.perf_counter()
            load_spec = manager.prepare_load(hits)
            phases["prepare_load"].record(time.perf_counter() - t0)

            gpu_ids = list(range(len(hits)))
            job_id += 1

            t0 = time.perf_counter()
            worker.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
            phases["submit_load"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker.get_finished():
                time.sleep(0.0001)
            phases["get_finished"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager.complete_load(hits)
            phases["complete_load"].record(time.perf_counter() - t0)

            total_blocks += len(hits)

        iteration += 1

    wall = time.perf_counter() - wall_start
    return BenchResult(
        label="pattern load",
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )


def _remove_int_keys(ring: Ring, keys: list[int], world_size: int):
    """Remove integer keys (no BLAKE2b hashing — same keyspace as certus_fio)."""
    from certus_shmq_connector.mediums import ns_key
    expanded = []
    for k in keys:
        for r in range(world_size):
            expanded.append(ns_key(k, r, world_size))
    if expanded:
        try:
            ring.remove(expanded)
        except Exception:
            pass


def _populate_int_keys(manager, worker, keys, batch_size, ring, tp):
    """Store integer keys so they're available for load benchmarks."""
    _remove_int_keys(ring, keys, tp)
    job_id = 400_000
    for i in range(0, len(keys), batch_size):
        batch = keys[i : i + batch_size]
        manager.touch(batch)
        result = manager.prepare_store(batch)
        if result is None or not result.keys_to_store:
            continue
        stored = result.keys_to_store
        gpu_ids = list(range(len(stored)))
        job_id += 1
        worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
        while not worker.get_finished():
            time.sleep(0.0001)
        manager.complete_store(stored, success=True)


# ── helpers ─────────────────────────────────────────────────────────────────

class _FakeGPUSpec:
    """Minimal stand-in for GPULoadStoreSpec. The worker only reads .block_ids."""
    def __init__(self, block_ids: list[int]):
        self.block_ids = block_ids


def _remove_keys(ring: Ring, keys: list[bytes], world_size: int):
    """Remove all server entries for these keys (all TP ranks)."""
    from certus_shmq_connector.manager import _key_to_u64
    from certus_shmq_connector.mediums import ns_key

    int_keys = [_key_to_u64(k) for k in keys]
    expanded = []
    for k in int_keys:
        for r in range(world_size):
            expanded.append(ns_key(k, r, world_size))
    if expanded:
        try:
            ring.remove(expanded)
        except Exception:
            pass


def _populate_keys(
    manager: ShmqCertusOffloadingManager,
    worker,
    keys: list[bytes],
    batch_size: int,
):
    """Store all keys so they're available for load benchmarks."""
    job_id = 500_000
    for i in range(0, len(keys), batch_size):
        batch = keys[i : i + batch_size]
        manager.touch(batch)
        result = manager.prepare_store(batch)
        if result is None or not result.keys_to_store:
            continue
        stored = result.keys_to_store
        gpu_ids = list(range(len(stored)))
        job_id += 1
        worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
        while not worker.get_finished():
            time.sleep(0.0001)
        manager.complete_store(stored, success=True)


# ── main ────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="End-to-end connector lifecycle benchmark",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Modes (--mode):\n"
            "\n"
            "  default        Four serial phases: store, warm load, cold load, mixed eviction\n"
            "  pipelined      Submit N batches before reaping (saturate the worker pool)\n"
            "  prefix-miss    Vary prefix hit ratio to measure _maximal_prefix_lookup cost\n"
            "  contention     Concurrent store + load threads on the same ring\n"
            "  scheduler-step Full scheduler step simulation with multi-request prefix sharing\n"
            "\n"
            "Without --mode, runs the default 4-phase lifecycle (or --pattern if given).\n"
            "\n"
            "Unlike certus_fio (which calls Ring.lookup directly), this exercises\n"
            "key namespacing, region construction, ThreadPoolExecutor dispatch,\n"
            "BLAKE2b key hashing, the lookup cache, and eviction-event draining."
        ),
    )
    parser.add_argument("--pattern", type=str, default=None,
                        help="Run a certus_fio YAML pattern through the connector "
                        "(e.g. cold_prefill_store, warm_prefill_load_and_suffix_store)")
    parser.add_argument("--override", nargs="*", default=[],
                        help="Pattern parameter overrides (e.g. requested_blocks=256)")
    parser.add_argument("--shm-path", default="/dev/shm/certus-shmq",
                        help="certus-server shmq mailbox path")
    parser.add_argument("--bs", type=int, default=16,
                        help="Batch size (blocks per batch)")
    parser.add_argument("--num-blocks", type=int, default=128,
                        help="Total blocks per iteration")
    parser.add_argument("--block-bytes", type=int, default=DEFAULT_BLOCK_BYTES,
                        help=f"Per-block size in bytes (default: {DEFAULT_BLOCK_BYTES})")
    parser.add_argument("--gpu", type=int, default=0,
                        help="CUDA device index")
    parser.add_argument("--tp", type=int, default=1,
                        help="Simulated tensor-parallel world size")
    parser.add_argument("--min-duration", type=float, default=5.0,
                        help="Minimum seconds per benchmark phase")
    parser.add_argument("--warmup", type=int, default=4,
                        help="Warmup iterations before measurement")
    parser.add_argument("--workers", type=int, default=4,
                        help="ThreadPoolExecutor worker count")
    parser.add_argument("--working-set", type=int, default=512,
                        help="Working set size for eviction-pressure mode (blocks)")
    parser.add_argument("--csv", type=str, default=None,
                        help="Append results to CSV file")
    parser.add_argument("--tag", type=str, default="",
                        help="Tag column for CSV (e.g. branch name)")
    parser.add_argument("--no-cold", action="store_true",
                        help="Skip the cold-load benchmark (SSD→DRAM→GPU)")
    parser.add_argument("--no-eviction", action="store_true",
                        help="Skip the eviction-pressure benchmark")
    parser.add_argument("--mode", type=str, default=None,
                        choices=["default", "pipelined", "prefix-miss",
                                 "contention", "scheduler-step"],
                        help="Benchmark mode (omit for default 4-phase or --pattern)")
    parser.add_argument("--pipeline-depth", type=int, default=4,
                        help="(pipelined/scheduler-step) Batches in-flight before reaping")
    parser.add_argument("--direction", type=str, default="both",
                        choices=["store", "load", "both"],
                        help="(pipelined) Which direction to benchmark")
    parser.add_argument("--hit-ratio", type=float, default=0.75,
                        help="(prefix-miss) Fraction of keys pre-stored (0.0-1.0)")
    parser.add_argument("--requests-per-step", type=int, default=8,
                        help="(scheduler-step) Requests per scheduler step")
    parser.add_argument("--num-sessions", type=int, default=4,
                        help="(scheduler-step) Distinct conversations in the pool")
    args = parser.parse_args()

    torch.cuda.set_device(args.gpu)

    print(f"Connector Lifecycle Benchmark")
    print(f"  shm_path={args.shm_path}  gpu={args.gpu}  tp={args.tp}")
    print(f"  num_blocks={args.num_blocks}  bs={args.bs}  block_bytes={args.block_bytes}")
    print(f"  min_duration={args.min_duration}s  workers={args.workers}")

    # ── set up ring + connector objects ──
    ring = Ring(args.shm_path, ready_timeout=10.0, log=lambda msg: None)

    manager = ShmqCertusOffloadingManager(
        ring, block_size_bytes=args.block_bytes, world_size=args.tp,
    )

    # Allocate enough GPU memory for all block_ids the benchmark will use.
    # The eviction mode uses --working-set blocks; use the max of that and num_blocks.
    max_blocks = max(args.num_blocks, args.working_set)
    store_region, store_tensor = make_kv_region(args.gpu, args.block_bytes, max_blocks)
    load_region, load_tensor = make_kv_region(args.gpu, args.block_bytes, max_blocks)

    executor = ThreadPoolExecutor(max_workers=args.workers, thread_name_prefix="bench-shmq")
    Worker = worker_class()
    worker = Worker(
        ring, [store_region], args.block_bytes, executor,
        rank=0, world_size=args.tp,
    )

    keys = make_content_keys(args.num_blocks)

    # ── warmup ──
    print(f"\nWarmup ({args.warmup} store/remove cycles)...")
    warmup_keys = make_content_keys(args.warmup, seed=99999)
    for wk in warmup_keys:
        try:
            _populate_keys(manager, worker, [wk], 1)
        except Exception:
            pass
    _remove_keys(ring, warmup_keys, args.tp)

    # ── mode dispatch ──
    mode = args.mode
    if mode is None:
        mode = "pattern" if args.pattern else "default"

    all_results: list[BenchResult] = []

    if mode == "pattern":
        overrides = {}
        for ov in args.override:
            if "=" in ov:
                k, v = ov.split("=", 1)
                overrides[k] = v
        pattern, eval_expr_fn = _load_pattern(args.pattern, overrides)
        print(f"\nPattern: {pattern.id} ({pattern.name})")
        pattern.describe()

        all_results = bench_pattern(
            pattern, eval_expr_fn, manager, worker, [store_region],
            ring, args.bs, args.block_bytes if args.block_bytes != DEFAULT_BLOCK_BYTES else None,
            args.min_duration, args.tp,
        )

    elif mode == "pipelined":
        print(f"\n  Mode: pipelined (depth={args.pipeline_depth}, "
              f"direction={args.direction})")
        if args.direction in ("store", "both"):
            r = bench_pipelined(
                manager, worker, [store_region], keys,
                args.bs, args.block_bytes, args.min_duration,
                pipeline_depth=args.pipeline_depth, direction="store",
            )
            r.print_report()
            all_results.append(r)
        if args.direction in ("load", "both"):
            _remove_keys(ring, keys, args.tp)
            r = bench_pipelined(
                manager, worker, [store_region], keys,
                args.bs, args.block_bytes, args.min_duration,
                pipeline_depth=args.pipeline_depth, direction="load",
            )
            r.print_report()
            all_results.append(r)

    elif mode == "prefix-miss":
        print(f"\n  Mode: prefix-miss (hit_ratio={args.hit_ratio})")
        r = bench_prefix_miss(
            manager, worker, [store_region],
            args.bs, args.block_bytes, args.min_duration,
            hit_ratio=args.hit_ratio,
            num_blocks=args.num_blocks,
        )
        all_results.append(r)

    elif mode == "contention":
        print(f"\n  Mode: contention (working_set={args.working_set})")
        manager_load = ShmqCertusOffloadingManager(
            ring, block_size_bytes=args.block_bytes, world_size=args.tp,
        )
        executor_load = ThreadPoolExecutor(
            max_workers=args.workers, thread_name_prefix="bench-load",
        )
        Worker = worker_class()
        worker_load = Worker(
            ring, [load_region], args.block_bytes, executor_load,
            rank=0, world_size=args.tp,
        )
        all_results = bench_contention(
            manager, manager_load, worker, worker_load,
            [store_region], ring,
            args.bs, args.block_bytes, args.min_duration,
            working_set=args.working_set,
        )
        executor_load.shutdown(wait=False)

    elif mode == "scheduler-step":
        print(f"\n  Mode: scheduler-step (reqs={args.requests_per_step}, "
              f"sessions={args.num_sessions})")
        r = bench_scheduler_step(
            manager, worker, [store_region], ring,
            args.bs, args.block_bytes, args.min_duration,
            requests_per_step=args.requests_per_step,
            pipeline_depth=args.pipeline_depth,
            num_sessions=args.num_sessions,
            num_blocks=args.num_blocks,
        )
        all_results.append(r)

    else:
        # ── default mode: hardcoded store/load/eviction lifecycle ──
        store_result = bench_store_lifecycle(
            manager, worker, [store_region], keys,
            args.bs, args.block_bytes, args.min_duration,
        )
        store_result.print_report()

        _remove_keys(ring, keys, args.tp)
        _populate_keys(manager, worker, keys, args.bs)

        warm_result = bench_load_lifecycle(
            manager, worker, [load_region], keys,
            args.bs, args.block_bytes, args.min_duration,
            cold=False,
        )
        warm_result.print_report()

        cold_result = None
        if not args.no_cold:
            cold_result = bench_load_lifecycle(
                manager, worker, [load_region], keys,
                args.bs, args.block_bytes, args.min_duration,
                cold=True,
            )
            cold_result.print_report()

        eviction_result = None
        if not args.no_eviction:
            _remove_keys(ring, keys, args.tp)
            ring.clear_memory_tier()
            eviction_result = bench_mixed_eviction(
                manager, worker, [store_region],
                args.bs, args.block_bytes, args.min_duration,
                working_set=args.working_set,
                load_fraction=0.5,
            )

        all_results = [store_result, warm_result]
        if cold_result:
            all_results.append(cold_result)
        if eviction_result:
            all_results.append(eviction_result)

    # ── CSV output ──
    if args.csv and all_results:
        extra = {
            "tag": args.tag,
            "mode": mode,
            "shm_path": args.shm_path,
            "gpu": args.gpu,
            "tp": args.tp,
            "bs": args.bs,
            "num_blocks": args.num_blocks,
            "block_bytes": args.block_bytes,
            "workers": args.workers,
            "working_set": args.working_set,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        }
        if mode == "pipelined":
            extra["pipeline_depth"] = args.pipeline_depth
            extra["direction"] = args.direction
        elif mode == "prefix-miss":
            extra["hit_ratio"] = args.hit_ratio
        elif mode == "scheduler-step":
            extra["requests_per_step"] = args.requests_per_step
            extra["num_sessions"] = args.num_sessions
        elif mode == "pattern":
            extra["pattern"] = args.pattern

        rows = [r.csv_row(extra) for r in all_results]
        write_header = not os.path.exists(args.csv)
        fieldnames = list(rows[0].keys())
        for r in rows[1:]:
            for k in r:
                if k not in fieldnames:
                    fieldnames.append(k)
        with open(args.csv, "a", newline="") as f:
            w = csv.DictWriter(f, fieldnames=fieldnames, extrasaction="ignore")
            if write_header:
                w.writeheader()
            w.writerows(rows)
        print(f"\nResults appended to {args.csv}")

    # ── cleanup ──
    if mode not in ("pattern", "contention"):
        _remove_keys(ring, keys, args.tp)
    executor.shutdown(wait=False)
    ring.close()

    print(f"\nDone.")


if __name__ == "__main__":
    main()
