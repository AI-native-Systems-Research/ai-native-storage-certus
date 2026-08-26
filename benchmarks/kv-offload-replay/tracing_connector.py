# SPDX-License-Identifier: Apache-2.0
"""
tracing_connector.py

A **connector-agnostic** KVConnector that sits ABOVE whatever connector you
configure and records every method call to a JSONL trace file. It wraps an inner
connector chosen at runtime from config — it is not tied to any one connector and
knows nothing about certus, offloading internals, etc.

This supersedes the two former connector-specific tracers
(``tracing_offloading_connector.py`` wrapping ``OffloadingConnector`` and
``tracing_certus_connector.py`` wrapping the gRPC ``CertusConnector``): both are
folded into this one, with the structured block/request summaries kept.

Each process writes to its own file: ``offloading_trace_<pid>.jsonl`` (the
filename the downstream consumers — ``replay_offloading_traces.py`` and the
drivers' trace-reaping — already expect). Set ``TRACE_DIR`` to redirect the
output directory (default: this module's directory), e.g. to a mounted path so a
``--rm`` container can export the traces.

Register via ``kv_transfer_config``. The inner connector is selected by two extra
keys; everything else in ``kv_connector_extra_config`` is passed through to the
inner connector unchanged (it reads them off ``vllm_config``):

    {
        "kv_connector": "TracingConnector",
        "kv_connector_module_path": "tracing_connector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            # which connector to wrap (default: OffloadingConnector):
            "traced_kv_connector": "OffloadingConnector",
            # optional — import the inner from a module instead of vLLM's registry:
            # "traced_kv_connector_module_path": "certus_connector",
            # ...the inner connector's own knobs ride here too, untouched:
            "spec_name": "CertusShmqOffloadingSpec",
            "spec_module_path": "certus_shmq_connector.spec",
            "shm_path": "/dev/shm/certus-shmq",
        },
    }

Because both the CPU-offload path and the shmq/certus path run vLLM's
``OffloadingConnector`` (differing only in the *spec* named above), the default
inner traces both with no extra configuration.
"""

import functools
import importlib
import json
import os
import threading
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
from vllm.logger import init_logger
from vllm.v1.attention.backend import AttentionMetadata

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


def _trace_dir() -> Path:
    return Path(os.environ.get("TRACE_DIR") or Path(__file__).parent)


def _trace_file() -> Path:
    return _trace_dir() / f"offloading_trace_{os.getpid()}.jsonl"


_fh = None
_fh_lock = threading.Lock()


def _write(record: dict):
    """Append one JSON record. Thread-safe: WORKLOAD_MODE=async issues transfers
    (and thus traced calls) from multiple threads, so serialize the writes."""
    global _fh
    with _fh_lock:
        if _fh is None:
            _trace_dir().mkdir(parents=True, exist_ok=True)
            _fh = open(_trace_file(), "a", buffering=1)
        _fh.write(json.dumps(record) + "\n")


def _safe_repr(obj, maxlen: int = 8000) -> str:
    try:
        s = repr(obj)
    except Exception:
        s = "<repr-error>"
    return (s[:maxlen] + "…") if len(s) > maxlen else s


def _request_summary(request) -> dict:
    """Identifying fields from a Request, without dumping tensors."""
    try:
        return {
            "request_id": getattr(request, "request_id", None),
            "num_tokens": getattr(request, "num_tokens", None),
            "num_prompt_tokens": len(getattr(request, "prompt_token_ids", []) or []),
        }
    except Exception:
        return {"repr": _safe_repr(request, 400)}


def _blocks_summary(blocks) -> dict:
    """Block-id counts from a KVCacheBlocks. This is the per-request block
    accounting the trace exists to capture."""
    try:
        ids = blocks.get_block_ids()
        return {
            "groups": len(ids),
            "block_ids_per_group": [list(g) for g in ids],
            "total_blocks": sum(len(g) for g in ids),
        }
    except Exception:
        return {"repr": _safe_repr(blocks, 200)}


def _block_ids_summary(block_ids) -> dict:
    """Summary for a plain (possibly grouped) block-id argument, e.g. the
    ``block_ids`` passed to request_finished / request_finished_all_groups."""
    try:
        # tuple/list of groups?
        if block_ids and isinstance(block_ids[0], (list, tuple)):
            return {
                "groups": len(block_ids),
                "total_blocks": sum(len(g) for g in block_ids),
            }
        return {"groups": 1, "total_blocks": len(block_ids)}
    except Exception:
        return {"repr": _safe_repr(block_ids, 200)}


def _trace(method_name: str, summarize_args=None):
    """Decorator: record call metadata + delegate to the wrapped method.

    ``summarize_args``: optional list of (name, summarizer_fn) applied positionally
    to args so structured fields (keys, block_ids) land instead of opaque reprs.
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
                raw_args = [_safe_repr(a, 200) for a in args[len(summarize_args):]]
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
                    "connector": "TracingConnector",
                    "inner": self._inner_name,
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


def _resolve_inner_class(extra_config: dict | None):
    """Resolve the inner connector class from config, mirroring how vLLM itself
    resolves a connector: an explicit module path is imported directly, otherwise
    the name is looked up in vLLM's built-in registry. Defaults to
    ``OffloadingConnector`` (what both the CPU-offload and shmq/certus paths use)."""
    extra_config = extra_config or {}
    name = extra_config.get("traced_kv_connector") or "OffloadingConnector"
    module_path = extra_config.get("traced_kv_connector_module_path")

    if module_path:
        return name, getattr(importlib.import_module(module_path), name)

    if name == "OffloadingConnector":
        from vllm.distributed.kv_transfer.kv_connector.v1.offloading_connector import (
            OffloadingConnector,
        )
        return name, OffloadingConnector

    # A built-in connector referenced by name: consult vLLM's factory registry.
    from vllm.distributed.kv_transfer.kv_connector.factory import KVConnectorFactory

    registry = getattr(KVConnectorFactory, "_registry", None)
    if isinstance(registry, dict) and name in registry:
        entry = registry[name]
        # Across vLLM versions the registry value is either a zero-arg loader
        # returning the class, or a (module_path, class_name) tuple.
        if callable(entry):
            return name, entry()
        if isinstance(entry, (tuple, list)) and len(entry) == 2:
            mod, cls = entry
            return name, getattr(importlib.import_module(mod), cls)

    raise ValueError(
        f"TracingConnector: cannot resolve inner connector {name!r}. Pass "
        f"'traced_kv_connector_module_path' in kv_connector_extra_config to import "
        f"it explicitly."
    )


class TracingConnector(KVConnectorBase_V1, SupportsHMA):
    """Wraps a config-selected inner connector with per-call JSONL tracing."""

    @property
    def prefer_cross_layer_blocks(self) -> bool:
        return getattr(self._inner, "prefer_cross_layer_blocks", False)

    def __init__(
        self,
        vllm_config: "VllmConfig",
        role: KVConnectorRole,
        kv_cache_config: "KVCacheConfig | None" = None,
    ):
        super().__init__(vllm_config, role, kv_cache_config)
        extra = {}
        try:
            extra = dict(vllm_config.kv_transfer_config.kv_connector_extra_config or {})
        except Exception:
            extra = {}
        self._inner_name, inner_cls = _resolve_inner_class(extra)
        self._inner = inner_cls(vllm_config, role, kv_cache_config)
        logger.info(
            "TracingConnector initialized (pid=%d, role=%s) wrapping %s → %s",
            os.getpid(),
            role.name,
            self._inner_name,
            _trace_file(),
        )

    # Anything not explicitly wrapped below is delegated verbatim to the inner
    # connector — keeps the wrapper tolerant of connector- and version-specific
    # methods without needing an edit here.
    def __getattr__(self, name: str):
        # __getattr__ only fires for misses; _inner itself is a real attribute.
        return getattr(self.__dict__["_inner"], name)

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
            summarize_args=[("request", _request_summary),
                            ("block_ids", _block_ids_summary)])
    def request_finished(
        self,
        request: "Request",
        block_ids: list[int],
    ) -> tuple[bool, dict[str, Any] | None]:
        return self._inner.request_finished(request, block_ids)

    @_trace("request_finished_all_groups",
            summarize_args=[("request", _request_summary),
                            ("block_ids", _block_ids_summary)])
    def request_finished_all_groups(
        self,
        request: "Request",
        block_ids: tuple[list[int], ...],
    ) -> tuple[bool, dict[str, Any] | None]:
        if hasattr(self._inner, "request_finished_all_groups"):
            return self._inner.request_finished_all_groups(request, block_ids)
        # Fall back to the single-group form if the installed vLLM lacks it.
        merged: list[int] = []
        for group in block_ids:
            merged.extend(group)
        return self._inner.request_finished(request, merged)

    @_trace("take_events")
    def take_events(self):
        if hasattr(self._inner, "take_events"):
            return self._inner.take_events()
        return []

    # ── Classmethods (delegate to the resolved inner class) ────────────────────

    @classmethod
    def build_kv_connector_stats(
        cls, data: dict[str, Any] | None = None
    ) -> "KVConnectorStats | None":
        # No vllm_config here to resolve the inner from; default to the common
        # OffloadingConnector (both CPU-offload and shmq/certus paths use it).
        from vllm.distributed.kv_transfer.kv_connector.v1.offloading_connector import (
            OffloadingConnector,
        )
        return OffloadingConnector.build_kv_connector_stats(data)

    @classmethod
    def build_prom_metrics(cls, vllm_config: "VllmConfig", *args, **kwargs):
        try:
            extra = dict(vllm_config.kv_transfer_config.kv_connector_extra_config or {})
        except Exception:
            extra = {}
        _name, inner_cls = _resolve_inner_class(extra)
        return inner_cls.build_prom_metrics(vllm_config, *args, **kwargs)

    @classmethod
    def get_required_kvcache_layout(cls, vllm_config: "VllmConfig") -> str | None:
        try:
            extra = dict(vllm_config.kv_transfer_config.kv_connector_extra_config or {})
        except Exception:
            extra = {}
        _name, inner_cls = _resolve_inner_class(extra)
        if hasattr(inner_cls, "get_required_kvcache_layout"):
            return inner_cls.get_required_kvcache_layout(vllm_config)
        return None
