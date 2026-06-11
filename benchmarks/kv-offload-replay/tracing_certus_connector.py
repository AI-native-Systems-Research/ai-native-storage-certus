# SPDX-License-Identifier: Apache-2.0
"""
tracing_certus_connector.py

A KVConnector that wraps CertusConnector and records every method call to a
JSONL trace file. Works in both the scheduler and worker processes.

Each process writes to its own file:
    kvconn_trace_certus_<pid>.jsonl

Register via kv_transfer_config:
    {
        "kv_connector": "TracingCertusConnector",
        "kv_connector_module_path": "tracing_certus_connector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            "socket_path": "/tmp/certus.sock",
            "fill_mean": 0.015,
            "fill_std": 0.0
        }
    }
"""

import functools
import json
import os
import time
from pathlib import Path
from typing import TYPE_CHECKING, Any

import torch

from vllm.distributed.kv_transfer.kv_connector.v1 import (
    KVConnectorBase_V1,
    KVConnectorRole,
)
from vllm.distributed.kv_transfer.kv_connector.v1.base import (
    KVConnectorMetadata,
    KVConnectorWorkerMetadata,
)
from vllm.logger import init_logger
from vllm.v1.attention.backend import AttentionMetadata

from certus_connector import CertusConnector

if TYPE_CHECKING:
    from vllm.config import VllmConfig
    from vllm.distributed.kv_events import KVConnectorKVEvents
    from vllm.distributed.kv_transfer.kv_connector.v1.metrics import KVConnectorStats
    from vllm.forward_context import ForwardContext
    from vllm.v1.core.kv_cache_manager import KVCacheBlocks
    from vllm.v1.core.sched.output import SchedulerOutput
    from vllm.v1.kv_cache_interface import KVCacheConfig
    from vllm.v1.outputs import KVConnectorOutput
    from vllm.v1.request import Request

logger = init_logger(__name__)

TRACE_DIR = Path(__file__).parent


def _trace_file() -> Path:
    return TRACE_DIR / f"kvconn_trace_certus_{os.getpid()}.jsonl"


_fh = None


def _write(record: dict):
    global _fh
    if _fh is None:
        _fh = open(_trace_file(), "a", buffering=1)
    _fh.write(json.dumps(record) + "\n")


def _safe_repr(obj, maxlen: int = 8000) -> str:
    try:
        s = repr(obj)
    except Exception:
        s = "<repr-error>"
    return (s[:maxlen] + "…") if len(s) > maxlen else s


def _request_summary(request) -> dict:
    """Extract the identifying fields we care about from a Request."""
    try:
        return {
            "request_id": getattr(request, "request_id", None),
            "num_tokens": getattr(request, "num_tokens", None),
            "num_prompt_tokens": len(getattr(request, "prompt_token_ids", []) or []),
        }
    except Exception:
        return {"repr": _safe_repr(request, 400)}


def _blocks_summary(blocks) -> dict:
    """Extract block_ids from a KVCacheBlocks without dumping tensors."""
    try:
        ids = blocks.get_block_ids()
        return {
            "groups": len(ids),
            "block_ids_per_group": [list(g) for g in ids],
            "total_blocks": sum(len(g) for g in ids),
        }
    except Exception:
        return {"repr": _safe_repr(blocks, 200)}


def _trace(method_name: str, summarize_args=None):
    """Decorator: records call metadata and delegates to the real method.

    summarize_args: optional list of (name, summarizer_fn) pairs applied
    positionally to args so we get structured output (keys, block_ids)
    rather than opaque reprs.
    """

    def decorator(fn):
        @functools.wraps(fn)
        def wrapper(self, *args, **kwargs):
            try:
                role_str = self.role.name
            except Exception:
                role_str = "unknown"

            summarized: dict[str, Any] = {}
            raw_args: list[str] = []
            if summarize_args:
                for i, (name, summ) in enumerate(summarize_args):
                    if i < len(args):
                        try:
                            summarized[name] = summ(args[i])
                        except Exception:
                            summarized[name] = _safe_repr(args[i], 200)
                raw_args = [_safe_repr(a, 200)
                            for a in args[len(summarize_args):]]
            else:
                raw_args = [_safe_repr(a, 200) for a in args]

            kwarg_reprs = {k: _safe_repr(v, 200) for k, v in kwargs.items()}

            t0 = time.perf_counter()
            error = None
            result = None
            try:
                result = fn(self, *args, **kwargs)
                return result
            except Exception as exc:
                error = f"{type(exc).__name__}: {exc}"
                raise
            finally:
                elapsed = time.perf_counter() - t0
                rec: dict[str, Any] = {
                    "pid": os.getpid(),
                    "ts": t0,
                    "elapsed": round(elapsed, 9),
                    "role": role_str,
                    "connector": "TracingCertusConnector",
                    "method": method_name,
                    "error": error,
                }
                if summarized:
                    rec["summary"] = summarized
                if raw_args:
                    rec["args"] = raw_args
                if kwarg_reprs:
                    rec["kwargs"] = kwarg_reprs
                if error is None:
                    rec["result"] = _safe_repr(result, 400)
                _write(rec)

        return wrapper

    return decorator


class TracingCertusConnector(KVConnectorBase_V1):
    """Wraps CertusConnector with per-call JSONL tracing."""

    def __init__(
        self,
        vllm_config: "VllmConfig",
        role: KVConnectorRole,
        kv_cache_config: "KVCacheConfig | None" = None,
    ):
        super().__init__(vllm_config, role, kv_cache_config)
        self._inner = CertusConnector(vllm_config, role, kv_cache_config)
        logger.info(
            "TracingCertusConnector initialized (pid=%d, role=%s) → %s",
            os.getpid(),
            role.name,
            _trace_file(),
        )

    # ── Worker-side ───────────────────────────────────────────────────────────

    @_trace("register_kv_caches")
    def register_kv_caches(self, kv_caches: dict[str, torch.Tensor]):
        return self._inner.register_kv_caches(kv_caches)

    @_trace("bind_connector_metadata")
    def bind_connector_metadata(self, connector_metadata: KVConnectorMetadata) -> None:
        self._inner.bind_connector_metadata(connector_metadata)
        self._connector_metadata = self._inner._connector_metadata

    @_trace("clear_connector_metadata")
    def clear_connector_metadata(self) -> None:
        self._inner.clear_connector_metadata()
        self._connector_metadata = None

    @_trace("start_load_kv")
    def start_load_kv(self, forward_context: "ForwardContext", **kwargs: Any) -> None:
        self._inner._connector_metadata = self._connector_metadata
        return self._inner.start_load_kv(forward_context, **kwargs)

    @_trace("wait_for_layer_load")
    def wait_for_layer_load(self, layer_name: str) -> None:
        return self._inner.wait_for_layer_load(layer_name)

    @_trace("save_kv_layer")
    def save_kv_layer(
        self,
        layer_name: str,
        kv_layer: torch.Tensor,
        attn_metadata: "AttentionMetadata",
        **kwargs: Any,
    ) -> None:
        return self._inner.save_kv_layer(layer_name, kv_layer, attn_metadata, **kwargs)

    @_trace("wait_for_save")
    def wait_for_save(self):
        return self._inner.wait_for_save()

    @_trace("get_finished")
    def get_finished(
        self, finished_req_ids: set[str]
    ) -> tuple[set[str] | None, set[str] | None]:
        if hasattr(self._inner, "get_finished"):
            return self._inner.get_finished(finished_req_ids)
        return None, None

    @_trace("shutdown")
    def shutdown(self):
        return self._inner.shutdown()

    # ── Scheduler-side ────────────────────────────────────────────────────────

    @_trace("get_num_new_matched_tokens",
            summarize_args=[("request", _request_summary)])
    def get_num_new_matched_tokens(
        self,
        request: "Request",
        num_computed_tokens: int,
    ) -> tuple[int | None, bool]:
        return self._inner.get_num_new_matched_tokens(request, num_computed_tokens)

    @_trace("update_state_after_alloc",
            summarize_args=[("request", _request_summary),
                            ("blocks", _blocks_summary)])
    def update_state_after_alloc(
        self,
        request: "Request",
        blocks: "KVCacheBlocks",
        num_external_tokens: int,
    ):
        return self._inner.update_state_after_alloc(request, blocks, num_external_tokens)

    @_trace("build_connector_meta")
    def build_connector_meta(
        self, scheduler_output: "SchedulerOutput"
    ) -> KVConnectorMetadata:
        return self._inner.build_connector_meta(scheduler_output)

    @_trace("update_connector_output")
    def update_connector_output(self, connector_output: "KVConnectorOutput"):
        if hasattr(self._inner, "update_connector_output"):
            return self._inner.update_connector_output(connector_output)
        return None

    @_trace("request_finished",
            summarize_args=[("request", _request_summary)])
    def request_finished(
        self,
        request: "Request",
        block_ids: list[int],
    ) -> tuple[bool, dict[str, Any] | None]:
        return self._inner.request_finished(request, block_ids)

    @_trace("take_events")
    def take_events(self):
        if hasattr(self._inner, "take_events"):
            return self._inner.take_events()
        return []

    @classmethod
    def get_required_kvcache_layout(cls, vllm_config: "VllmConfig") -> str | None:
        if hasattr(CertusConnector, "get_required_kvcache_layout"):
            return CertusConnector.get_required_kvcache_layout(vllm_config)
        return None
