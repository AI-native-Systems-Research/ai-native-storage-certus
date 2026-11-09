# SPDX-License-Identifier: Apache-2.0
"""
tracing_bench_connector.py

A KVConnector that wraps DecodeBenchConnector and records every method call
to a JSONL trace file. Works in both the scheduler and worker processes.

Each process writes to its own file:
    kvconn_trace_<pid>.jsonl

The analyze_trace.py script will merge them automatically.

Register via kv_transfer_config:
    {
        "kv_connector": "TracingDecodeBenchConnector",
        "kv_connector_module_path": "tracing_bench_connector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {"fill_mean": 0.015, "fill_std": 0.0}
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
from vllm.distributed.kv_transfer.kv_connector.v1.decode_bench_connector import (
    DecodeBenchConnector,
)
from vllm.logger import init_logger
from vllm.v1.attention.backend import AttentionMetadata

if TYPE_CHECKING:
    from vllm.config import VllmConfig
    from vllm.distributed.kv_events import KVConnectorKVEvents
    from vllm.distributed.kv_transfer.kv_connector.v1.base import (
        KVConnectorHandshakeMetadata,
    )
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
    return TRACE_DIR / f"kvconn_trace_{os.getpid()}.jsonl"


_fh = None


def _write(record: dict):
    global _fh
    if _fh is None:
        _fh = open(_trace_file(), "a", buffering=1)  # line-buffered
    _fh.write(json.dumps(record) + "\n")


def _safe_repr(obj, maxlen: int = 8000) -> str:
    try:
        s = repr(obj)
    except Exception:
        s = "<repr-error>"
    return (s[:maxlen] + "…") if len(s) > maxlen else s


def _trace(method_name: str):
    """Decorator: records call metadata and delegates to the real method."""

    def decorator(fn):
        @functools.wraps(fn)
        def wrapper(self, *args, **kwargs):
            try:
                role_str = self.role.name
            except Exception:
                role_str = "unknown"

            arg_reprs = [_safe_repr(a) for a in args]
            kwarg_reprs = {k: _safe_repr(v) for k, v in kwargs.items()}

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
                _write({
                    "pid": os.getpid(),
                    "ts": t0,
                    "elapsed": round(elapsed, 9),
                    "role": role_str,
                    "connector": "TracingDecodeBenchConnector",
                    "method": method_name,
                    "args": arg_reprs,
                    "kwargs": kwarg_reprs,
                    "result": _safe_repr(result) if error is None else None,
                    "error": error,
                })

        return wrapper

    return decorator


class TracingDecodeBenchConnector(KVConnectorBase_V1):
    """
    Wraps DecodeBenchConnector with per-call JSONL tracing.
    All KV operations are delegated to the inner connector.
    """

    def __init__(
        self,
        vllm_config: "VllmConfig",
        role: KVConnectorRole,
        kv_cache_config: "KVCacheConfig | None" = None,
    ):
        super().__init__(vllm_config, role, kv_cache_config)
        self._inner = DecodeBenchConnector(vllm_config, role, kv_cache_config)
        logger.info(
            "TracingDecodeBenchConnector initialized (pid=%d, role=%s) → %s",
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
        # Keep parent state in sync so has_connector_metadata() works
        self._connector_metadata = self._inner._connector_metadata

    @_trace("clear_connector_metadata")
    def clear_connector_metadata(self) -> None:
        self._inner.clear_connector_metadata()
        self._connector_metadata = None

    @_trace("start_load_kv")
    def start_load_kv(self, forward_context: "ForwardContext", **kwargs: Any) -> None:
        # forward_context holds our metadata; inner connector reads from itself
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
        return self._inner.get_finished(finished_req_ids)

    @_trace("build_connector_worker_meta")
    def build_connector_worker_meta(self) -> KVConnectorWorkerMetadata | None:
        return self._inner.build_connector_worker_meta()

    @_trace("get_kv_connector_stats")
    def get_kv_connector_stats(self) -> "KVConnectorStats | None":
        return self._inner.get_kv_connector_stats()

    @_trace("get_kv_connector_kv_cache_events")
    def get_kv_connector_kv_cache_events(self) -> "KVConnectorKVEvents | None":
        return self._inner.get_kv_connector_kv_cache_events()

    @_trace("shutdown")
    def shutdown(self):
        return self._inner.shutdown()

    # ── Scheduler-side ────────────────────────────────────────────────────────

    @_trace("get_num_new_matched_tokens")
    def get_num_new_matched_tokens(
        self,
        request: "Request",
        num_computed_tokens: int,
    ) -> tuple[int | None, bool]:
        return self._inner.get_num_new_matched_tokens(request, num_computed_tokens)

    @_trace("update_state_after_alloc")
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
        return self._inner.update_connector_output(connector_output)

    @_trace("request_finished")
    def request_finished(
        self,
        request: "Request",
        block_ids: list[int],
    ) -> tuple[bool, dict[str, Any] | None]:
        return self._inner.request_finished(request, block_ids)

    @_trace("take_events")
    def take_events(self):
        return self._inner.take_events()

    @classmethod
    def get_required_kvcache_layout(cls, vllm_config: "VllmConfig") -> str | None:
        return DecodeBenchConnector.get_required_kvcache_layout(vllm_config)

    @classmethod
    def requires_piecewise_for_cudagraph(cls, extra_config: dict[str, Any]) -> bool:
        return DecodeBenchConnector.requires_piecewise_for_cudagraph(extra_config)
