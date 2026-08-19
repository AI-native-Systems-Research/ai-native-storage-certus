"""multiturn_async.py — the opt-in async per-conversation execution model.

The default, synchronous batched-round workload lives in
:mod:`multiturn_workload` (``run_batched``) alongside the shared setup every
driver uses (``build_engine``, ``make_n_tokens``, ``mfu_kwargs``,
``start_prom_exporter``, the telemetry helpers). This module is the *other*
execution model — one vLLM coroutine per conversation on a V1 ``AsyncLLM`` — kept
in its own file so the default path stays lean and the async orchestration lands
in exactly one place instead of being copy-pasted into each backend driver's
``WORKLOAD_MODE=async`` branch.

A driver opts in with a single call to :func:`run_async_driver`, handing it the
same ``engine_kwargs`` dict it would pass to the batched path plus its
backend-specific ``disk_rw_bytes`` closure and summary fields. Everything
async-specific — building the ``AsyncLLM``, the 1 Hz disk+Prometheus sampler,
``asyncio.run``, the per-turn latency percentiles, and the summary shape — is
here. The low-level :func:`run_async` loop is exposed too for drivers that need
finer control (e.g. a per-conversation ``session_id_fn``).

Like :mod:`multiturn_workload`, nothing here imports vllm at module load — the
engine import happens lazily inside ``multiturn_workload.build_engine``.
"""

import sys
import time

import multiturn_workload as mw


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


async def run_async(engine, convs, sampling_params, *, prompt_budget, max_rounds,
                    n_tokens, skip_empty=False, session_id_fn=None,
                    sampler=None, sample_hz=1.0, on_turn_end=None):
    """Drive the same multi-turn workload as one coroutine per conversation.

    Every conversation is launched at once; within a coroutine its turns run
    sequentially (each turn's prompt is the running context + the next human
    turn, exactly as ``multiturn_workload.run_batched`` builds it). vLLM's
    ``max_num_seqs`` bounds how many run concurrently — the rest queue in WAITING
    — so this is the max-concurrency analogue of the batched rounds, not a
    behavioral change to the workload itself.

    Each turn issues a unique ``request_id=f"{conv}:{turn}"`` and consumes
    ``engine.generate`` to the finished output, recording a turn record
    ``(conv, turn, prompt_toks, gen_toks, ttft, latency)``. When ``sampler`` is
    given, a concurrent task calls it every ``1/sample_hz`` seconds and appends
    ``(t, sampler())`` — the async analogue of the batched per-round snapshot
    (``AsyncLLM`` has no ``get_metrics()``, so a sampler typically reads the
    global prometheus REGISTRY via ``multiturn_workload.prom_counters``).

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


def run_async_driver(engine_kwargs, convs, sampling_params, *, prompt_budget,
                     max_rounds, capture_metrics=True, disk_rw_bytes=None,
                     session_id_fn=None, skip_empty=False, summary_base=None):
    """Run the async execution model end-to-end and return a summary dict.

    This is the single entry point a backend driver's ``WORKLOAD_MODE=async``
    branch calls. It builds a V1 ``AsyncLLM`` from ``engine_kwargs`` (via
    ``multiturn_workload.build_engine(..., async_mode=True)`` — the same kwargs
    the batched path uses, so the backend config is not duplicated), starts the
    optional Prometheus exporter, replays ``convs`` through :func:`run_async`
    with a 1 Hz disk+counter sampler, then folds the per-turn latency / TTFT
    percentiles and the sampler buckets into ``summary_base``.

    Parameters
    ----------
    engine_kwargs : dict
        Fully-assembled engine kwargs (``kv_transfer_config`` etc.), identical to
        the batched path's.
    disk_rw_bytes : Callable[[], tuple] | None
        The driver's ``() -> (read_bytes, write_bytes)`` closure (each driver
        reads its own block device); ``(None, None)`` when omitted.
    capture_metrics : bool
        When False, no sampler runs (matches the batched CAPTURE_METRICS=0
        stats-off baseline) and ``prom_counters`` is not read.
    session_id_fn : Callable[[int], int] | None
        Per-conversation KV-offload ``session_id`` tagging (shmq driver).
    summary_base : dict | None
        Backend fields merged into the returned summary (model, tier, …).

    Returns the summary dict (``summary_base`` + ``elapsed_time``, ``num_rounds``,
    ``total_generations``, ``mode="async"``, latency/ttft percentiles, ``samples``).
    """
    import asyncio

    engine = mw.build_engine(engine_kwargs, async_mode=True)
    mw.start_prom_exporter()
    print("[run] WORKLOAD_MODE=async — one coroutine per conversation "
          "(max_num_seqs bounds the running batch)", file=sys.stderr)

    def _disk():
        return disk_rw_bytes() if disk_rw_bytes is not None else (None, None)

    # 1-second sampler. AsyncLLM has no get_metrics(), so prom_counters reads the
    # global prometheus REGISTRY (populated when disable_log_stats is False);
    # pair it with per-device disk bytes so the buckets mirror the batched
    # per-round disk+counter deltas.
    def sampler():
        rd, wr = _disk()
        return {"prom": mw.prom_counters(engine, capture_metrics),
                "read_bytes": rd, "write_bytes": wr}

    async def _amain():
        tokenizer = await get_tokenizer(engine)
        n_tokens = mw.make_n_tokens(tokenizer)
        return await run_async(
            engine, convs, sampling_params,
            prompt_budget=prompt_budget,
            max_rounds=max_rounds,
            n_tokens=n_tokens,
            skip_empty=skip_empty,
            session_id_fn=session_id_fn,
            sampler=sampler if capture_metrics else None,
        )

    result = asyncio.run(_amain())

    # Per-turn latency / TTFT percentiles from the turn records.
    lat = sorted(r[5] for r in result["turn_records"])
    ttfts = sorted(r[4] for r in result["turn_records"] if r[4] is not None)

    def _pct(vals, p):
        if not vals:
            return None
        return vals[min(len(vals) - 1, int(p * len(vals)))]

    print(f"[run] async turn latency: n={len(lat)}  "
          f"p50={_pct(lat, 0.50)} p90={_pct(lat, 0.90)} p99={_pct(lat, 0.99)}  "
          f"ttft_p50={_pct(ttfts, 0.50)}", file=sys.stderr, flush=True)

    # vLLM counter movement across the run (first sample -> last). AsyncLLM has
    # no get_metrics(), so the sampler read the global REGISTRY; the batched path
    # prints per-round deltas, this prints the whole-run delta. If the REGISTRY
    # stayed empty the run is aggregate-only — timing/percentiles are still valid.
    proms = [v["prom"] for _, v in result["samples"] if v.get("prom")]
    counter_movement = {}
    if proms:
        first, last = proms[0], proms[-1]
        counter_movement = {k: last.get(k, 0.0) - first.get(k, 0.0)
                            for k in last if last.get(k, 0.0) - first.get(k, 0.0)}
    if capture_metrics and not proms:
        print("[prom] async: REGISTRY empty under AsyncLLM — aggregate-only "
              "(timing/percentiles valid, no vllm: counters)",
              file=sys.stderr, flush=True)
    elif counter_movement:
        shown = " ".join(f"{k[len('vllm:'):] if k.startswith('vllm:') else k}"
                         f"={counter_movement[k]:.0f}"
                         for k in sorted(counter_movement))
        print(f"[prom] async counter movement: {shown}", file=sys.stderr, flush=True)

    summary = dict(summary_base or {})
    summary.update({
        "elapsed_time": result["elapsed"],
        "num_conversations": len(convs),
        "num_rounds": result["rounds_done"],
        "total_generations": result["total_generations"],
        "mode": "async",
        "turn_latency_p50": _pct(lat, 0.50),
        "turn_latency_p90": _pct(lat, 0.90),
        "turn_latency_p99": _pct(lat, 0.99),
        "ttft_p50": _pct(ttfts, 0.50),
        "counter_movement": counter_movement,
        "samples": [
            {"t": t, "read_bytes": v["read_bytes"], "write_bytes": v["write_bytes"]}
            for t, v in result["samples"]
        ],
    })
    return summary
