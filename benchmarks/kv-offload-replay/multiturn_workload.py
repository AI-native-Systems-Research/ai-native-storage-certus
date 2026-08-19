"""multiturn_workload.py — the shared multi-turn ShareGPT replay workload.

This module owns the *workload* — dataset loading, per-conversation turn
assembly, the prompt-budget / empty-prompt guards, and the generation loop —
so the five backend drivers
(``run_multiturn_nooffload.py``, ``run_multiturn_offloading.py``,
``run_fs_bench_450.py``, ``run_fs_bench_450_iostat.py`` and
``certus-shmq-connector/run_multiturn_shmq_certus.py``) stop copy-pasting it.

The *backend* — how vLLM is constructed (``kv_transfer_config``, engine kwargs)
and what telemetry each run captures — stays in the driver. The driver hands the
loop its ``LLM``, its ``SamplingParams``, an ``n_tokens`` callable, and optional
per-round callbacks; the loop drives the conversation state machine and calls
back at the points where a driver wants to snapshot / print.

Behavior is intentionally identical to the pre-extraction inline loops. The two
places the drivers differed are parameters here:

* ``skip_empty`` — nooffload and the shmq driver drop a turn whose prompt
  tokenizes to 0 tokens (granite renders some ShareGPT turns empty, which aborts
  the vLLM engine); the offload/SharedStorage drivers did not have that guard.
* ``session_id_fn`` — the shmq driver tags every request with a per-conversation
  KV-offload ``session_id`` via a cloned ``SamplingParams``; the others pass one
  shared ``SamplingParams``.

Nothing here imports vllm, so the driver stays in control of when the (heavy)
engine import happens.
"""

import json
import sys
import time


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


# ── The batched round loop (current, synchronous behavior) ──────────────────
def run_batched(llm, convs, sampling_params, *, prompt_budget, max_rounds,
                n_tokens, skip_empty=False, session_id_fn=None,
                on_round_start=None, on_round_end=None):
    """Drive the multi-turn workload as synchronous batched rounds.

    Round k submits, for every still-alive conversation, the cumulative prompt
    (all prior human turns + all prior vLLM responses + the k'th human turn) in
    one ``llm.generate`` batch, then appends each response for the next round.

    Parameters
    ----------
    llm : vllm.LLM
        Constructed by the driver with its backend ``kv_transfer_config``.
    convs : list[list[str]]
        Human-turn streams from :func:`load_convs`.
    sampling_params : vllm.SamplingParams
        Base sampling params. When ``session_id_fn`` is given it is
        ``.clone()``d per request; otherwise the same object is reused.
    prompt_budget : int
        Max prompt tokens (``max_model_len - output_tokens``); a conversation
        whose next prompt exceeds this is dropped from further rounds.
    max_rounds : int
        Cap on rounds (0 = run until conversations are exhausted).
    n_tokens : Callable[[str], int]
        Token-length function (the driver owns tokenizer choice).
    skip_empty : bool
        Also drop a turn whose prompt tokenizes to 0 tokens.
    session_id_fn : Callable[[int], int] | None
        If given, each request gets ``sampling_params.clone()`` with
        ``extra_args={"kv_transfer_params": {"session_id": session_id_fn(i)}}``
        where ``i`` is the conversation index.
    on_round_start : Callable[[int, int], None] | None
        Called after the round's batch is assembled, before ``generate``, as
        ``on_round_start(round_idx, n_prompts)``. Use to snapshot pre-generate
        counters (disk bytes, etc.).
    on_round_end : Callable[[int, int, float, int], None] | None
        Called after outputs are folded back in, as
        ``on_round_end(round_idx, n_prompts, round_elapsed, n_alive)``. Use to
        snapshot post-generate counters, print the ``[run] round N:`` line, and
        record per-round telemetry.

    Returns
    -------
    dict with ``rounds_done``, ``total_generations`` and ``elapsed`` (loop-only
    wall time, excluding model load).
    """
    n = len(convs)
    alive = [True] * n
    next_turn = [0] * n
    contexts = [""] * n

    rounds_done = 0
    total_generations = 0
    t_start = time.perf_counter()

    while True:
        if max_rounds and rounds_done >= max_rounds:
            break

        active_idx = []
        active_prompts = []
        active_sps = [] if session_id_fn is not None else None
        for i, conv in enumerate(convs):
            if not alive[i]:
                continue
            k = next_turn[i]
            if k >= len(conv):
                alive[i] = False
                continue
            human = conv[k]
            candidate = human if k == 0 else contexts[i] + "\n\n" + human
            nt = n_tokens(candidate)
            if (skip_empty and nt == 0) or nt > prompt_budget:
                alive[i] = False
                continue
            contexts[i] = candidate
            active_idx.append(i)
            active_prompts.append(candidate)
            if session_id_fn is not None:
                # Tag each request with its conversation as the KV-offload
                # session_id. The conversation index is stable across rounds, so
                # every turn of the same conversation shares one session_id.
                sp_i = sampling_params.clone()
                sp_i.extra_args = {
                    "kv_transfer_params": {"session_id": session_id_fn(i)}
                }
                active_sps.append(sp_i)

        if not active_prompts:
            break

        rounds_done += 1
        if on_round_start is not None:
            on_round_start(rounds_done, len(active_prompts))

        round_start = time.perf_counter()
        outs = llm.generate(
            active_prompts,
            active_sps if session_id_fn is not None else sampling_params,
        )
        round_elapsed = time.perf_counter() - round_start

        for i, out in zip(active_idx, outs):
            response = out.outputs[0].text if out.outputs else ""
            contexts[i] = contexts[i] + response
            next_turn[i] += 1
        total_generations += len(active_prompts)
        n_alive = sum(alive)

        if on_round_end is not None:
            on_round_end(rounds_done, len(active_prompts), round_elapsed, n_alive)

    elapsed = time.perf_counter() - t_start
    return {
        "rounds_done": rounds_done,
        "total_generations": total_generations,
        "elapsed": elapsed,
    }


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
# Prometheus exporter, and the tokenizer-length closure. Keeping them here means
# the async path (build_engine(..., async_mode=True) + run_async) reuses exactly
# the same setup instead of forking a second copy.
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


async def get_tokenizer(engine):
    """Return the engine's tokenizer, awaiting if the accessor is a coroutine.

    ``LLM.get_tokenizer()`` is synchronous; a V1 ``AsyncLLM`` may expose it as a
    coroutine. This normalizes both so the async driver path can build its
    ``n_tokens`` closure uniformly."""
    import inspect
    tok = engine.get_tokenizer()
    if inspect.isawaitable(tok):
        tok = await tok
    return tok


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


# ── The async per-conversation loop (opt-in; batched stays the default) ──────
async def run_async(engine, convs, sampling_params, *, prompt_budget, max_rounds,
                    n_tokens, skip_empty=False, session_id_fn=None,
                    sampler=None, sample_hz=1.0, on_turn_end=None):
    """Drive the same multi-turn workload as one coroutine per conversation.

    Every conversation is launched at once; within a coroutine its turns run
    sequentially (each turn's prompt is the running context + the next human
    turn, exactly as :func:`run_batched` builds it). vLLM's ``max_num_seqs``
    bounds how many run concurrently — the rest queue in WAITING — so this is the
    max-concurrency analogue of the batched rounds, not a behavioral change to
    the workload itself.

    Each turn issues a unique ``request_id=f"{conv}:{turn}"`` and consumes
    ``engine.generate`` to the finished output, recording a turn record
    ``(conv, turn, prompt_toks, gen_toks, ttft, latency)``. When ``sampler`` is
    given, a concurrent task calls it every ``1/sample_hz`` seconds and appends
    ``(t, sampler())`` — the async analogue of the batched per-round snapshot
    (``AsyncLLM`` has no ``get_metrics()``, so a sampler typically reads the
    global prometheus REGISTRY via :func:`prom_counters`).

    ``max_rounds`` here is a per-conversation turn cap (0 = all turns).

    Returns a dict with ``rounds_done`` (max turns reached by any conv),
    ``total_generations``, ``elapsed``, ``turn_records`` and ``samples``.
    """
    import asyncio

    turn_records = []  # (conv, turn, prompt_toks, gen_toks, ttft, latency)
    samples = []       # (t, sampler_value)

    async def run_conv(i, conv):
        context = ""
        if session_id_fn is not None:
            sp = sampling_params.clone()
            sp.extra_args = {
                "kv_transfer_params": {"session_id": session_id_fn(i)}
            }
        else:
            sp = sampling_params
        n_turns = min(len(conv), max_rounds) if max_rounds else len(conv)
        for k in range(n_turns):
            human = conv[k]
            candidate = human if k == 0 else context + "\n\n" + human
            nt = n_tokens(candidate)
            if (skip_empty and nt == 0) or nt > prompt_budget:
                break
            context = candidate
            t0 = time.perf_counter()
            ttft = None
            final = None
            async for out in engine.generate(candidate, sp, f"{i}:{k}"):
                if ttft is None:
                    ttft = time.perf_counter() - t0
                final = out
                if out.finished:
                    break
            latency = time.perf_counter() - t0
            if final and final.outputs:
                response = final.outputs[0].text
                gen_toks = len(final.outputs[0].token_ids)
            else:
                response, gen_toks = "", 0
            context = context + response
            turn_records.append((i, k, nt, gen_toks, ttft, latency))
            if on_turn_end is not None:
                on_turn_end(i, k, nt, gen_toks, ttft, latency)

    async def sample_loop():
        while True:
            await asyncio.sleep(1.0 / sample_hz)
            try:
                samples.append((time.perf_counter(), sampler()))
            except Exception:  # noqa: BLE001 - a sampler hiccup must not kill the run
                pass

    t_start = time.perf_counter()
    sampler_task = asyncio.create_task(sample_loop()) if sampler else None
    try:
        await asyncio.gather(*(run_conv(i, c) for i, c in enumerate(convs)))
    finally:
        if sampler_task is not None:
            sampler_task.cancel()
            try:
                await sampler_task
            except asyncio.CancelledError:
                pass

    elapsed = time.perf_counter() - t_start
    return {
        "rounds_done": max((r[1] + 1 for r in turn_records), default=0),
        "total_generations": len(turn_records),
        "elapsed": elapsed,
        "turn_records": turn_records,
        "samples": samples,
    }
