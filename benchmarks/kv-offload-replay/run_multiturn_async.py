"""run_multiturn_async.py — the opt-in async per-conversation execution model.

The default, synchronous batched-round loop lives in
:mod:`run_multiturn_sync_batched` (``run_batched``); the shared setup every
driver uses (``build_engine``, ``make_n_tokens``, ``mfu_kwargs``,
``start_prom_exporter``, the telemetry helpers) lives in
:mod:`run_multiturn_common`. This module is the *other* execution model — one
vLLM coroutine per conversation on a V1 ``AsyncLLM`` — kept in its own file so
the default path stays lean and the async orchestration lands in exactly one
place instead of being copy-pasted into each backend driver's
``WORKLOAD_MODE=async`` branch.

A driver opts in with a single call to :func:`run_async_driver`, handing it the
same ``engine_kwargs`` dict it would pass to the batched path plus its
backend-specific ``disk_rw_bytes`` closure and summary fields. Everything
async-specific — building the ``AsyncLLM``, the 1 Hz disk+Prometheus sampler,
``asyncio.run``, the per-turn latency percentiles, and the summary shape — is
here. The low-level :func:`run_async` loop is exposed too for drivers that need
finer control (e.g. a per-conversation ``session_id_fn``).

Like :mod:`run_multiturn_common`, nothing here imports vllm at module load — the
engine import happens lazily inside ``run_multiturn_common.build_engine``.
"""

import os
import sys
import time

import run_multiturn_common as common


def _prom_key(name):
    """Normalise a vLLM counter name to the renderer's curated key: drop the
    ``vllm:`` prefix and a single trailing ``_total``.

    Under a V1 ``AsyncLLM`` there is no ``get_metrics()``, so the sampler reads
    the global prometheus REGISTRY, whose counter names are the cumulative
    ``vllm:*_total`` form only (the sync path additionally gets bare names from
    ``get_metrics()``). ``tools/render_kvprofile.py`` matches its curated
    ``COUNTERS`` keys *exactly* — bare, no ``_total`` — so both must be stripped
    for an async ``[prom]`` line to plot (e.g. ``vllm:prompt_tokens_total`` ->
    ``prompt_tokens``, ``vllm:kv_offload_store_bytes_total`` ->
    ``kv_offload_store_bytes``)."""
    if name.startswith("vllm:"):
        name = name[len("vllm:"):]
    if name.endswith("_total"):
        name = name[: -len("_total")]
    return name


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
                    sampler=None, sample_hz=1.0, on_turn_end=None,
                    progress_interval=0.0, active_sessions=0):
    """Drive the same multi-turn workload as one coroutine per conversation.

    Within a coroutine a conversation's turns run sequentially (each turn's
    prompt is the running context + the next human turn, exactly as
    ``run_multiturn_sync_batched.run_batched`` builds it).

    Two admission models, selected by ``active_sessions``:

    * ``active_sessions=0`` (default) — **open loop**: every conversation is
      launched at once and vLLM's ``max_num_seqs`` bounds how many run
      concurrently, the rest queue in WAITING. The max-concurrency analogue of
      the batched rounds; unchanged historical behavior.
    * ``active_sessions=N`` (> 0) — **closed loop**: keep exactly ``N``
      conversations active at a time (a fixed pool of ``N`` workers pulls the
      next conversation from the backlog whenever one finishes all its turns),
      so a new session is admitted only as a running one retires — steady-state
      concurrency rather than a load-everything-up-front burst. ``N`` should be
      ``<= max_num_seqs`` so the driver, not the engine queue, is the gate.

    Each turn issues a unique ``request_id=f"{conv}:{turn}"`` and consumes
    ``engine.generate`` to the finished output, recording a turn record
    ``(conv, turn, prompt_toks, gen_toks, ttft, latency)``. When ``sampler`` is
    given, a concurrent task calls it every ``1/sample_hz`` seconds and appends
    ``(t, sampler())`` — the async analogue of the batched per-round snapshot
    (``AsyncLLM`` has no ``get_metrics()``, so a sampler typically reads the
    global prometheus REGISTRY via ``run_multiturn_common.prom_counters``).

    ``max_rounds`` here is a per-conversation turn cap (0 = all turns).

    ``progress_interval`` (seconds, 0 = off) prints a periodic heartbeat to
    stderr while the run is in flight — conversations finished vs total,
    generations so far, throughput, and the spread of turn indices currently
    being worked — so an async run isn't silent between launch and completion.

    Returns a dict with ``rounds_done`` (max turns reached by any conv),
    ``total_generations``, ``elapsed``, ``turn_records`` and ``samples``.
    """
    import asyncio

    turn_records = []  # (conv, turn, prompt_toks, gen_toks, ttft, latency)
    samples = []       # (t, sampler_value)
    # Live progress state (read by the heartbeat, mutated by each coroutine).
    n_convs = len(convs)
    cur_turn = [-1] * n_convs   # turn index each conv is currently working (-1 = not started)
    conv_done = [False] * n_convs

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
            cur_turn[i] = k
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
        conv_done[i] = True

    async def sample_loop():
        while True:
            await asyncio.sleep(1.0 / sample_hz)
            try:
                samples.append((time.perf_counter(), sampler()))
            except Exception:  # noqa: BLE001 - a sampler hiccup must not kill the run
                pass

    async def progress_loop():
        # Heartbeat: convs finished vs total, generations so far, gen/s, and the
        # min/median/max turn index among conversations still in flight (which
        # shows the async concurrency — turns advance at different rates).
        while True:
            await asyncio.sleep(progress_interval)
            n_done = sum(conv_done)
            n_gen = len(turn_records)
            el = time.perf_counter() - t_start
            rate = n_gen / el if el > 0 else 0.0
            active = sorted(cur_turn[i] for i in range(n_convs)
                            if not conv_done[i] and cur_turn[i] >= 0)
            if active:
                spread = (f"  turns-in-flight min={active[0]} "
                          f"med={active[len(active) // 2]} max={active[-1]}")
            else:
                spread = ""
            print(f"[run] async progress: {n_done}/{n_convs} convs done  "
                  f"{n_gen} generations  {rate:.1f} gen/s  "
                  f"{len(active)} in flight{spread}",
                  file=sys.stderr, flush=True)

    async def worker(pull):
        """Closed-loop worker: run conversations to completion, one after the
        next, until the shared backlog is drained. ``pull`` hands back the next
        (index, conv) or None. Keeping a fixed number of these coroutines alive
        holds the active-session count constant."""
        while True:
            item = pull()
            if item is None:
                return
            i, conv = item
            await run_conv(i, conv)

    t_start = time.perf_counter()
    sampler_task = asyncio.create_task(sample_loop()) if sampler else None
    progress_task = (asyncio.create_task(progress_loop())
                     if progress_interval and progress_interval > 0 else None)
    try:
        if active_sessions < 0:
            raise ValueError(f"active_sessions must be >= 0, got {active_sessions}")
        if active_sessions > 0:
            # Closed loop: a fixed pool of `active_sessions` workers. No await
            # between reading and advancing `nxt`, so a plain counter is safe
            # under the single-threaded event loop (no lock needed).
            nxt = 0
            n_workers = min(active_sessions, len(convs))

            def pull():
                nonlocal nxt
                if nxt >= len(convs):
                    return None
                i = nxt
                nxt += 1
                return i, convs[i]

            print(f"[run] async closed loop: {n_workers} active sessions over "
                  f"{len(convs)} conversations (admit-on-finish)",
                  file=sys.stderr, flush=True)
            await asyncio.gather(*(worker(pull) for _ in range(n_workers)))
        else:
            await asyncio.gather(*(run_conv(i, c) for i, c in enumerate(convs)))
    finally:
        for task in (sampler_task, progress_task):
            if task is not None:
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass

    elapsed = time.perf_counter() - t_start
    return {
        "rounds_done": max((r[1] + 1 for r in turn_records), default=0),
        "total_generations": len(turn_records),
        "elapsed": elapsed,
        "turn_records": turn_records,
        "samples": samples,
        "active_sessions": (min(active_sessions, len(convs))
                            if active_sessions and active_sessions > 0
                            else len(convs)),
    }


def run_async_driver(engine_kwargs, convs, sampling_params, *, prompt_budget,
                     max_rounds, capture_metrics=True, disk_rw_bytes=None,
                     session_id_fn=None, skip_empty=False, summary_base=None,
                     n_tokens_flavor="input_ids", active_sessions=0):
    """Run the async execution model end-to-end and return a summary dict.

    This is the single entry point a backend driver's ``WORKLOAD_MODE=async``
    branch calls. It builds a V1 ``AsyncLLM`` from ``engine_kwargs`` (via
    ``run_multiturn_common.build_engine(..., async_mode=True)`` — the same kwargs
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
    n_tokens_flavor : str
        ``"input_ids"`` (default) or ``"encode"`` — the driver's tokenizer-length
        flavor, forwarded to ``make_n_tokens`` so the async path counts prompt
        tokens exactly as the batched path did (the fs_bench drivers use encode).
    summary_base : dict | None
        Backend fields merged into the returned summary (model, tier, …).

    Returns the summary dict (``summary_base`` + ``elapsed_time``, ``num_rounds``,
    ``total_generations``, ``mode="async"``, latency/ttft percentiles, ``samples``).
    """
    import asyncio

    engine = common.build_engine(engine_kwargs, async_mode=True)
    common.start_prom_exporter()
    # Progress heartbeat cadence in seconds (ASYNC_PROGRESS_SECS, default 10;
    # 0 disables) — keeps the async run from being silent between launch and the
    # final latency line.
    try:
        progress_interval = float(os.environ.get("ASYNC_PROGRESS_SECS", "10"))
    except ValueError:
        progress_interval = 10.0
    _cadence = (f", progress every {progress_interval:g}s"
                if progress_interval > 0 else "")
    if active_sessions and active_sessions > 0:
        print(f"[run] WORKLOAD_MODE=async — closed loop, {active_sessions} "
              f"active sessions (driver admits a new conversation on finish); "
              f"{len(convs)} conversations{_cadence}",
              file=sys.stderr, flush=True)
    else:
        print("[run] WORKLOAD_MODE=async — one coroutine per conversation "
              f"(max_num_seqs bounds the running batch); "
              f"{len(convs)} conversations{_cadence}",
              file=sys.stderr, flush=True)

    def _disk():
        return disk_rw_bytes() if disk_rw_bytes is not None else (None, None)

    # 1-second sampler. AsyncLLM has no get_metrics(), so prom_counters reads the
    # global prometheus REGISTRY (populated when disable_log_stats is False);
    # pair it with per-device disk bytes so the buckets mirror the batched
    # per-round disk+counter deltas.
    def sampler():
        rd, wr = _disk()
        return {"prom": common.prom_counters(engine, capture_metrics),
                "read_bytes": rd, "write_bytes": wr}

    async def _amain():
        tokenizer = await get_tokenizer(engine)
        n_tokens = common.make_n_tokens(tokenizer, n_tokens_flavor)
        return await run_async(
            engine, convs, sampling_params,
            prompt_budget=prompt_budget,
            max_rounds=max_rounds,
            n_tokens=n_tokens,
            skip_empty=skip_empty,
            session_id_fn=session_id_fn,
            sampler=sampler if capture_metrics else None,
            progress_interval=progress_interval,
            active_sessions=active_sessions,
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

    # ── Per-tick [prom] markers (renderer input) ──────────────────────────────
    # The 1 Hz sampler recorded cumulative counters (+ SSD device bytes, when the
    # driver supplied a disk_rw_bytes closure) at each tick. Emit the per-tick
    # DELTAS in the same "[prom] round N: k=v …" form the batched path prints, so
    # tools/render_kvprofile.py plots async runs exactly like batched ones (here a
    # "round" is one 1 s telemetry tick). Counter names are normalised to the
    # renderer's curated keys via _prom_key (strip vllm:/_total). Without this an
    # async run emitted only the single cumulative line below, which the renderer's
    # per-round parser ignores — so async slides carried no vLLM-counter panels.
    if capture_metrics:
        prev_prom = None
        prev_rd = prev_wr = None
        for tick, (_t, v) in enumerate(result["samples"]):
            pr = v.get("prom") or {}
            parts = []
            if prev_prom is not None:
                d = {}
                for k in pr:
                    delta = pr.get(k, 0.0) - prev_prom.get(k, 0.0)
                    if delta:
                        d[_prom_key(k)] = delta
                if d:
                    parts.append(" ".join(f"{k}={d[k]:.0f}" for k in sorted(d)))
            rd, wr = v.get("read_bytes"), v.get("write_bytes")
            if (rd is not None and wr is not None
                    and prev_rd is not None and prev_wr is not None):
                parts.append(f"ssd_read_bytes={rd - prev_rd} "
                             f"ssd_write_bytes={wr - prev_wr}")
            line = " ".join(p for p in parts if p)
            if line:
                print(f"[prom] round {tick}: {line}", file=sys.stderr, flush=True)
            prev_prom, prev_rd, prev_wr = pr, rd, wr

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
        "active_sessions": result.get("active_sessions"),
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
