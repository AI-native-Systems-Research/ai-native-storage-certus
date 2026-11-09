#!/usr/bin/env python3
"""
trace_kvconn.py — monkey-patch KVConnectorBase_V1 to record every method call.

Output: kvconn_trace.jsonl  (one JSON object per line)
Each record:
    {
        "ts":        float,   # time.perf_counter() at call start
        "elapsed":   float,   # seconds the call took
        "role":      str,     # "SCHEDULER" | "WORKER" | "unknown"
        "connector": str,     # connector class name
        "method":    str,
        "args":      list,    # repr of positional args (excluding self)
        "kwargs":    dict,    # repr of keyword args
        "result":    str,     # repr of return value
        "error":     str|null # exception message if raised
    }
"""

import functools
import json
import sys
import time
import traceback
from pathlib import Path

TRACE_FILE = Path(__file__).parent / "kvconn_trace.jsonl"

# Methods on KVConnectorBase_V1 we want to trace.
# Split by side so we can label records clearly.
SCHEDULER_METHODS = {
    "get_num_new_matched_tokens",
    "update_state_after_alloc",
    "build_connector_meta",
    "update_connector_output",
    "request_finished",
    "take_events",
}

WORKER_METHODS = {
    "register_kv_caches",
    "register_cross_layers_kv_cache",
    "bind_connector_metadata",
    "clear_connector_metadata",
    "handle_preemptions",
    "start_load_kv",
    "wait_for_layer_load",
    "save_kv_layer",
    "wait_for_save",
    "get_finished",
    "get_block_ids_with_load_errors",
    "build_connector_worker_meta",
    "get_kv_connector_stats",
    "get_kv_connector_kv_cache_events",
    "shutdown",
}

ALL_METHODS = SCHEDULER_METHODS | WORKER_METHODS

_trace_fh = None


def _open_trace():
    global _trace_fh
    if _trace_fh is None:
        TRACE_FILE.parent.mkdir(parents=True, exist_ok=True)
        _trace_fh = open(TRACE_FILE, "w")
    return _trace_fh


def _write(record: dict):
    fh = _open_trace()
    fh.write(json.dumps(record) + "\n")
    fh.flush()


def _safe_repr(obj, maxlen=120) -> str:
    try:
        s = repr(obj)
    except Exception:
        s = "<repr-error>"
    return s[:maxlen] + "…" if len(s) > maxlen else s


def _wrap(method_name: str, original_fn):
    if method_name in SCHEDULER_METHODS:
        side = "SCHEDULER"
    elif method_name in WORKER_METHODS:
        side = "WORKER"
    else:
        side = "unknown"

    @functools.wraps(original_fn)
    def wrapper(self, *args, **kwargs):
        connector_name = type(self).__name__
        # prefer the role attribute when available
        try:
            role_str = self.role.name
        except Exception:
            role_str = side

        arg_reprs = [_safe_repr(a) for a in args]
        kwarg_reprs = {k: _safe_repr(v) for k, v in kwargs.items()}

        t0 = time.perf_counter()
        error = None
        result = None
        try:
            result = original_fn(self, *args, **kwargs)
            return result
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            raise
        finally:
            elapsed = time.perf_counter() - t0
            _write({
                "ts": t0,
                "elapsed": round(elapsed, 9),
                "role": role_str,
                "connector": connector_name,
                "method": method_name,
                "args": arg_reprs,
                "kwargs": kwarg_reprs,
                "result": _safe_repr(result) if error is None else None,
                "error": error,
            })

    return wrapper


def install():
    """Monkey-patch KVConnectorBase_V1 with tracing wrappers."""
    from vllm.distributed.kv_transfer.kv_connector.v1.base import KVConnectorBase_V1

    patched = 0
    for name in ALL_METHODS:
        fn = getattr(KVConnectorBase_V1, name, None)
        if fn is None:
            continue
        # avoid double-patching
        if getattr(fn, "_kvtrace_patched", False):
            continue
        wrapped = _wrap(name, fn)
        wrapped._kvtrace_patched = True
        setattr(KVConnectorBase_V1, name, wrapped)
        patched += 1

    print(f"[kvtrace] Patched {patched} methods on KVConnectorBase_V1 → {TRACE_FILE}",
          file=sys.stderr)


def close():
    global _trace_fh
    if _trace_fh is not None:
        _trace_fh.close()
        _trace_fh = None


if __name__ == "__main__":
    install()
    print("[kvtrace] Patch installed. Import this module before using vllm.", file=sys.stderr)
