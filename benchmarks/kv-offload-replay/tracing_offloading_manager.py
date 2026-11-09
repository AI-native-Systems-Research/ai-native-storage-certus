# SPDX-License-Identifier: Apache-2.0
"""
tracing_offloading_manager.py

Instruments the vLLM OffloadingManager layer with JSONL tracing.
Every call to lookup / touch / prepare_load / complete_load / prepare_store /
complete_store / take_events is logged. Works in conjunction with the
OffloadingConnector (or TracingConnector wrapping it).

The module exports `TracingCPUOffloadingSpec` — a drop-in replacement for
CPUOffloadingSpec that wraps the returned manager. Select via kv_transfer_config:

    {
        "kv_connector": "TracingConnector",
        "kv_connector_module_path": "tracing_connector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            "traced_kv_connector": "OffloadingConnector",
            "cpu_bytes_to_use": 4294967296,
            "spec_name": "TracingCPUOffloadingSpec",
            "spec_module_path": "tracing_offloading_manager"
        }
    }

Each process writes to its own file: offloading_mgr_<pid>.jsonl
"""

import functools
import json
import os
import time
from collections.abc import Iterable
from pathlib import Path
from typing import TYPE_CHECKING

from vllm.v1.kv_offload.abstract import (
    LoadStoreSpec,
    OffloadingEvent,
    OffloadingManager,
    PrepareStoreOutput,
)
from vllm.v1.kv_offload.cpu.spec import CPUOffloadingSpec
from vllm.v1.core.kv_cache_utils import BlockHash
from vllm.v1.kv_offload.worker.worker import (
    OffloadingHandler,
    TransferResult,
    TransferSpec,
)

if TYPE_CHECKING:
    pass


TRACE_DIR = Path(__file__).parent


def _trace_file() -> Path:
    return TRACE_DIR / f"offloading_mgr_{os.getpid()}.jsonl"


def _handler_trace_file() -> Path:
    return TRACE_DIR / f"offloading_handler_{os.getpid()}.jsonl"


_fh = None
_hfh = None
_t0: float | None = None  # first perf_counter tick; subtracted from every ts


def _now() -> float:
    """Seconds since the first call (per process)."""
    global _t0
    t = time.perf_counter()
    if _t0 is None:
        _t0 = t
    return t - _t0


def _write(record: dict) -> None:
    global _fh
    if _fh is None:
        _fh = open(_trace_file(), "a", buffering=1)
    _fh.write(json.dumps(record) + "\n")


def _hwrite(record: dict) -> None:
    global _hfh
    if _hfh is None:
        _hfh = open(_handler_trace_file(), "a", buffering=1)
    _hfh.write(json.dumps(record) + "\n")


def _spec_summary(s: LoadStoreSpec) -> dict:
    """Capture the replayable shape of a LoadStoreSpec."""
    try:
        medium = s.medium()
    except Exception:
        medium = "?"
    out: dict = {"medium": medium}
    # BlockIDsLoadStoreSpec (GPU/CPU) — has .block_ids (numpy array)
    if hasattr(s, "block_ids"):
        out["block_ids"] = [int(b) for b in s.block_ids]
    # GPULoadStoreSpec additions
    if hasattr(s, "group_sizes") and s.group_sizes is not None:
        out["group_sizes"] = [int(g) for g in s.group_sizes]
    if hasattr(s, "block_indices") and s.block_indices is not None:
        out["block_indices"] = [int(i) for i in s.block_indices]
    return out


def _hash_to_str(h) -> str:
    """BlockHash is a bytes NewType; render as hex for compact JSON."""
    try:
        if isinstance(h, (bytes, bytearray)):
            return h.hex()
        return repr(h)
    except Exception:
        return "<hash-err>"


def _keys_summary(keys) -> list[str]:
    try:
        return [_hash_to_str(h) for h in keys]
    except Exception:
        return ["<keys-err>"]


def _summarize_store_output(out) -> dict:
    if out is None:
        return {"rejected": True}
    try:
        return {
            "block_hashes_to_store": [_hash_to_str(h) for h in out.block_hashes_to_store],
            "block_hashes_evicted": [_hash_to_str(h) for h in out.block_hashes_evicted],
            "store_spec_medium": out.store_spec.medium() if out.store_spec else None,
        }
    except Exception:
        return {"repr": repr(out)[:400]}


def _summarize_events(events) -> list[dict]:
    out = []
    try:
        for e in events:
            out.append({
                "block_hashes": [_hash_to_str(h) for h in e.block_hashes],
                "block_size": e.block_size,
                "medium": e.medium,
                "removed": e.removed,
            })
    except Exception:
        out = [{"repr": repr(events)[:400]}]
    return out


class TracingOffloadingManager(OffloadingManager):
    """Wraps any OffloadingManager with JSONL tracing of all 6+ methods."""

    def __init__(self, inner: OffloadingManager):
        self._inner = inner

    # ── Primary API ────────────────────────────────────────────────────────

    def lookup(self, *args, **kwargs):
        # vLLM 0.19: lookup(keys: Iterable) -> int | None
        # vLLM 0.20: lookup(key: OffloadKey, req_context: ReqContext) -> bool
        t0 = _now()
        if args and isinstance(args[0], (bytes, bytearray, memoryview)):
            # 0.20: single-key + req_context
            keys_for_log = [args[0]]
        else:
            # 0.19: iterable of keys
            keys_for_log = list(args[0]) if args else []
            args = (keys_for_log, *args[1:])
        try:
            return self._inner.lookup(*args, **kwargs)
        finally:
            _write({
                "ts": t0,
                "method": "lookup",
                "keys": _keys_summary(keys_for_log),
            })

    def prepare_load(self, *args, **kwargs):
        # 0.19: (keys);  0.20: (keys, req_context)
        keys_for_log = list(args[0]) if args else list(kwargs.get("keys") or [])
        if args:
            args = (keys_for_log, *args[1:])
        t0 = _now()
        try:
            return self._inner.prepare_load(*args, **kwargs)
        finally:
            _write({
                "ts": t0,
                "method": "prepare_load",
                "keys": _keys_summary(keys_for_log),
            })

    def touch(self, block_hashes: Iterable[BlockHash]) -> None:
        block_hashes = list(block_hashes)
        t0 = _now()
        try:
            self._inner.touch(block_hashes)
        finally:
            _write({
                "ts": t0,
                "method": "touch",
                "keys": _keys_summary(block_hashes),
            })

    def complete_load(self, block_hashes: Iterable[BlockHash]) -> None:
        block_hashes = list(block_hashes)
        t0 = _now()
        try:
            self._inner.complete_load(block_hashes)
        finally:
            _write({
                "ts": t0,
                "method": "complete_load",
                "keys": _keys_summary(block_hashes),
            })

    def prepare_store(self, *args, **kwargs):
        # 0.19: (keys);  0.20: (keys, req_context)
        keys_for_log = list(args[0]) if args else list(kwargs.get("keys") or [])
        if args:
            args = (keys_for_log, *args[1:])
        t0 = _now()
        try:
            return self._inner.prepare_store(*args, **kwargs)
        finally:
            _write({"ts": t0, "method": "prepare_store",
                    "keys": _keys_summary(keys_for_log)})

    def complete_store(
        self, block_hashes: Iterable[BlockHash], success: bool = True
    ) -> None:
        block_hashes = list(block_hashes)
        t0 = _now()
        try:
            self._inner.complete_store(block_hashes, success)
        finally:
            _write({
                "ts": t0,
                "method": "complete_store",
                "keys": _keys_summary(block_hashes),
                "success": success,
            })

    def take_events(self) -> Iterable[OffloadingEvent]:
        # take_events is purely a policy *output* channel — nothing here is
        # replay-input. Don't log it.
        yield from self._inner.take_events()


class TracingOffloadingHandler(OffloadingHandler):
    """Wraps an OffloadingHandler to log the worker-side data-movement API.

    Captures:
      transfer_async(job_id, (src, dst))  — submit of a block copy
      wait(job_ids)                       — blocking wait for a set of jobs
      get_finished() -> [TransferResult]  — async completion poll

    Input-only where possible: transfer_async logs (job_id, src, dst). The
    return value (`success`) is recorded because it's part of the contract —
    a failed submit isn't a policy decision. wait/get_finished log inputs only.
    """

    def __init__(self, inner: OffloadingHandler, transfer_type: str):
        self._inner = inner
        self._type = transfer_type  # e.g. "GPU→CPU" (for readability)

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src, dst = spec
        t0 = _now()
        try:
            return self._inner.transfer_async(job_id, spec)
        finally:
            _hwrite({
                "ts": t0,
                "method": "transfer_async",
                "transfer_type": self._type,
                "job_id": int(job_id),
                "src": _spec_summary(src),
                "dst": _spec_summary(dst),
            })

    def get_finished(self) -> list[TransferResult]:
        t0 = _now()
        results = self._inner.get_finished()
        if results:
            _hwrite({
                "ts": t0,
                "method": "get_finished",
                "transfer_type": self._type,
                "job_ids": [int(r.job_id) for r in results],
            })
        return results

    def wait(self, job_ids: set[int]) -> None:
        t0 = _now()
        try:
            self._inner.wait(job_ids)
        finally:
            _hwrite({
                "ts": t0,
                "method": "wait",
                "transfer_type": self._type,
                "job_ids": sorted(int(j) for j in job_ids),
            })


class TracingCPUOffloadingSpec(CPUOffloadingSpec):
    """CPUOffloadingSpec that returns trace-wrapped manager + handlers."""

    def get_manager(self) -> OffloadingManager:
        inner = super().get_manager()
        if isinstance(inner, TracingOffloadingManager):
            return inner
        wrapped = TracingOffloadingManager(inner)
        self._manager = wrapped
        return wrapped

    def get_handlers(self, kv_caches):
        for src_cls, dst_cls, handler in super().get_handlers(kv_caches):
            ttype = f"{src_cls.medium()}->{dst_cls.medium()}"
            yield src_cls, dst_cls, TracingOffloadingHandler(handler, ttype)
