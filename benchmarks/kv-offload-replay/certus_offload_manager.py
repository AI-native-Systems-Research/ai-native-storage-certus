"""certus_offload_manager.py — OffloadingManager-shaped wrapper over CertusEngine.

Exposes the 6 scheduler-side primitives from vllm.v1.kv_offload.abstract against
the real Rust CertusEngine (SPDK + NVMe):

    lookup(keys)          → engine.batch_check
    prepare_load(keys)    → reserves GPU block ids + submits engine.load_async
    complete_load(load)   → engine.wait_job (IO settled ⇒ refs drop)
    prepare_store(keys)   → engine.prepare_store (returns to_store, evicted)
    complete_store(...)   → engine.store_async + wait_job + engine.complete_store
    touch(keys)           → engine.touch

Keys are 8-byte unsigned ints (the engine's CacheKey type). The driver owns
the block-hash→key mapping; we just carry u64s through.
"""

from __future__ import annotations

import json
import time
from collections import OrderedDict
from dataclasses import dataclass, field
from typing import Iterable, TextIO

import torch

import certus_native


# NVMe slab / DRAM block size. Must match engine.gpu_block_size / slab_size.
NATIVE_BLOCK_BYTES = 131072


@dataclass
class LoadHandle:
    job_id: int
    keys: list[int]
    gpu_block_ids: list[int]
    t_submit: float


@dataclass
class StoreHandle:
    job_id: int
    keys: list[int]          # keys_to_store returned by prepare_store
    evicted: list[int]       # keys evicted to make room
    gpu_block_ids: list[int]
    t_submit: float


@dataclass
class ManagerMetrics:
    lookups: int = 0
    lookup_blocks_requested: int = 0
    lookup_blocks_hit: int = 0

    loads: int = 0
    load_blocks: int = 0
    load_latency_ms: list[float] = field(default_factory=list)

    stores_attempted: int = 0
    stores_rejected: int = 0
    store_blocks_accepted: int = 0
    store_blocks_already_present: int = 0
    store_latency_ms: list[float] = field(default_factory=list)

    evicted_keys_total: int = 0
    evicted_key_stream: list[tuple[float, int]] = field(default_factory=list)
    # (sim_time_s, key) — used for eviction-regret scoring

    touches: int = 0
    touch_blocks: int = 0

    pin_depth_max: int = 0
    pin_depth_cur: int = 0


class PinnedBlockPool:
    """Rotating pool of page-aligned pinned blocks, used as gpu_block_ids.

    The engine interprets each gpu_block_id as `ptr // NATIVE_BLOCK_BYTES`.
    Content is irrelevant for load/store benchmarking — we reuse slots in a ring.
    """

    def __init__(self, capacity_blocks: int):
        total = capacity_blocks * NATIVE_BLOCK_BYTES
        self.buf = torch.empty(total, dtype=torch.uint8).pin_memory()
        base = self.buf.data_ptr()
        if base % NATIVE_BLOCK_BYTES != 0:
            raise RuntimeError(
                f"pinned buffer base 0x{base:x} not {NATIVE_BLOCK_BYTES}-aligned"
            )
        self.base = base
        self.capacity = capacity_blocks
        self.cursor = 0

    def take(self, n: int) -> list[int]:
        ids = []
        for _ in range(n):
            ptr = self.base + self.cursor * NATIVE_BLOCK_BYTES
            ids.append(ptr // NATIVE_BLOCK_BYTES)
            self.cursor = (self.cursor + 1) % self.capacity
        return ids


class CertusOffloadManager:
    """OffloadingManager over a running CertusEngine, with Python-side LRU.

    The native engine currently has no eviction (see engine.rs: "evicted is
    always empty — the dispatcher handles allocation"). To measure eviction
    regret we overlay a virtual capacity cap here: an LRU of stored keys with
    ref-counts. When prepare_store would push us over capacity, we evict LRU
    unpinned keys in Python — the bytes remain physically on NVMe but the
    manager stops acknowledging them to lookup().

    Set virtual_capacity_blocks=0 to disable the cap (trust the engine).
    """

    def __init__(
        self,
        engine: certus_native.CertusEngine,
        pinned_pool_blocks: int = 16384,
        virtual_capacity_blocks: int = 0,
        trace_file: TextIO | None = None,
        handler_trace_file: TextIO | None = None,
    ):
        self.engine = engine
        self.pool = PinnedBlockPool(pinned_pool_blocks)
        self.metrics = ManagerMetrics()
        self._next_job_id = 1
        self._now_s = 0.0  # driver updates this as simulated time advances

        self.virtual_capacity = virtual_capacity_blocks
        # Python-side view: key → ref_cnt (0 = evictable). OrderedDict gives LRU.
        # Presence in this dict means "logically stored & reachable via lookup".
        self._present: OrderedDict[int, int] = OrderedDict()

        self._trace_file = trace_file
        self._handler_trace_file = handler_trace_file
        # Per-request context stamped on every op line
        self._ctx_req: int = -1
        self._ctx_conv_id: str = ""
        self._ctx_turn_idx: int = -1

    def set_request_context(self, req_idx: int, conv_id: str, turn_idx: int) -> None:
        self._ctx_req = req_idx
        self._ctx_conv_id = conv_id
        self._ctx_turn_idx = turn_idx

    def _log(self, op: str, **fields) -> None:
        if self._trace_file is None:
            return
        rec = {
            "t": round(self._now_s, 6),
            "req": self._ctx_req,
            "conv_id": self._ctx_conv_id,
            "turn_idx": self._ctx_turn_idx,
            "op": op,
            **fields,
        }
        self._trace_file.write(json.dumps(rec) + "\n")

    def _hlog(self, op: str, wall_t: float, **fields) -> None:
        """Handler-level (data-movement) trace. Separate stream from scheduler log.

        Wall timestamps are real perf_counter seconds since the run started,
        relative to the first call (set on first emission). This captures the
        actual submit/completion order and IO service times — the scheduler
        log uses simulated arrival time instead.
        """
        if self._handler_trace_file is None:
            return
        if not hasattr(self, "_hlog_t0"):
            self._hlog_t0 = wall_t
        rec = {
            "wall_s": round(wall_t - self._hlog_t0, 6),
            "sim_s": round(self._now_s, 6),
            "req": self._ctx_req,
            "conv_id": self._ctx_conv_id,
            "turn_idx": self._ctx_turn_idx,
            "op": op,
            **fields,
        }
        self._handler_trace_file.write(json.dumps(rec) + "\n")

    def set_sim_time(self, t_s: float) -> None:
        self._now_s = t_s

    def _jid(self) -> int:
        jid = self._next_job_id
        self._next_job_id += 1
        return jid

    # ── OffloadingManager interface ────────────────────────────────────────

    def lookup(self, keys: list[int]) -> int:
        """Max prefix of `keys` currently offloaded (ready to load)."""
        if not keys:
            return 0
        if self.virtual_capacity > 0:
            # Python-side view: walk prefix against _present.
            hit = 0
            for k in keys:
                if k not in self._present:
                    break
                hit += 1
        else:
            hit = int(self.engine.batch_check(keys))
        self.metrics.lookups += 1
        self.metrics.lookup_blocks_requested += len(keys)
        self.metrics.lookup_blocks_hit += hit
        self._log("lookup", keys=keys, hit=hit)
        return hit

    def touch(self, keys: list[int]) -> None:
        if not keys:
            return
        self.engine.touch(keys)
        if self.virtual_capacity > 0:
            for k in keys:
                if k in self._present:
                    self._present.move_to_end(k)
        self.metrics.touches += 1
        self.metrics.touch_blocks += len(keys)
        self._log("touch", keys=keys)

    def prepare_load(self, keys: list[int]) -> LoadHandle:
        """Pin `keys` and submit NVMe/DRAM→GPU transfer. Non-blocking."""
        assert keys, "prepare_load called with empty key list"
        gpu_ids = self.pool.take(len(keys))
        jid = self._jid()
        t0 = time.perf_counter()
        self._hlog("load_submit", t0, job=jid, keys=keys,
                   gpu_block_ids=gpu_ids, bytes=len(keys) * NATIVE_BLOCK_BYTES)
        self.engine.load_async(jid, gpu_ids, keys)
        if self.virtual_capacity > 0:
            for k in keys:
                self._present[k] = self._present.get(k, 0) + 1
                self._present.move_to_end(k)
        self.metrics.pin_depth_cur += len(keys)
        if self.metrics.pin_depth_cur > self.metrics.pin_depth_max:
            self.metrics.pin_depth_max = self.metrics.pin_depth_cur
        self._log("prepare_load", keys=keys, gpu_block_ids=gpu_ids, job=jid)
        return LoadHandle(job_id=jid, keys=keys, gpu_block_ids=gpu_ids, t_submit=t0)

    def complete_load(self, handle: LoadHandle) -> None:
        self.engine.wait_job(handle.job_id)
        t1 = time.perf_counter()
        if self.virtual_capacity > 0:
            for k in handle.keys:
                if k in self._present and self._present[k] > 0:
                    self._present[k] -= 1
        dt_ms = (t1 - handle.t_submit) * 1000
        self.metrics.loads += 1
        self.metrics.load_blocks += len(handle.keys)
        self.metrics.load_latency_ms.append(dt_ms)
        self.metrics.pin_depth_cur -= len(handle.keys)
        self._hlog("load_complete", t1, job=handle.job_id,
                   n_keys=len(handle.keys), ms=round(dt_ms, 3))
        self._log("complete_load", keys=handle.keys, job=handle.job_id,
                  ms=round(dt_ms, 3))

    def _evict_lru(self, n: int, protected: set[int]) -> list[int]:
        """Evict up to n LRU keys with ref_cnt==0, skipping `protected`."""
        evicted: list[int] = []
        for k in list(self._present.keys()):
            if len(evicted) >= n:
                break
            if k in protected:
                continue
            if self._present[k] > 0:  # pinned (in-flight load)
                continue
            del self._present[k]
            evicted.append(k)
        return evicted

    def prepare_store(self, keys: list[int]) -> StoreHandle | None:
        """Allocate slabs for `keys`, evicting if needed.

        With virtual_capacity_blocks > 0, Python enforces capacity and
        populates `evicted`. Otherwise we pass through to engine.prepare_store
        (which today never evicts).

        Returns None if the store cannot be fulfilled.
        """
        if not keys:
            return None
        self.metrics.stores_attempted += 1

        evicted: list[int] = []

        if self.virtual_capacity > 0:
            # Filter out already-present keys
            to_store = [k for k in keys if k not in self._present]
            already = len(keys) - len(to_store)
            self.metrics.store_blocks_already_present += already

            if not to_store:
                # All keys already present — refresh LRU and short-circuit
                for k in keys:
                    self._present.move_to_end(k)
                self._log("prepare_store", keys=keys,
                          accepted_keys=[], evicted_keys=[],
                          already_present=already)
                return StoreHandle(job_id=0, keys=[], evicted=[],
                                   gpu_block_ids=[], t_submit=time.perf_counter())

            needed = (len(self._present) + len(to_store)) - self.virtual_capacity
            if needed > 0:
                protected = set(keys)
                evicted = self._evict_lru(needed, protected)
                if len(evicted) < needed:
                    # Not enough unpinned victims — reject
                    self.metrics.stores_rejected += 1
                    self._log("prepare_store", keys=keys,
                              accepted_keys=[], evicted_keys=evicted,
                              already_present=already, rejected=True)
                    return None

            # Reserve slots (ref_cnt=1 until complete_store)
            for k in to_store:
                self._present[k] = 1

            # Also tell native engine (so batch_check still agrees, though we
            # don't consult it under virtual-capacity mode)
            native_to_store, _ = self.engine.prepare_store(to_store)
            native_to_store = list(native_to_store)
        else:
            native_to_store, evicted_native = self.engine.prepare_store(keys)
            native_to_store = list(native_to_store)
            evicted = list(evicted_native)
            self.metrics.store_blocks_already_present += len(keys) - len(native_to_store) - len(evicted)
            to_store = native_to_store

        if evicted:
            self.metrics.evicted_keys_total += len(evicted)
            self.metrics.evicted_key_stream.extend(
                (self._now_s, k) for k in evicted
            )

        if not native_to_store:
            return StoreHandle(job_id=0, keys=[], evicted=evicted,
                               gpu_block_ids=[], t_submit=time.perf_counter())

        gpu_ids = self.pool.take(len(native_to_store))
        jid = self._jid()
        t0 = time.perf_counter()
        self._hlog("store_submit", t0, job=jid, keys=native_to_store,
                   gpu_block_ids=gpu_ids,
                   bytes=len(native_to_store) * NATIVE_BLOCK_BYTES)
        try:
            self.engine.store_async(jid, gpu_ids, native_to_store)
        except Exception:
            self.metrics.stores_rejected += 1
            self._log("prepare_store", keys=keys, accepted_keys=[],
                      evicted_keys=evicted, rejected=True)
            return None
        self.metrics.store_blocks_accepted += len(native_to_store)
        self._log("prepare_store", keys=keys,
                  accepted_keys=native_to_store,
                  evicted_keys=evicted,
                  gpu_block_ids=gpu_ids, job=jid)
        return StoreHandle(job_id=jid, keys=native_to_store, evicted=evicted,
                           gpu_block_ids=gpu_ids, t_submit=t0)

    def complete_store(self, handle: StoreHandle, success: bool = True) -> None:
        dt_ms = 0.0
        t1 = time.perf_counter()
        if handle.job_id != 0:
            self.engine.wait_job(handle.job_id)
            t1 = time.perf_counter()
            dt_ms = (t1 - handle.t_submit) * 1000
            self.metrics.store_latency_ms.append(dt_ms)
            self._hlog("store_complete", t1, job=handle.job_id,
                       n_keys=len(handle.keys), ms=round(dt_ms, 3))
        if handle.keys:
            self.engine.complete_store(handle.keys, success)
            if self.virtual_capacity > 0:
                for k in handle.keys:
                    if k in self._present:
                        # Drop the reservation ref, keep LRU position
                        self._present[k] = 0
        self._log("complete_store", keys=handle.keys, job=handle.job_id,
                  ms=round(dt_ms, 3), success=success)
