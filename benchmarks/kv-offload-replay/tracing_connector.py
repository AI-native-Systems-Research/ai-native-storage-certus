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


# ── Worker-layer (submit_load / submit_store / transfer_async) tracing ──────────
#
# These live BELOW the connector, inside the OffloadingConnector's worker: it
# holds one or more OffloadingHandler objects (for the shmq/certus path, a
# CertusShmqWorker) and calls handler.submit_load / submit_store (vLLM 0.26) or
# handler.transfer_async (<=0.24) once per TRANSFER JOB. We reach them WITHOUT
# knowing anything connector-specific: after the inner connector registers its
# KV caches, we locate its OffloadingWorker by duck typing and swap each handler
# for the proxy below, which logs the per-submit block/key counts and delegates.
# Per-job (thousands over a run), NOT per-layer, so it does not reintroduce the
# per-forward-pass overhead the connector-layer scope note (below) avoids.


def _spec_summary(spec) -> dict:
    """Connector-agnostic shape of a LoadStoreSpec: its medium plus whichever of
    the two block-count views it exposes — ``block_ids`` (the GPU side, one entry
    per KV-cache block = 16 tokens) and/or ``keys`` (the store side, one entry per
    store object/slab). Capturing both is the whole point: they are the two
    different "block" counts, side by side, per submit."""
    out: dict[str, Any] = {}
    try:
        out["medium"] = spec.medium()
    except Exception:
        pass
    bids = getattr(spec, "block_ids", None)
    if bids is not None:
        try:
            out["num_blocks"] = len(bids)
        except Exception:
            pass
    keys = getattr(spec, "keys", None)
    if keys is not None:
        try:
            out["num_keys"] = len(keys)
        except Exception:
            pass
    return out


def _summarize_transfer(src, dst) -> tuple[dict, int | None, int | None]:
    """(summary, gpu_blocks, store_keys) for a (src, dst) transfer pair.

    ``gpu_blocks`` = block_ids on the GPU-medium side (the count that lines up
    with ``num_external_tokens ÷ 16``); ``store_keys`` = keys on the store side
    (the coarser store-object count). Either may be None if a side doesn't expose
    it."""
    ss, ds = _spec_summary(src), _spec_summary(dst)
    gpu_blocks = None
    for x in (ss, ds):
        if x.get("medium") == "GPU" and "num_blocks" in x:
            gpu_blocks = x["num_blocks"]
    if gpu_blocks is None:  # fall back to any block_ids present
        gpu_blocks = ss.get("num_blocks", ds.get("num_blocks"))
    store_keys = ss.get("num_keys")
    if store_keys is None:
        store_keys = ds.get("num_keys")
    return {"src": ss, "dst": ds}, gpu_blocks, store_keys


class _TracingHandler:
    """Delegating proxy around a vLLM ``OffloadingHandler``.

    Wraps only the data-movement *submit* entry points (``submit_load`` /
    ``submit_store`` on 0.26, ``transfer_async`` on <=0.24), logging the per-call
    block/key counts; everything else (``get_finished``, ``wait``, ``shutdown``,
    ``medium``, …) passes straight through untraced — those are per-step polls and
    would flood the trace. Knows nothing about which connector built the handler."""

    def __init__(self, inner, inner_name: str):
        self._inner_handler = inner
        self._inner_name = inner_name

    def __getattr__(self, name):
        # Fires only for attributes this proxy doesn't define (get_finished, wait,
        # shutdown, medium, spec-specific extras) — delegate to the real handler.
        return getattr(self.__dict__["_inner_handler"], name)

    def _record(self, method: str, job_id, src, dst, t0, error, result):
        summary, gpu_blocks, store_keys = _summarize_transfer(src, dst)
        rec: dict[str, Any] = {
            "pid": os.getpid(),
            "ts": t0,
            "elapsed": round(time.perf_counter() - t0, 9),
            "role": "WORKER",
            "layer": "worker",
            "connector": "TracingConnector",
            "inner": self._inner_name,
            "method": method,
            "job_id": int(job_id) if isinstance(job_id, int) else _safe_repr(job_id, 40),
            "gpu_blocks": gpu_blocks,
            "store_keys": store_keys,
            "summary": summary,
            "error": error,
        }
        if error is None:
            rec["result"] = _safe_repr(result, 80)
        _write(rec)

    # 0.26 explicit-direction interface
    def submit_load(self, job_id, src_spec, dst_spec) -> bool:
        t0 = time.perf_counter()
        error = result = None
        try:
            result = self._inner_handler.submit_load(job_id, src_spec, dst_spec)
            return result
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            raise
        finally:
            self._record("submit_load", job_id, src_spec, dst_spec, t0, error, result)

    def submit_store(self, job_id, src_spec, dst_spec) -> bool:
        t0 = time.perf_counter()
        error = result = None
        try:
            result = self._inner_handler.submit_store(job_id, src_spec, dst_spec)
            return result
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            raise
        finally:
            self._record("submit_store", job_id, src_spec, dst_spec, t0, error, result)

    # <=0.24 medium-pair interface: spec is (src, dst)
    def transfer_async(self, job_id, spec) -> bool:
        t0 = time.perf_counter()
        error = result = None
        try:
            result = self._inner_handler.transfer_async(job_id, spec)
            return result
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            raise
        finally:
            try:
                src, dst = spec
            except Exception:
                src = dst = None
            self._record("transfer_async", job_id, src, dst, t0, error, result)


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
    #   (2) worker-side, per-TRANSFER-JOB: the OffloadingWorker's handler
    #       submit_load / submit_store / transfer_async, traced via the
    #       _TracingHandler proxies installed in _wrap_worker_handlers() — same
    #       order of magnitude as (1), and the ONLY place the exact per-submit
    #       block/key count is visible. (This is the layer the user asked for.)
    #   (3) register_kv_caches / shutdown — once each.
    # Everything else is per-STEP or per-LAYER and is left as a plain UNTRACED
    # forwarder: save_kv_layer / wait_for_layer_load alone are called once per
    # layer inside the GPU forward pass, from multiple worker threads under
    # WORKLOAD_MODE=async, and were ~86k of ~147k records in a real run. Tracing
    # those funnels every forward-pass layer through the single _fh_lock + a
    # line-buffered flush syscall, which stalled the engine. The handler
    # get_finished / wait polls are per-step for the same reason, so the proxy
    # leaves them untraced too. (The _connector_metadata plumbing on
    # start_load_kv/wait_for_save is preserved.)

    @_trace("register_kv_caches")
    def register_kv_caches(self, kv_caches: dict[str, torch.Tensor]):
        result = self._inner.register_kv_caches(kv_caches)
        # Handlers are created inside the inner connector's register — wrap them
        # now so every subsequent submit_load/submit_store is traced.
        self._wrap_worker_handlers()
        return result

    @_trace("register_cross_layers_kv_cache")
    def register_cross_layers_kv_cache(
        self, kv_cache: torch.Tensor, attn_backend: Any
    ):
        result = self._inner.register_cross_layers_kv_cache(kv_cache, attn_backend)
        # Cross-layer registration can (re)register handlers too — re-wrap
        # (idempotent: already-wrapped handlers are left alone).
        self._wrap_worker_handlers()
        return result

    # ── Worker-handler wrapping (locate the OffloadingWorker, swap in proxies) ──

    @staticmethod
    def _looks_like_offloading_worker(obj) -> bool:
        return (
            obj is not None
            and hasattr(obj, "transfer_type_to_handler")
            and hasattr(obj, "handlers")
        )

    def _find_offloading_worker(self):
        """Locate the inner connector's OffloadingWorker without importing or
        naming any connector-specific type. Tries vLLM's known path
        (connector_worker.worker) first, then a bounded breadth-first walk of the
        inner connector's attribute graph for anything that quacks like one."""
        cw = getattr(self._inner, "connector_worker", None)
        w = getattr(cw, "worker", None)
        if self._looks_like_offloading_worker(w):
            return w

        seen: set[int] = set()
        queue = [self._inner]
        budget = 300
        while queue and budget > 0:
            budget -= 1
            obj = queue.pop(0)
            if id(obj) in seen:
                continue
            seen.add(id(obj))
            if self._looks_like_offloading_worker(obj):
                return obj
            d = getattr(obj, "__dict__", None)
            if isinstance(d, dict):
                for v in d.values():
                    if hasattr(v, "__dict__") and id(v) not in seen:
                        queue.append(v)
        return None

    def _wrap_worker_handlers(self) -> None:
        """Replace each handler in the OffloadingWorker with a _TracingHandler
        proxy, in BOTH the ``handlers`` set and the ``transfer_type_to_handler``
        map (they alias the same objects). Idempotent and fully best-effort: any
        failure is logged and left to not disturb the run."""
        try:
            worker = self._find_offloading_worker()
            if worker is None:
                logger.info(
                    "TracingConnector: no OffloadingWorker found under %s — "
                    "worker-layer (submit_load/submit_store) tracing disabled",
                    self._inner_name,
                )
                return
            handlers = getattr(worker, "handlers", None)
            ttoh = getattr(worker, "transfer_type_to_handler", None)

            proxies: dict[int, _TracingHandler] = {}

            def proxy_for(h):
                if isinstance(h, _TracingHandler):
                    return h
                p = proxies.get(id(h))
                if p is None:
                    p = _TracingHandler(h, self._inner_name)
                    proxies[id(h)] = p
                return p

            # transfer_type_to_handler: rewrite values in place.
            if isinstance(ttoh, dict):
                for ttype, h in list(ttoh.items()):
                    ttoh[ttype] = proxy_for(h)

            # handlers: a set of the same objects — rebuild in place so the
            # worker's reference to the set survives.
            if isinstance(handlers, set):
                wrapped = {proxy_for(h) for h in handlers}
                handlers.clear()
                handlers.update(wrapped)
            elif isinstance(handlers, (list, tuple)):
                new = [proxy_for(h) for h in handlers]
                try:
                    handlers[:] = new  # list, mutate in place
                except TypeError:
                    setattr(worker, "handlers", type(handlers)(new))

            if proxies:
                logger.info(
                    "TracingConnector: wrapped %d worker handler(s) for %s — "
                    "submit_load/submit_store/transfer_async now traced",
                    len(proxies),
                    self._inner_name,
                )
        except Exception as e:  # never let tracing break the run
            logger.warning(
                "TracingConnector: worker-handler wrap skipped (%r)", e
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
