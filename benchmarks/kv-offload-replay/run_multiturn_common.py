"""run_multiturn_common.py — shared setup for the multi-turn ShareGPT replay.

This module owns the pieces both execution models need, so the five backend
drivers
(``run_multiturn_nooffload.py``, ``run_multiturn_offloading.py``,
``run_fs_bench_450.py``, ``run_fs_bench_450_iostat.py`` and
``certus-shmq-connector/run_multiturn_shmq_certus.py``) stop copy-pasting them:
dataset loading + turn extraction (``load_convs``), engine construction
(``build_engine``), the tokenizer-length closure (``make_n_tokens``), the MFU
probe (``mfu_kwargs``), the Prometheus exporter (``start_prom_exporter``), and
the telemetry helpers (``prom_counters``, ``prom_histograms``, ``hist_pct``,
``disk_rw_bytes``, ``gib``).

The two *execution models* live in their own siblings and both build on this
module: the default synchronous batched-round loop in
:mod:`run_multiturn_sync_batched` (``run_batched``) and the async
per-conversation model in :mod:`run_multiturn_async` (``run_async`` /
``run_async_driver``).

The *backend* — how vLLM is constructed (``kv_transfer_config``, engine kwargs)
and what telemetry each run captures — stays in the driver, which assembles
``engine_kwargs`` and passes them here.

Nothing here imports vllm at module load, so the driver stays in control of when
the (heavy) engine import happens.
"""

import json
import sys


# ── Workload input ────────────────────────────────────────────────────────
def load_convs(dataset_path, num_convs, conv_multiplier=1):
    """Load a ShareGPT-format json and return a list of human-turn streams.

    Each returned element is the list of ``human`` turn strings for one
    conversation (only conversations with >= 2 human turns are kept), capped at
    ``num_convs`` conversations.

    ``conv_multiplier`` > 1 replicates the whole set N times for a larger
    concurrent working set; each replica's first turn is tagged ``[r{n}] `` so
    the accumulated context hashes distinctly per replica (otherwise
    byte-identical copies would dedup at the prefix-cache / KV-block layer and
    store no extra data). Replica order is (r0: all convs), (r1: all convs), …
    """
    with open(dataset_path) as f:
        all_data = json.load(f)
    convs = []
    for entry in all_data:
        if len(convs) >= num_convs:
            break
        turns = entry.get("conversations", [])
        human_turns = [t["value"] for t in turns if t.get("from") == "human"]
        if len(human_turns) >= 2:
            convs.append(human_turns)

    if conv_multiplier > 1:
        base = convs
        convs = []
        for r in range(conv_multiplier):
            for conv in base:
                tagged = list(conv)
                tagged[0] = f"[r{r}] {tagged[0]}"
                convs.append(tagged)
    return convs


# ── Shared telemetry helpers (lifted verbatim from the drivers) ─────────────
def gib(n):
    """Format a byte count as GiB, or 'n/a' for None."""
    return "n/a" if n is None else f"{n / (1024 ** 3):.2f} GiB"


def disk_rw_bytes(disk_stat_path):
    """Return (bytes_read, bytes_written) cumulative for a /sys/block/<dev>/stat
    path, or (None, None) if unreadable. Fields 3 and 7 are sectors read/written
    in 512-byte units."""
    if not disk_stat_path:
        return None, None
    try:
        with open(disk_stat_path) as f:
            fields = f.read().split()
        return int(fields[2]) * 512, int(fields[6]) * 512
    except (OSError, IndexError, ValueError):
        return None, None


def prom_counters(llm, capture=True):
    """Snapshot vLLM's cumulative ``vllm:`` Counter metrics into a name->value
    dict. Prefers the V1 offline ``llm.get_metrics()`` snapshot (engine counters
    live there, not on the global REGISTRY in offline mode), then supplements
    with the global prometheus REGISTRY (V0 engines, and connector-registered
    ``vllm:kv_offload_*`` which only ever live on the registry). Names differ by
    source — get_metrics() uses bare names, REGISTRY samples carry ``_total`` —
    so both keys are kept."""
    vals = {}
    if not capture:
        return vals
    try:
        for m in llm.get_metrics():
            if type(m).__name__ != "Counter":
                continue
            name = getattr(m, "name", "")
            val = getattr(m, "value", None)
            if name.startswith("vllm:") and isinstance(val, (int, float)):
                # Sum across label sets: get_metrics() emits one Counter per
                # sample, so labeled metrics share a name.
                vals[name] = vals.get(name, 0.0) + float(val)
    except Exception:  # noqa: BLE001 - get_metrics() is V1-only; skip on V0
        pass
    try:
        from prometheus_client import REGISTRY
        for metric in REGISTRY.collect():
            if not metric.name.startswith("vllm:"):
                continue
            for s in metric.samples:
                if s.name.endswith("_total"):
                    vals[s.name] = vals.get(s.name, 0.0) + float(s.value)
    except Exception as e:  # noqa: BLE001
        print(f"[prom] collect failed: {e}", file=sys.stderr, flush=True)
    return vals


def prom_histograms(llm, names, capture=True):
    """Sample the named vLLM latency histograms once (cumulative over the run).
    get_metrics() exposes each as Histogram(count, sum, buckets); sum across
    label sets into one distribution per name."""
    out = {}
    if not capture:
        return out
    try:
        for m in llm.get_metrics():
            if type(m).__name__ != "Histogram":
                continue
            name = getattr(m, "name", "")
            if name not in names:
                continue
            agg = out.setdefault(name, {"count": 0, "sum": 0.0, "buckets": {}})
            agg["count"] += int(getattr(m, "count", 0))
            agg["sum"] += float(getattr(m, "sum", 0.0))
            for le, c in (getattr(m, "buckets", {}) or {}).items():
                agg["buckets"][le] = agg["buckets"].get(le, 0) + int(c)
    except Exception as e:  # noqa: BLE001 - get_metrics() is V1-only
        print(f"[prom] histogram sample failed: {e}", file=sys.stderr)
    return out


def hist_pct(buckets, count, p):
    """Percentile from cumulative buckets: the smallest upper bound ``le`` whose
    cumulative count first reaches ``p*count``. Bucket-granular; returns inf when
    the crossing lands in the +Inf bucket, None for an empty histogram."""
    if not count:
        return None
    target = p * count

    def _le(k):
        return float("inf") if k in ("+Inf", "inf", "Inf") else float(k)

    for le in sorted(buckets, key=_le):
        if buckets[le] >= target:
            return _le(le)
    return float("inf")


# ── Shared engine construction / setup (lifted from the drivers) ─────────────
# These own the boilerplate every driver repeated around the backend-specific
# kv_transfer_config: the common LLM kwargs, the MFU-metrics probe, the optional
# Prometheus exporter, and the tokenizer-length closure. Both execution models
# use them — run_batched in run_multiturn_sync_batched and the async model in
# run_multiturn_async (build_engine(..., async_mode=True) + run_async) — so
# setup is not forked.
def build_engine(engine_kwargs, *, async_mode=False):
    """Construct the vLLM engine from a fully-assembled kwargs dict.

    ``async_mode=False`` (default) returns a synchronous ``LLM(**engine_kwargs)``
    — byte-identical to the drivers' former inline construction. ``async_mode=True``
    returns a V1 ``AsyncLLM`` built from ``AsyncEngineArgs(**engine_kwargs)`` for
    the per-conversation :func:`run_async` path; a ``kv_transfer_config`` given as
    a dict is converted to ``KVTransferConfig`` (AsyncEngineArgs wants the typed
    object, whereas ``LLM`` accepts the dict directly).

    The same ``engine_kwargs`` feed both modes, so a driver flips between batched
    and async by toggling this one flag — the backend config is not duplicated.
    """
    if not async_mode:
        from vllm import LLM
        return LLM(**engine_kwargs)

    from vllm import AsyncEngineArgs
    from vllm.v1.engine.async_llm import AsyncLLM

    kwargs = dict(engine_kwargs)
    kv = kwargs.get("kv_transfer_config")
    if isinstance(kv, dict):
        from vllm.config import KVTransferConfig
        kwargs["kv_transfer_config"] = KVTransferConfig(**kv)
    return AsyncLLM.from_engine_args(AsyncEngineArgs(**kwargs))


def make_n_tokens(tokenizer, flavor="input_ids"):
    """Return a ``text -> token count`` callable.

    ``flavor="input_ids"`` uses ``len(tokenizer(text).input_ids)`` (nooffload,
    offloading, shmq); ``flavor="encode"`` uses ``len(tokenizer.encode(text))``
    (the fs_bench drivers). Both count the same for these tokenizers; the flavor
    only preserves each driver's exact prior call."""
    if flavor == "encode":
        return lambda text: len(tokenizer.encode(text))
    return lambda text: len(tokenizer(text).input_ids)


def mfu_kwargs(capture=True, *, verbose=True):
    """Return ``{"enable_mfu_metrics": True}`` iff the running vLLM's EngineArgs
    accepts it, else ``{}``. Emits the same two ``[prom]`` lines the drivers
    printed. ``capture=False`` short-circuits to ``{}`` with no probe/print."""
    kwargs = {}
    if not capture:
        return kwargs
    try:
        import dataclasses as _dc
        from vllm.engine.arg_utils import EngineArgs as _EA
        if any(f.name == "enable_mfu_metrics" for f in _dc.fields(_EA)):
            kwargs["enable_mfu_metrics"] = True
    except Exception as e:  # noqa: BLE001
        if verbose:
            print(f"[prom] enable_mfu_metrics unavailable: {e}", file=sys.stderr)
    if verbose:
        print(f"[prom] enable_mfu_metrics={bool(kwargs)} "
              f"(estimated_flops_per_gpu_total "
              f"{'captured per round' if kwargs else 'ABSENT'})",
              file=sys.stderr)
    return kwargs


def start_prom_exporter():
    """Start the optional Prometheus HTTP exporter iff ``PROM_PORT`` is set.

    Reads ``PROM_PORT``/``LOG_STATS`` from the environment and reproduces the
    drivers' former inline block (start the server, warn when LOG_STATS is off so
    the endpoint would serve an empty registry). No-op when ``PROM_PORT`` is
    unset."""
    import os
    port = os.environ.get("PROM_PORT")
    if not port:
        return
    from prometheus_client import start_http_server
    start_http_server(int(port))
    print(f"[prom] metrics exporter listening on :{port}/metrics", file=sys.stderr)
    if os.environ.get("LOG_STATS", "0") == "0":
        print(
            "[prom] warning: LOG_STATS is off — vLLM metrics are not "
            "registered, so /metrics will be empty. Set LOG_STATS=1.",
            file=sys.stderr,
        )


# The two execution models live in their own siblings and carry no shared setup:
# the default synchronous batched-round loop (run_batched) in
# run_multiturn_sync_batched, and the opt-in async per-conversation model
# (run_async, run_async_driver) in run_multiturn_async.
