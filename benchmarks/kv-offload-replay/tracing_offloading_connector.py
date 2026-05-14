# SPDX-License-Identifier: Apache-2.0
"""
tracing_offloading_connector.py

Wraps OffloadingConnector with per-call JSONL tracing.
Each process writes to its own file: offloading_trace_<pid>.jsonl

Register via kv_transfer_config:
    {
        "kv_connector": "TracingOffloadingConnector",
        "kv_connector_module_path": "tracing_offloading_connector",
        "kv_role": "kv_both"
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
    SupportsHMA,
)
from vllm.distributed.kv_transfer.kv_connector.v1.base import (
    KVConnectorMetadata,
    KVConnectorWorkerMetadata,
)
from vllm.distributed.kv_transfer.kv_connector.v1.offloading_connector import (
    OffloadingConnector,
)
from vllm.logger import init_logger
from vllm.v1.attention.backend import AttentionMetadata

if TYPE_CHECKING:
    from vllm.config import VllmConfig
    from vllm.distributed.kv_events import KVCacheEvent, KVConnectorKVEvents
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
    return TRACE_DIR / f"offloading_trace_{os.getpid()}.jsonl"


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


def _trace(method_name: str):
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
                    "connector": "TracingOffloadingConnector",
                    "method": method_name,
                    "args": arg_reprs,
                    "kwargs": kwarg_reprs,
                    "result": _safe_repr(result) if error is None else None,
                    "error": error,
                })

        return wrapper

    return decorator


class TracingOffloadingConnector(KVConnectorBase_V1, SupportsHMA):
    """Wraps OffloadingConnector with per-call JSONL tracing."""

    @property
    def prefer_cross_layer_blocks(self) -> bool:
        return self._inner.prefer_cross_layer_blocks

    def __init__(
        self,
        vllm_config: "VllmConfig",
        role: KVConnectorRole,
        kv_cache_config: "KVCacheConfig | None" = None,
    ):
        super().__init__(vllm_config, role, kv_cache_config)
        self._inner = OffloadingConnector(vllm_config, role, kv_cache_config)
        logger.info(
            "TracingOffloadingConnector initialized (pid=%d, role=%s) → %s",
            os.getpid(),
            role.name,
            _trace_file(),
        )

    # ── Worker-side ───────────────────────────────────────────────────────────

    @_trace("register_kv_caches")
    def register_kv_caches(self, kv_caches: dict[str, torch.Tensor]):
        return self._inner.register_kv_caches(kv_caches)

    @_trace("register_cross_layers_kv_cache")
    def register_cross_layers_kv_cache(
        self, kv_cache: torch.Tensor, attn_backend: Any
    ):
        return self._inner.register_cross_layers_kv_cache(kv_cache, attn_backend)

    @_trace("bind_connector_metadata")
    def bind_connector_metadata(self, connector_metadata: KVConnectorMetadata) -> None:
        self._inner.bind_connector_metadata(connector_metadata)
        self._connector_metadata = self._inner._connector_metadata

    @_trace("clear_connector_metadata")
    def clear_connector_metadata(self) -> None:
        self._inner.clear_connector_metadata()
        self._connector_metadata = None

    @_trace("handle_preemptions")
    def handle_preemptions(self, kv_connector_metadata: KVConnectorMetadata):
        return self._inner.handle_preemptions(kv_connector_metadata)

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
        self._inner._connector_metadata = self._connector_metadata
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

    @_trace("request_finished_all_groups")
    def request_finished_all_groups(
        self,
        request: "Request",
        block_ids: tuple[list[int], ...],
    ) -> tuple[bool, dict[str, Any] | None]:
        if hasattr(self._inner, "request_finished_all_groups"):
            return self._inner.request_finished_all_groups(request, block_ids)
        # Fall back to single-group form if the installed vLLM doesn't
        # expose the all-groups variant
        merged: list[int] = []
        for group in block_ids:
            merged.extend(group)
        return self._inner.request_finished(request, merged)

    @_trace("take_events")
    def take_events(self):
        return self._inner.take_events()

    @classmethod
    def build_kv_connector_stats(
        cls, data: dict[str, Any] | None = None
    ) -> "KVConnectorStats | None":
        return OffloadingConnector.build_kv_connector_stats(data)

    @classmethod
    def build_prom_metrics(cls, vllm_config: "VllmConfig", *args, **kwargs):
        return OffloadingConnector.build_prom_metrics(vllm_config, *args, **kwargs)
