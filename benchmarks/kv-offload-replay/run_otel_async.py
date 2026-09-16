"""run_otel_async.py — timestamp-scheduled async execution model for the
kv-offload-otel-replay tool.

This is the OTel-corpus analogue of :mod:`run_multiturn_async`. Where that module
replays ShareGPT human-turn streams as fast as the engine will take them (one
coroutine per conversation, turns issued back-to-back), this one replays the
``otel_trace_replay`` corpus produced by ``benchmarks/kv-offload-otel-replay/trace-gen/trace_to_otel.py`` and
**honors the recorded per-turn timing**: within a conversation coroutine, before
issuing turn ``k`` it sleeps the recorded inter-turn gap
``start_time[k] - end_time[k-1]`` (the tool-call / think delay the trace
encodes), so the wall-clock spacing of turns reproduces the workload's real
turn intervals rather than a saturating burst.

Two more OTel-specific differences from :mod:`run_multiturn_async`:

* **Per-turn ``max_tokens``.** Each span carries ``gen_ai.request.max_tokens``;
  the turn is generated to that budget (optionally clamped by ``max_output_tokens``)
  instead of one fixed ``OUTPUT_TOKENS`` for every turn, so generated sizes track
  the recorded distribution.
* **Sliding-window context.** ``conversation_replay`` accumulates context with no
  compaction (deep turns reach ~289k tokens), and the corpus was written with a
  120k sliding window. We reproduce that here on the *reconstructed* context
  (prior human turns + vLLM's own generations): before each turn we drop the
  oldest (human, response) pairs until the assembled prompt fits ``context_cap``
  (and ``prompt_budget``). Using vLLM's own outputs — not the recorded assistant
  markers — keeps prefix tokens matching turn-to-turn so the offload cache sees
  real read-path traffic, exactly as the ShareGPT drivers do.

Like :mod:`run_multiturn_async` it imports nothing from vllm at module load (the
engine import happens lazily inside :func:`run_multiturn_common.build_engine`),
and it reuses that module's engine/tokenizer/telemetry helpers plus
:mod:`run_multiturn_async`'s ``_prom_key`` / ``get_tokenizer`` so the summary and
``[prom]`` markers are byte-compatible with the ShareGPT async path (same
renderer input).
"""

import os
import sys
import time
from collections import deque

import run_multiturn_common as common
import run_multiturn_async as mta  # reuse _prom_key / get_tokenizer


def _assemble(history, human):
    """Reconstruct the running context string from the kept (human, response)
    pairs plus the current human turn, joined the same way the ShareGPT drivers
    accumulate context (``"\\n\\n"`` between segments)."""
    parts = []
    for h, r in history:
        parts.append(h)
        if r:
            parts.append(r)
    parts.append(human)
    return "\n\n".join(parts)


async def run_otel(engine, convs, base_sp, *, prompt_budget, context_cap,
                   n_tokens, time_scale=1.0, max_output_tokens=None,
                   sampler=None, sample_hz=1.0, progress_interval=0.0,
                   active_sessions=0, session_id_fn=None):
    """Replay the OTel corpus as one coroutine per conversation, honoring the
    recorded inter-turn delays.

    ``convs`` is a list of conversations; each conversation is a list of turn
    tuples ``(human_text, max_tokens, delay_before_sec)`` in trace order
    (``delay_before_sec`` is 0 for the first turn and ``start[k] - end[k-1]`` for
    later turns). Within a coroutine turns run sequentially; before turn ``k>0``
    the coroutine sleeps ``delay_before_sec * time_scale`` seconds so the spacing
    reproduces the trace (``time_scale<1`` compresses a very long recorded
    schedule; ``time_scale=0`` disables the waits for a saturating run).

    The context is rebuilt from prior human turns + vLLM's own generations under
    a sliding window (drop oldest pairs until the assembled prompt fits
    ``min(context_cap, prompt_budget)``); a lone turn that still overflows retires
    the conversation (matching the ShareGPT drivers' ``nt > prompt_budget``
    guard). Each turn is generated to its recorded ``max_tokens`` (clamped by
    ``max_output_tokens`` when given).

    Admission mirrors :func:`run_multiturn_async.run_async`: ``active_sessions=0``
    launches every conversation at once (open loop, ``max_num_seqs`` bounds the
    running batch); ``active_sessions=N>0`` keeps N conversations active
    (closed loop, admit-on-finish). Returns the same result dict shape.
    """
    import asyncio

    budget = min(prompt_budget, context_cap) if context_cap else prompt_budget
    turn_records = []  # (conv, turn, prompt_toks, gen_toks, ttft, latency)
    samples = []       # (t, sampler_value)
    n_convs = len(convs)
    cur_turn = [-1] * n_convs
    conv_done = [False] * n_convs
    slept_total = [0.0]  # aggregate recorded delay actually applied (diagnostics)

    async def run_conv(i, conv):
        history = deque()  # (human, response) — vLLM's own outputs
        if session_id_fn is not None:
            sp_base = base_sp.clone()
            sp_base.extra_args = {"kv_transfer_params": {"session_id": session_id_fn(i)}}
        else:
            sp_base = base_sp
        for k, (human, max_tok, delay) in enumerate(conv):
            if k > 0 and delay > 0 and time_scale > 0:
                await asyncio.sleep(delay * time_scale)
                slept_total[0] += delay * time_scale

            # Sliding window on the reconstructed context.
            prompt = _assemble(history, human)
            while history and n_tokens(prompt) > budget:
                history.popleft()
                prompt = _assemble(history, human)
            nt = n_tokens(prompt)
            if not history and nt > budget:
                break  # a single turn overflows the window — retire the conv

            cur_turn[i] = k
            sp = sp_base.clone()
            sp.max_tokens = (min(max_tok, max_output_tokens)
                             if max_output_tokens else max_tok)

            t0 = time.perf_counter()
            ttft = None
            final = None
            async for out in engine.generate(prompt, sp, f"{i}:{k}"):
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
            history.append((human, response))
            turn_records.append((i, k, nt, gen_toks, ttft, latency))
        conv_done[i] = True

    async def sample_loop():
        while True:
            await asyncio.sleep(1.0 / sample_hz)
            try:
                samples.append((time.perf_counter(), sampler()))
            except Exception:  # noqa: BLE001 - a sampler hiccup must not kill the run
                pass

    async def progress_loop():
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
            print(f"[run] otel progress: {n_done}/{n_convs} convs done  "
                  f"{n_gen} generations  {rate:.1f} gen/s  "
                  f"{len(active)} in flight  slept={slept_total[0]:.0f}s{spread}",
                  file=sys.stderr, flush=True)

    async def worker(pull):
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
            nxt = 0
            n_workers = min(active_sessions, len(convs))

            def pull():
                nonlocal nxt
                if nxt >= len(convs):
                    return None
                i = nxt
                nxt += 1
                return i, convs[i]

            print(f"[run] otel closed loop: {n_workers} active sessions over "
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
        "recorded_delay_applied": slept_total[0],
        "turn_records": turn_records,
        "samples": samples,
        "active_sessions": (min(active_sessions, len(convs))
                            if active_sessions and active_sessions > 0
                            else len(convs)),
    }


def run_otel_driver(engine_kwargs, convs, base_sp, *, prompt_budget, context_cap,
                    time_scale=1.0, max_output_tokens=None, capture_metrics=True,
                    disk_rw_bytes=None, session_id_fn=None,
                    n_tokens_flavor="input_ids", active_sessions=0,
                    summary_base=None):
    """Run the timestamp-scheduled OTel replay end-to-end and return a summary.

    The OTel analogue of :func:`run_multiturn_async.run_async_driver`: builds a V1
    ``AsyncLLM`` from ``engine_kwargs`` (same kwargs the ShareGPT drivers use, so
    the backend config is not duplicated), starts the optional Prometheus
    exporter, replays ``convs`` through :func:`run_otel` with a 1 Hz disk+counter
    sampler, then folds the per-turn latency / TTFT percentiles, the per-tick
    ``[prom]`` deltas, and the whole-run counter movement into ``summary_base`` —
    identical shape and markers to the ShareGPT async path so
    ``tools/render_kvprofile.py`` plots both the same way.
    """
    import asyncio

    engine = common.build_engine(engine_kwargs, async_mode=True)
    common.start_prom_exporter()
    try:
        progress_interval = float(os.environ.get("ASYNC_PROGRESS_SECS", "10"))
    except ValueError:
        progress_interval = 10.0
    _cadence = (f", progress every {progress_interval:g}s"
                if progress_interval > 0 else "")
    _sched = ("timestamps DISABLED (TIME_SCALE=0, saturating)" if time_scale == 0
              else f"timestamps scaled x{time_scale:g}" if time_scale != 1.0
              else "real recorded timing")
    if active_sessions and active_sessions > 0:
        print(f"[run] OTel replay — closed loop, {active_sessions} active "
              f"sessions; {len(convs)} conversations; {_sched}{_cadence}",
              file=sys.stderr, flush=True)
    else:
        print(f"[run] OTel replay — one coroutine per conversation "
              f"(max_num_seqs bounds the running batch); {len(convs)} "
              f"conversations; {_sched}{_cadence}", file=sys.stderr, flush=True)

    def _disk():
        return disk_rw_bytes() if disk_rw_bytes is not None else (None, None)

    def sampler():
        rd, wr = _disk()
        return {"prom": common.prom_counters(engine, capture_metrics),
                "read_bytes": rd, "write_bytes": wr}

    async def _amain():
        tokenizer = await mta.get_tokenizer(engine)
        n_tokens = common.make_n_tokens(tokenizer, n_tokens_flavor)
        return await run_otel(
            engine, convs, base_sp,
            prompt_budget=prompt_budget,
            context_cap=context_cap,
            n_tokens=n_tokens,
            time_scale=time_scale,
            max_output_tokens=max_output_tokens,
            sampler=sampler if capture_metrics else None,
            progress_interval=progress_interval,
            active_sessions=active_sessions,
            session_id_fn=session_id_fn,
        )

    result = asyncio.run(_amain())

    lat = sorted(r[5] for r in result["turn_records"])
    ttfts = sorted(r[4] for r in result["turn_records"] if r[4] is not None)

    def _pct(vals, p):
        if not vals:
            return None
        return vals[min(len(vals) - 1, int(p * len(vals)))]

    print(f"[run] otel turn latency: n={len(lat)}  "
          f"p50={_pct(lat, 0.50)} p90={_pct(lat, 0.90)} p99={_pct(lat, 0.99)}  "
          f"ttft_p50={_pct(ttfts, 0.50)}", file=sys.stderr, flush=True)
    print(f"[run] otel recorded delay applied: "
          f"{result.get('recorded_delay_applied', 0.0):.0f}s "
          f"(sum of inter-turn gaps actually slept across all conversations)",
          file=sys.stderr, flush=True)

    # Per-tick [prom] markers, identical form to run_multiturn_async (strip
    # vllm:/_total via mta._prom_key), so render_kvprofile.py plots OTel runs too.
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
                        d[mta._prom_key(k)] = delta
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

    proms = [v["prom"] for _, v in result["samples"] if v.get("prom")]
    counter_movement = {}
    if proms:
        first, last = proms[0], proms[-1]
        counter_movement = {k: last.get(k, 0.0) - first.get(k, 0.0)
                            for k in last if last.get(k, 0.0) - first.get(k, 0.0)}
    if capture_metrics and not proms:
        print("[prom] otel: REGISTRY empty under AsyncLLM — aggregate-only "
              "(timing/percentiles valid, no vllm: counters)",
              file=sys.stderr, flush=True)
    elif counter_movement:
        shown = " ".join(f"{k[len('vllm:'):] if k.startswith('vllm:') else k}"
                         f"={counter_movement[k]:.0f}"
                         for k in sorted(counter_movement))
        print(f"[prom] otel counter movement: {shown}", file=sys.stderr, flush=True)

    summary = dict(summary_base or {})
    summary.update({
        "elapsed_time": result["elapsed"],
        "num_conversations": len(convs),
        "num_rounds": result["rounds_done"],
        "total_generations": result["total_generations"],
        "mode": "otel-async",
        "time_scale": time_scale,
        "recorded_delay_applied": result.get("recorded_delay_applied"),
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
