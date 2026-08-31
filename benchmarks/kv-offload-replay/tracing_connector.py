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


# ── Scheduler-layer (prepare_load / prepare_store) block accounting ─────────────
#
# The exact per-transfer block count lives on the scheduler-side OffloadingManager,
# NOT on the worker threads. In vLLM's scheduler (offloading/scheduler.py):
#
#     src_spec = self.manager.prepare_load(keys_to_load, req_context)   # -> LoadSpec
#     ...
#     transfer_spec = (src_spec, dst_spec)                              # -> submit_load
#
# so prepare_load's RETURN is exactly the spec later handed to the worker's
# submit_load, and its ``keys`` argument is one entry per KV-cache block to load
# (1:1 with the GPU blocks). The store side is symmetric: prepare_store returns a
# PrepareStoreOutput whose ``keys_to_store`` is the exact set of blocks actually
# written (prefix hits already present are excluded — which is why this differs
# from ``num_external_tokens ÷ 16``). Capturing both here, on the scheduler thread,
# gets the real per-load/per-store block count WITHOUT instrumenting any worker
# thread (see the TRACING SCOPE note below for why the worker/per-layer path is
# deliberately left untraced). Duck-typed: knows nothing connector-specific.


def _safe_len(x):
    try:
        return len(x)
    except Exception:
        return None


def _spec_summary(spec) -> dict:
    """Connector-agnostic shape of a LoadStoreSpec: its medium plus whichever of
    the two block-count views it exposes — ``block_ids`` (the GPU side, one entry
    per KV-cache block = 16 tokens) and/or ``keys`` (the store side, one entry per
    store object/slab)."""
    out: dict[str, Any] = {}
    try:
        out["medium"] = spec.medium()
    except Exception:
        pass
    bids = getattr(spec, "block_ids", None)
    if bids is not None:
        n = _safe_len(bids)
        if n is not None:
            out["num_blocks"] = n
    keys = getattr(spec, "keys", None)
    if keys is not None:
        n = _safe_len(keys)
        if n is not None:
            out["num_keys"] = n
    return out


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
        self._install_passthroughs()
        # Scheduler-side manager exists as soon as the inner connector is built
        # (only for the scheduler role); instrument prepare_load/prepare_store in
        # place to capture the real per-transfer block count on the scheduler
        # thread. No-op / best-effort on the worker role (no manager there).
        self._wrap_scheduler_manager()
        logger.info(
            "TracingConnector initialized (pid=%d, role=%s) wrapping %s → %s",
            os.getpid(),
            role.name,
            self._inner_name,
            _trace_file(),
        )

    # ── Delegation of un-wrapped methods ──────────────────────────────────────
    #
    # There are TWO delegation mechanisms, because a wrapper subclassing
    # ``KVConnectorBase_V1`` faces a subtle trap: ``__getattr__`` only fires for
    # attribute *misses*, and a method the base class declares is NOT a miss — the
    # tracer *inherits* it. So for any method the base defines as a default (no-op,
    # ``False``, ``None``, …) but the real inner connector *overrides*, the tracer
    # would silently run the base default instead of the inner's real behaviour.
    # (This is exactly the bug that made vLLM 0.26's ``on_new_request`` a no-op on
    # the tracer, so the inner's ``_req_status`` was never populated and the next
    # ``get_num_new_matched_tokens`` raised ``KeyError``.)
    #
    #   * ``__getattr__``  — covers attributes the base does NOT declare (inner- or
    #     version-specific extras). Fires on a genuine miss.
    #   * ``_install_passthroughs`` — covers every PUBLIC base-declared instance
    #     method the tracer doesn't itself define, by installing an instance-level
    #     forwarder to ``self._inner`` (an instance-dict entry shadows the inherited
    #     class method). These forwarders are deliberately *untraced*: they include
    #     per-step hooks like ``has_pending_push_work`` that would flood the JSONL.
    #     The methods worth tracing are wrapped explicitly below.
    def __getattr__(self, name: str):
        # __getattr__ only fires for misses; _inner itself is a real attribute.
        return getattr(self.__dict__["_inner"], name)

    def _install_passthroughs(self) -> None:
        import inspect

        base = KVConnectorBase_V1
        # Names the tracer (or a non-base ancestor) defines explicitly — those are
        # handled/traced here, so they must NOT be overwritten by a passthrough.
        explicit: set[str] = set()
        for klass in type(self).__mro__:
            if klass in (base, SupportsHMA, object):
                continue
            explicit.update(klass.__dict__.keys())
        # The tracer deliberately keeps its OWN connector-metadata bookkeeping
        # (bind/clear sync self._connector_metadata), so don't forward its reader.
        explicit.add("has_connector_metadata")

        for name, _fn in inspect.getmembers(base, predicate=inspect.isfunction):
            if name.startswith("_") or name in explicit:
                continue
            # inspect.isfunction already excludes classmethods (bound) and
            # properties (descriptors); guard anyway for safety across versions.
            if isinstance(
                inspect.getattr_static(base, name), (classmethod, staticmethod, property)
            ):
                continue
            self.__dict__[name] = self._make_passthrough(name)

    def _make_passthrough(self, name: str):
        inner = self._inner

        def passthrough(*args, **kwargs):
            return getattr(inner, name)(*args, **kwargs)

        passthrough.__name__ = name
        return passthrough

    # ── Worker-side ───────────────────────────────────────────────────────────
    #
    # TRACING SCOPE — traced by frequency class, not by layer. Three groups are
    # cheap enough to trace and carry the block-count data this trace exists for:
    #   (1) scheduler-side, per-REQUEST: on_new_request,
    #       get_num_new_matched_tokens, update_state_after_alloc,
    #       request_finished[_all_groups] — ~5400 calls over a 450x12 run.
    #   (2) scheduler-side, per-TRANSFER: the OffloadingManager's prepare_load /
    #       prepare_store, instrumented in place by _wrap_scheduler_manager() —
    #       same order of magnitude as (1), runs on the SAME scheduler thread, and
    #       is where the exact per-load/per-store block count is visible
    #       (prepare_load's return is submit_load's src_spec; prepare_store's
    #       keys_to_store is the blocks actually written). No worker thread touched.
    #   (3) register_kv_caches / shutdown — once each.
    # Everything else is per-STEP or per-LAYER and is left as a plain UNTRACED
    # forwarder: save_kv_layer / wait_for_layer_load alone are called once per
    # layer inside the GPU forward pass, from multiple worker threads under
    # WORKLOAD_MODE=async, and were ~86k of ~147k records in a real run. Tracing
    # those funnels every forward-pass layer through the single _fh_lock + a
    # line-buffered flush syscall, which stalled the engine. (The
    # _connector_metadata plumbing on start_load_kv/wait_for_save is preserved.)

    @_trace("register_kv_caches")
    def register_kv_caches(self, kv_caches: dict[str, torch.Tensor]):
        return self._inner.register_kv_caches(kv_caches)

    @_trace("register_cross_layers_kv_cache")
    def register_cross_layers_kv_cache(
        self, kv_cache: torch.Tensor, attn_backend: Any
    ):
        return self._inner.register_cross_layers_kv_cache(kv_cache, attn_backend)

    # ── Scheduler-manager wrapping (instrument prepare_load/prepare_store) ──────

    def _find_offloading_manager(self):
        """Locate the scheduler-side OffloadingManager without importing or naming
        any connector-specific type. Tries vLLM's known path
        (connector_scheduler.manager) first, then a bounded breadth-first walk of
        the inner connector's attribute graph for anything exposing the manager's
        prepare_load/prepare_store primitives."""

        def looks_like_manager(obj):
            return (
                obj is not None
                and callable(getattr(obj, "prepare_load", None))
                and callable(getattr(obj, "prepare_store", None))
            )

        cs = getattr(self._inner, "connector_scheduler", None)
        m = getattr(cs, "manager", None)
        if looks_like_manager(m):
            return m

        seen: set[int] = set()
        queue = [self._inner]
        budget = 300
        while queue and budget > 0:
            budget -= 1
            obj = queue.pop(0)
            if id(obj) in seen:
                continue
            seen.add(id(obj))
            if looks_like_manager(obj):
                return obj
            d = getattr(obj, "__dict__", None)
            if isinstance(d, dict):
                for v in d.values():
                    if hasattr(v, "__dict__") and id(v) not in seen:
                        queue.append(v)
        return None

    def _manager_record(self, method: str, fields: dict, t0: float, error, result):
        try:
            role_str = self.role.name
        except Exception:
            role_str = "unknown"
        rec: dict[str, Any] = {
            "pid": os.getpid(),
            "ts": t0,
            "elapsed": round(time.perf_counter() - t0, 9),
            "role": role_str,
            "layer": "manager",
            "connector": "TracingConnector",
            "inner": self._inner_name,
            "method": method,
        }
        rec.update(fields)
        rec["error"] = error
        if error is None:
            rec["result"] = _safe_repr(result, 120)
        _write(rec)

    def _wrap_scheduler_manager(self) -> None:
        """Instrument the OffloadingManager's ``prepare_load`` / ``prepare_store``
        IN PLACE (instance-attribute shadows the bound method), on the scheduler
        thread. prepare_load's return IS the src_spec later handed to the worker's
        submit_load, and prepare_store's PrepareStoreOutput carries the exact
        ``keys_to_store`` — so this captures the real per-transfer block count with
        no worker-thread instrumentation. Best-effort and idempotent."""
        try:
            manager = self._find_offloading_manager()
            if manager is None:
                logger.info(
                    "TracingConnector: no OffloadingManager found under %s — "
                    "scheduler-layer (prepare_load/prepare_store) tracing disabled "
                    "(expected on the worker role)",
                    self._inner_name,
                )
                return
            if getattr(manager, "_tracing_wrapped", False):
                return  # idempotent

            orig_load = manager.prepare_load
            orig_store = manager.prepare_store
            record = self._manager_record

            @functools.wraps(orig_load)
            def traced_prepare_load(keys, req_context, *a, **k):
                t0 = time.perf_counter()
                error = spec = None
                try:
                    spec = orig_load(keys, req_context, *a, **k)
                    return spec
                except Exception as exc:
                    error = f"{type(exc).__name__}: {exc}"
                    raise
                finally:
                    # len(keys) = KV blocks to load, 1:1 with the GPU blocks; the
                    # returned spec is submit_load's src_spec.
                    record(
                        "prepare_load",
                        {
                            "req_id": getattr(req_context, "req_id", None),
                            "load_blocks": _safe_len(keys),
                            "spec": _spec_summary(spec) if spec is not None else None,
                        },
                        t0,
                        error,
                        spec,
                    )

            @functools.wraps(orig_store)
            def traced_prepare_store(keys, req_context, *a, **k):
                t0 = time.perf_counter()
                error = out = None
                try:
                    out = orig_store(keys, req_context, *a, **k)
                    return out
                except Exception as exc:
                    error = f"{type(exc).__name__}: {exc}"
                    raise
                finally:
                    # keys_to_store = blocks actually written (prefix hits already
                    # present are excluded); this is the exact per-store count.
                    fields = {
                        "req_id": getattr(req_context, "req_id", None),
                        "offer_blocks": _safe_len(keys),
                    }
                    if out is not None:
                        fields["store_blocks"] = _safe_len(
                            getattr(out, "keys_to_store", None)
                        )
                        fields["evicted"] = _safe_len(
                            getattr(out, "evicted_keys", None)
                        )
                        fields["spec"] = _spec_summary(
                            getattr(out, "store_spec", None)
                        )
                    else:
                        # prepare_store returns None when blocks cannot be stored.
                        fields["store_blocks"] = 0
                    record("prepare_store", fields, t0, error, out)

            manager.prepare_load = traced_prepare_load
            manager.prepare_store = traced_prepare_store
            manager._tracing_wrapped = True
            logger.info(
                "TracingConnector: instrumented OffloadingManager "
                "prepare_load/prepare_store for %s (scheduler-thread block "
                "accounting)",
                self._inner_name,
            )
        except Exception as e:  # never let tracing break the run
            logger.warning(
                "TracingConnector: scheduler-manager wrap skipped (%r)", e
            )

    def bind_connector_metadata(self, connector_metadata: KVConnectorMetadata) -> None:
        self._inner.bind_connector_metadata(connector_metadata)
        self._connector_metadata = self._inner._connector_metadata

    def clear_connector_metadata(self) -> None:
        self._inner.clear_connector_metadata()
        self._connector_metadata = None

    def handle_preemptions(self, kv_connector_metadata: KVConnectorMetadata):
        return self._inner.handle_preemptions(kv_connector_metadata)

    def start_load_kv(self, forward_context: "ForwardContext", **kwargs: Any) -> None:
        self._inner._connector_metadata = self._connector_metadata
        return self._inner.start_load_kv(forward_context, **kwargs)

    def wait_for_layer_load(self, layer_name: str) -> None:
        return self._inner.wait_for_layer_load(layer_name)

    def save_kv_layer(
        self,
        layer_name: str,
        kv_layer: torch.Tensor,
        attn_metadata: "AttentionMetadata",
        **kwargs: Any,
    ) -> None:
        return self._inner.save_kv_layer(layer_name, kv_layer, attn_metadata, **kwargs)

    def wait_for_save(self):
        self._inner._connector_metadata = self._connector_metadata
        return self._inner.wait_for_save()

    def get_finished(
        self, finished_req_ids: set[str]
    ) -> tuple[set[str] | None, set[str] | None]:
        return self._inner.get_finished(finished_req_ids)

    def build_connector_worker_meta(self) -> KVConnectorWorkerMetadata | None:
        return self._inner.build_connector_worker_meta()

    def get_kv_connector_stats(self) -> "KVConnectorStats | None":
        return self._inner.get_kv_connector_stats()

    def get_kv_connector_kv_cache_events(self) -> "KVConnectorKVEvents | None":
        return self._inner.get_kv_connector_kv_cache_events()

    @_trace("shutdown")
    def shutdown(self):
        return self._inner.shutdown()

    # ── Scheduler-side ────────────────────────────────────────────────────────

    @_trace("on_new_request", summarize_args=[("request", _request_summary)])
    def on_new_request(self, request: "Request") -> None:
        # vLLM 0.26+ lifecycle hook: the inner connector records per-request state
        # here (OffloadingConnector populates its scheduler's _req_status), which a
        # later get_num_new_matched_tokens then reads. Forwarding this is REQUIRED —
        # the base-class default is a no-op, so inheriting it silently breaks the
        # inner's request bookkeeping. Guarded so it stays a no-op on older vLLM
        # (<0.26) whose connectors don't define the hook.
        if hasattr(self._inner, "on_new_request"):
            return self._inner.on_new_request(request)
        return None

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

    # Per-step, no per-request block data → untraced forwarders (see TRACING SCOPE).
    def build_connector_meta(
        self, scheduler_output: "SchedulerOutput"
    ) -> KVConnectorMetadata:
        return self._inner.build_connector_meta(scheduler_output)

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

    @classmethod
    def requires_piecewise_for_cudagraph(cls, extra_config: dict[str, Any]) -> bool:
        # Resolvable straight from extra_config (same dict the tracer reads its
        # traced_kv_connector key from), so forward to the inner class's decision
        # rather than inheriting the base default and mis-selecting cudagraph mode.
        _name, inner_cls = _resolve_inner_class(extra_config)
        if hasattr(inner_cls, "requires_piecewise_for_cudagraph"):
            return inner_cls.requires_piecewise_for_cudagraph(extra_config)
        return False
