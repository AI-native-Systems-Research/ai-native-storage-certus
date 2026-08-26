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

import glob
import json
import os
import sys


# ── Named workloads ─────────────────────────────────────────────────────────
# A "workload" names a dataset (+ a sensible default conversation count) so a
# driver can select it with WORKLOAD_NAME=<name> instead of spelling out
# DATASET_PATH. (Selector env is WORKLOAD_NAME, not WORKLOAD — see resolve_workload.)
# The repo-root data dir, resolved relative to this module
# (benchmarks/kv-offload-replay/ -> ../../data).
_DATA_DIR = os.path.normpath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "data")
)

# Total conversations in the full ShareGPT corpus (all data/sharegpt/*.json
# chunks). Used as the default conversation cap for the corpus (min-turns-1)
# case — i.e. "draw from the whole corpus" — since load_convs' cap is what
# bounds how much of the corpus is read.
_SHAREGPT_CORPUS_CONVS = 94145


def _sharegpt_turns():
    """(min_turns, max_turns) for the ``sharegpt`` workload from the environment.

    ``SHAREGPT_MIN_TURNS`` defaults to 12; ``SHAREGPT_MAX_TURNS`` defaults to
    ``min`` when unset, so a bare ``--min-turns N`` selects the ``N/N`` config."""
    min_env = os.environ.get("SHAREGPT_MIN_TURNS")
    max_env = os.environ.get("SHAREGPT_MAX_TURNS")
    min_turns = int(min_env) if min_env else 12
    max_turns = int(max_env) if max_env else min_turns
    return min_turns, max_turns


def _sharegpt_dataset(has_explicit_path=False):
    """Dataset for the ``sharegpt`` workload, selected by human-turn count via
    ``SHAREGPT_MIN_TURNS`` / ``SHAREGPT_MAX_TURNS``.

    Exactly TWO configurations are prepared, and only these two are accepted:

    * ``min==12`` and ``max==12`` -> ``data/sharegpt_12turn_450.json``: 450
      conversations, each with exactly 12 human turns (the set every bench image
      bakes as its ``DATASET_PATH``).
    * ``min==2`` and ``max==2`` -> the ``data/sharegpt/`` directory: the FULL
      ShareGPT corpus (all chunks, 94,145 conversations). ``load_convs`` reads
      every ``*.json`` chunk in order and caps at ``num_convs`` — so
      ``min-turns 2`` means "draw from the whole corpus".

    ``2`` is the honest floor for the corpus: ``load_convs`` keeps only
    conversations with ``>= 2`` human turns (a single-turn conversation has no
    prior context for a multi-turn KV-reuse workload to reuse), so ``min-turns 1``
    loads the identical set. ``1`` is therefore accepted as a back-compat alias
    for ``2`` but ``2`` is the canonical value.

    ``max-turns`` defaults to ``min-turns`` when unset, so ``--min-turns 2`` and
    ``--min-turns 12`` alone each select a prepared config. Any other pair — in
    particular any ``max-turns`` value that does not mirror ``min`` (2 with min
    2, 12 with min 12) — is rejected: set ``DATASET_PATH`` to a custom
    turn-filtered ShareGPT file instead. When ``DATASET_PATH`` is set this returns
    ``None`` so that path takes over rather than erroring; otherwise it exits with
    the hint.

    Note the corpus directory is resolved ``__file__``-relative, which is only
    correct where the module keeps its repo layout (host runs, and the shmq
    image). The other bench images flatten the layout, so the orchestrator
    overrides ``DATASET_PATH`` with the in-container mount point for the corpus
    case rather than relying on this path."""
    min_turns, max_turns = _sharegpt_turns()
    if min_turns == 12 and max_turns == 12:
        return os.path.join(_DATA_DIR, "sharegpt_12turn_450.json")
    # min-turns 2 = the whole corpus (load_convs' own >=2 floor); 1 accepted as a
    # legacy alias for 2 since it loads the identical set.
    if min_turns in (1, 2) and max_turns == min_turns:
        return os.path.join(_DATA_DIR, "sharegpt")  # directory of corpus chunks
    if has_explicit_path:
        return None
    raise SystemExit(
        "[run] WORKLOAD_NAME=sharegpt: only min-turns==max-turns==2 (the full "
        "94,145-conv corpus; 1 also accepted) or min-turns==max-turns==12 (the "
        f"450-conv subset) are prepared, got min={min_turns} max={max_turns}. "
        "Set DATASET_PATH to a custom turn-filtered ShareGPT dataset for other "
        "turn counts."
    )


def _sharegpt_num_convs(has_explicit_path=False):
    """Default conversation cap for the ``sharegpt`` workload, by turn config.

    The 450-conv cap is the default *only* for the 12/12 subset; every other
    prepared config (i.e. min-turns 2) defaults to the whole corpus, so lowering
    the turn threshold draws from all conversations rather than silently keeping
    the 450-conv cap. ``NUM_CONVS`` still overrides either way."""
    min_turns, max_turns = _sharegpt_turns()
    if min_turns == 12 and max_turns == 12:
        return 450
    return _SHAREGPT_CORPUS_CONVS


# name -> {dataset: path|callable, num_convs: int, desc: str}. A callable dataset
# is resolved at selection time (so per-run env like SHAREGPT_MIN_TURNS is
# honored) and may return None to defer to an explicit DATASET_PATH. Add entries
# here to register new workloads for all five drivers.
WORKLOADS = {
    "sharegpt": {
        # ShareGPT multi-turn workload, selected by human-turn count (see
        # _sharegpt_dataset): 12/12 = the baked 450-conv 12-turn subset; 1/1 =
        # the full 94,145-conv corpus dir. Both dataset and num_convs are
        # callables resolved from the turn config at selection time: the 450-conv
        # cap is the default only for 12/12, otherwise the default is the whole
        # corpus (see _sharegpt_num_convs). NUM_CONVS overrides either way.
        "dataset": _sharegpt_dataset,
        "num_convs": _sharegpt_num_convs,
        "desc": "ShareGPT multi-turn workload by human-turn count "
                "(SHAREGPT_MIN_TURNS/MAX_TURNS: 12/12 = 450-conv subset, "
                "1/1 = full 94,145-conv corpus)",
    },
}


def resolve_workload(default_dataset, default_num_convs):
    """Resolve ``(dataset_path, num_convs)`` for this run from the environment.

    Dataset-path precedence:
      1. ``DATASET_PATH`` — explicit ShareGPT-format json; always wins.
      2. ``WORKLOAD_NAME=<name>`` — a registered workload (see :data:`WORKLOADS`).
      3. the caller's ``default_dataset`` (each driver's historical default).

    ``NUM_CONVS``, when set, always overrides the count; otherwise a selected
    workload's ``num_convs`` applies, else ``default_num_convs``. This keeps every
    driver's prior default behavior intact when neither WORKLOAD_NAME nor the env
    overrides are set. Unknown ``WORKLOAD_NAME`` values exit with the known list.

    Note: the selector env is ``WORKLOAD_NAME``, not ``WORKLOAD`` — the bench
    container images already use ``WORKLOAD`` for the driver-script path their
    entrypoint execs, so the two must not collide."""
    dataset = default_dataset
    num_convs = default_num_convs
    explicit_path = os.environ.get("DATASET_PATH")

    workload = os.environ.get("WORKLOAD_NAME", "").strip().lower()
    # Turn bounds only mean anything for the sharegpt workload, so setting either
    # without WORKLOAD_NAME implies it — otherwise SHAREGPT_MIN_TURNS is silently
    # ignored and the driver keeps its baked 450x12 default (the "asked for the
    # corpus, still got 450" trap the shell orchestrators already guard against).
    # This also keeps the 450-conv default tied to the 12/12 config: any other
    # turn setting now routes through _sharegpt_num_convs (whole corpus) instead
    # of falling back to default_num_convs.
    if not workload and (
        os.environ.get("SHAREGPT_MIN_TURNS") or os.environ.get("SHAREGPT_MAX_TURNS")
    ):
        workload = "sharegpt"
    if workload:
        spec = WORKLOADS.get(workload)
        if spec is None:
            raise SystemExit(
                f"[run] unknown WORKLOAD_NAME={workload!r}; "
                f"known: {', '.join(sorted(WORKLOADS)) or '(none)'}"
            )
        # Both dataset and num_convs may be callables, resolved now so per-run
        # env like SHAREGPT_MIN_TURNS is honored (e.g. the default conv count
        # tracks the turn config: 450 for 12/12, whole corpus otherwise). The
        # dataset callable may return None to defer to an explicit DATASET_PATH
        # rather than erroring on an unprepared range.
        nc = spec.get("num_convs", default_num_convs)
        num_convs = nc(bool(explicit_path)) if callable(nc) else nc
        ds = spec["dataset"]
        resolved = ds(bool(explicit_path)) if callable(ds) else ds
        if resolved is not None:
            dataset = resolved

    if explicit_path:
        dataset = explicit_path

    num_convs_env = os.environ.get("NUM_CONVS")
    if num_convs_env is not None:
        num_convs = int(num_convs_env)

    return dataset, num_convs


# ── Workload input ────────────────────────────────────────────────────────
def load_convs(dataset_path, num_convs, conv_multiplier=1):
    """Load ShareGPT-format json and return a list of human-turn streams.

    ``dataset_path`` may be a single json file or a *directory* of json chunks
    (the full-corpus ``sharegpt`` min-turns-1 case): a directory is read as its
    ``*.json`` files in sorted (numeric-chunk) order, concatenated, so a chunked
    corpus replays as one continuous conversation stream.

    Each returned element is the list of ``human`` turn strings for one
    conversation (only conversations with >= 2 human turns are kept), capped at
    ``num_convs`` conversations — the cap short-circuits chunk reads, so a small
    ``num_convs`` never loads the whole corpus.

    ``conv_multiplier`` > 1 replicates the whole set N times for a larger
    concurrent working set; each replica's first turn is tagged ``[r{n}] `` so
    the accumulated context hashes distinctly per replica (otherwise
    byte-identical copies would dedup at the prefix-cache / KV-block layer and
    store no extra data). Replica order is (r0: all convs), (r1: all convs), …
    """
    if os.path.isdir(dataset_path):
        paths = sorted(glob.glob(os.path.join(dataset_path, "*.json")))
    else:
        paths = [dataset_path]

    convs = []
    for path in paths:
        if len(convs) >= num_convs:
            break
        with open(path) as f:
            all_data = json.load(f)
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
