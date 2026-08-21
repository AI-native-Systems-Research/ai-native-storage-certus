#!/usr/bin/env python3
"""run_multiturn_nooffload.py — multi-turn e2e benchmark, NO KV offloading.

Same workload/driver loop as run_multiturn_certus.py, but vLLM runs with no
kv_transfer_config at all — the plain GPU-only baseline. Prefix caching stays
on (matching the offload runs); the only difference is that evicted KV is
recomputed rather than fetched from an offload tier. Use this as the
"no offload" reference point against Certus / CPU / SharedStorage.
"""

if __name__ == "__main__":
    import json
    import os
    import sys
    import time

    _here = os.path.dirname(os.path.abspath(__file__))
    if _here not in sys.path:
        sys.path.insert(0, _here)

    SUBSET_PATH = os.environ.get("DATASET_PATH",
                                   os.path.join(_here, "sharegpt_subset_5000.json"))
    if not os.path.exists(SUBSET_PATH):
        print(f"[run] missing {SUBSET_PATH}", file=sys.stderr)
        sys.exit(1)

    NUM_CONVS = int(os.environ.get("NUM_CONVS", 500))
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")
    MAX_ROUNDS = int(os.environ.get("MAX_ROUNDS", 0))  # 0 = until convs exhausted

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS
    print(f"[run] model={MODEL}", file=sys.stderr)
    print(f"[run] NO-OFFLOAD baseline (no kv_transfer_config)", file=sys.stderr)
    print(f"[run] num_convs={NUM_CONVS} max_model_len={MAX_MODEL_LEN} "
          f"output_tokens={OUTPUT_TOKENS} max_num_seqs={MAX_NUM_SEQS}",
          file=sys.stderr)

    with open(SUBSET_PATH) as f:
        all_data = json.load(f)
    convs = []
    for entry in all_data:
        if len(convs) >= NUM_CONVS:
            break
        turns = entry.get("conversations", [])
        human_turns = [t["value"] for t in turns if t.get("from") == "human"]
        if len(human_turns) >= 2:
            convs.append(human_turns)
    print(f"[run] loaded {len(convs)} conversations", file=sys.stderr)

    from vllm import LLM, SamplingParams

    # Capturing vLLM's Prometheus counters requires the engine's stat logging to
    # be on (it's what advances the metrics). Enabling it has a small overhead, so
    # a capture run is not byte-identical to the stats-off timing baseline —
    # CAPTURE_METRICS=0 restores that baseline and skips the per-round snapshot.
    CAPTURE_METRICS = os.environ.get("CAPTURE_METRICS", "1") != "0"

    # Model FLOPs Utilization (MFU): enable_mfu_metrics makes the engine emit
    # estimated per-GPU FLOPs / read+write bytes as monotonic Counters
    # (vllm:estimated_flops_per_gpu_total et al.), captured per round by the
    # prom_counters() snapshot below. The arg only exists on newer vLLM — detect
    # support at runtime so older versions don't reject the kwarg.
    _mfu_kwargs = {}
    if CAPTURE_METRICS:
        try:
            import dataclasses as _dc
            from vllm.engine.arg_utils import EngineArgs as _EA
            if any(f.name == "enable_mfu_metrics" for f in _dc.fields(_EA)):
                _mfu_kwargs["enable_mfu_metrics"] = True
        except Exception as e:  # noqa: BLE001
            print(f"[prom] enable_mfu_metrics unavailable: {e}", file=sys.stderr)
        print(f"[prom] enable_mfu_metrics={bool(_mfu_kwargs)} "
              f"(estimated_flops_per_gpu_total "
              f"{'captured per round' if _mfu_kwargs else 'ABSENT'})",
              file=sys.stderr)

    llm = LLM(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        enforce_eager=True,
        **_mfu_kwargs,
        disable_log_stats=not CAPTURE_METRICS,
    )

    # Optional Prometheus exporter. When PROM_PORT is set, expose vLLM's engine
    # metrics over HTTP at :PROM_PORT/metrics for live scraping. Requires
    # LOG_STATS=1 (above) so metrics are registered — otherwise the endpoint
    # serves an empty registry. No-op when PROM_PORT is unset.
    _prom_port = os.environ.get("PROM_PORT")
    if _prom_port:
        from prometheus_client import start_http_server

        start_http_server(int(_prom_port))
        print(f"[prom] metrics exporter listening on :{_prom_port}/metrics", file=sys.stderr)
        if os.environ.get("LOG_STATS", "0") == "0":
            print(
                "[prom] warning: LOG_STATS is off — vLLM metrics are not "
                "registered, so /metrics will be empty. Set LOG_STATS=1.",
                file=sys.stderr,
            )

    sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=OUTPUT_TOKENS)
    tokenizer = llm.get_tokenizer()

    def n_tokens(text: str) -> int:
        return len(tokenizer(text).input_ids)

    contexts = [""] * len(convs)
    alive = [True] * len(convs)
    next_turn = [0] * len(convs)

    # ── vLLM Prometheus counters (per round) ──────────────────────────────
    # vLLM registers every metric on the default prometheus_client REGISTRY under
    # the `vllm:` prefix, updated as the engine steps (present on both the V0 and
    # V1 engines whenever stat logging is on). Snapshot each counter (samples
    # named `vllm:*_total`, summed across label sets) at the end of every round
    # and log the delta; the full per-round series is also dumped to JSON.
    def prom_counters():
        # Prefer the V1 offline snapshot: engine counters (prompt/generation
        # tokens, prefix-cache queries/hits, preemptions) are exposed by
        # llm.get_metrics() and are NOT on the global prometheus REGISTRY in
        # offline mode — reading only the REGISTRY (or the log_stats logger) misses
        # them. Then supplement with the REGISTRY for older (V0) engines, where
        # get_metrics() is absent, and for connector-registered metrics
        # (vllm:kv_offload_*) which only ever live on the global registry.
        # Names differ by source: get_metrics() uses bare names
        # (vllm:prefix_cache_queries); REGISTRY counter samples carry the _total
        # suffix — we keep both keys so the delta shows under whichever the
        # running version populates.
        vals = {}
        if not CAPTURE_METRICS:
            return vals
        try:
            for m in llm.get_metrics():
                if type(m).__name__ != "Counter":
                    continue
                name = getattr(m, "name", "")
                val = getattr(m, "value", None)
                if name.startswith("vllm:") and isinstance(val, (int, float)):
                    # Sum across label sets: get_metrics() emits one Counter per
                    # sample, so labeled metrics (request_success by finish_reason,
                    # prompt_tokens_by_source, per-engine under TP>1) share a name.
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

    def prom_histograms(names):
        # Sample the named vLLM latency histograms ONCE (cumulative over the whole
        # run). get_metrics() exposes each as Histogram(count, sum, buckets) where
        # buckets maps an upper bound `le` -> cumulative count <= le; sum across
        # label sets (per-engine / finish-reason) into one distribution per name.
        out = {}
        if not CAPTURE_METRICS:
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

    def _hist_pct(buckets, count, p):
        # Percentile from cumulative buckets: the smallest upper bound `le` whose
        # cumulative count first reaches p*count. Bucket-granular approximation;
        # returns inf when the crossing lands in the +Inf bucket.
        if not count:
            return None
        target = p * count

        def _le(k):
            return float("inf") if k in ("+Inf", "inf", "Inf") else float(k)

        for le in sorted(buckets, key=_le):
            if buckets[le] >= target:
                return _le(le)
        return float("inf")

    prom_prev = prom_counters()
    prom_rounds = []  # (round, {counter_name: delta})

    rounds_done = 0
    total_generations = 0
    round_stats = []  # (round, prompts, elapsed, n_alive)

    t_start = time.perf_counter()

    while True:
        if MAX_ROUNDS and rounds_done >= MAX_ROUNDS:
            break
        active_idx = []
        active_prompts = []
        for i, conv in enumerate(convs):
            if not alive[i]:
                continue
            k = next_turn[i]
            if k >= len(conv):
                alive[i] = False
                continue
            human = conv[k]
            candidate = human if k == 0 else contexts[i] + "\n\n" + human
            # Guard both bounds: some ShareGPT turns are empty strings, which
            # granite tokenizes to 0 tokens. An empty decoder prompt makes vLLM
            # raise "The decoder prompt cannot be empty" and abort the engine, so
            # drop those convs (nt == 0) alongside the over-budget ones.
            nt = n_tokens(candidate)
            if nt == 0 or nt > PROMPT_BUDGET:
                alive[i] = False
                continue
            contexts[i] = candidate
            active_idx.append(i)
            active_prompts.append(candidate)

        if not active_prompts:
            break

        rounds_done += 1
        round_start = time.perf_counter()
        outs = llm.generate(active_prompts, sp)
        round_elapsed = time.perf_counter() - round_start
        for i, out in zip(active_idx, outs):
            response = out.outputs[0].text if out.outputs else ""
            contexts[i] = contexts[i] + response
            next_turn[i] += 1
        total_generations += len(active_prompts)
        n_alive = sum(alive)
        round_stats.append((rounds_done, len(active_prompts), round_elapsed, n_alive))
        print(f"[run] round {rounds_done}: {len(active_prompts)} prompts in "
              f"{round_elapsed:.1f}s  ({n_alive} convs still alive)",
              file=sys.stderr, flush=True)
        if CAPTURE_METRICS:
            prom_now = prom_counters()
            d_prom = {k: prom_now.get(k, 0.0) - prom_prev.get(k, 0.0)
                      for k in prom_now}
            prom_prev = prom_now
            prom_rounds.append((rounds_done, d_prom))
            shown = " ".join(f"{k[len('vllm:'):]}={d_prom[k]:.0f}"
                             for k in sorted(d_prom) if d_prom[k])
            print(f"[prom] round {rounds_done}: {shown or '(no counter movement)'}",
                  file=sys.stderr, flush=True)

    elapsed = time.perf_counter() - t_start
    if CAPTURE_METRICS and prom_rounds:
        try:
            with open(os.path.join(_here, "prom_counters_rounds.json"), "w") as f:
                json.dump([{"round": r, "counters": d} for r, d in prom_rounds],
                          f, indent=2)
        except OSError as e:
            print(f"[prom] could not save json: {e}", file=sys.stderr)

    # Latency-distribution histograms: sampled once here (cumulative over the run,
    # not per round) — queue time (WAITING phase) and decode time (DECODE phase).
    if CAPTURE_METRICS:
        hists = prom_histograms({"vllm:request_queue_time_seconds",
                                 "vllm:request_decode_time_seconds"})
        for name, h in sorted(hists.items()):
            cnt, tot = h["count"], h["sum"]
            mean = tot / cnt if cnt else 0.0
            fmt = lambda x: "n/a" if x is None else f"{x:.3f}s"  # noqa: E731
            print(f"[prom] hist {name[len('vllm:'):]}: n={cnt} mean={mean:.3f}s "
                  f"p50={fmt(_hist_pct(h['buckets'], cnt, 0.50))} "
                  f"p90={fmt(_hist_pct(h['buckets'], cnt, 0.90))} "
                  f"p99={fmt(_hist_pct(h['buckets'], cnt, 0.99))}",
                  file=sys.stderr, flush=True)
        if hists:
            # Full buckets on one stderr line so the per-variant teed log is a
            # complete source (the shared-name JSON below is overwritten when two
            # variants share this driver / dir).
            print(f"[prom] histjson {json.dumps(hists, separators=(',', ':'))}",
                  file=sys.stderr, flush=True)
            try:
                with open(os.path.join(_here, "prom_histograms.json"), "w") as f:
                    json.dump(hists, f, indent=2)
            except OSError as e:
                print(f"[prom] could not save histogram json: {e}", file=sys.stderr)

    tok_per_s = (total_generations * OUTPUT_TOKENS) / elapsed if elapsed else 0
    summary = {
        "elapsed_time": elapsed,
        "num_conversations": len(convs),
        "num_rounds": rounds_done,
        "total_generations": total_generations,
        "tokens_per_sec": tok_per_s,
        "model": MODEL,
        "max_model_len": MAX_MODEL_LEN,
        "output_tokens": OUTPUT_TOKENS,
        "offload": "none",
        "rounds": [
            {"round": r, "prompts": n, "elapsed": e, "alive": a}
            for r, n, e, a in round_stats
        ],
    }
    with open(os.path.join(_here, "nooffload_multiturn_results.json"), "w") as f:
        json.dump(summary, f, indent=2)
    print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} "
          f"rounds={rounds_done}  tok/s={tok_per_s:.0f}", file=sys.stderr)
