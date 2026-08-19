#!/usr/bin/env python3
"""run_multiturn_nooffload.py — multi-turn e2e benchmark, NO KV offloading.

Same workload/driver loop as the other backends (via multiturn_workload), but
vLLM runs with no kv_transfer_config at all — the plain GPU-only baseline.
Prefix caching stays on (matching the offload runs); the only difference is that
evicted KV is recomputed rather than fetched from an offload tier. Use this as
the "no offload" reference point against Certus / CPU / SharedStorage.
"""

if __name__ == "__main__":
    import json
    import os
    import sys
    import time

    _here = os.path.dirname(os.path.abspath(__file__))
    if _here not in sys.path:
        sys.path.insert(0, _here)

    import multiturn_workload as mw

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

    convs = mw.load_convs(SUBSET_PATH, NUM_CONVS)
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
        # ENFORCE_EAGER=1 disables CUDA-graph capture / torch.compile. Default 0
        # (graphs on) — matches vLLM's default and the other profile backends.
        enforce_eager=(os.environ.get("ENFORCE_EAGER", "0") != "0"),
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

    # ── Per-round vLLM Prometheus counter deltas ──────────────────────────
    prom_prev = [mw.prom_counters(llm, CAPTURE_METRICS)]
    prom_rounds = []   # (round, {counter_name: delta})
    round_stats = []   # (round, prompts, elapsed, n_alive)

    def on_round_end(round_idx, n_prompts, round_elapsed, n_alive):
        round_stats.append((round_idx, n_prompts, round_elapsed, n_alive))
        print(f"[run] round {round_idx}: {n_prompts} prompts in "
              f"{round_elapsed:.1f}s  ({n_alive} convs still alive)",
              file=sys.stderr, flush=True)
        if CAPTURE_METRICS:
            prom_now = mw.prom_counters(llm, CAPTURE_METRICS)
            d_prom = {k: prom_now.get(k, 0.0) - prom_prev[0].get(k, 0.0)
                      for k in prom_now}
            prom_prev[0] = prom_now
            prom_rounds.append((round_idx, d_prom))
            shown = " ".join(f"{k[len('vllm:'):]}={d_prom[k]:.0f}"
                             for k in sorted(d_prom) if d_prom[k])
            print(f"[prom] round {round_idx}: {shown or '(no counter movement)'}",
                  file=sys.stderr, flush=True)

    result = mw.run_batched(
        llm, convs, sp,
        prompt_budget=PROMPT_BUDGET,
        max_rounds=MAX_ROUNDS,
        n_tokens=n_tokens,
        skip_empty=True,
        on_round_end=on_round_end,
    )
    elapsed = result["elapsed"]
    rounds_done = result["rounds_done"]
    total_generations = result["total_generations"]

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
        hists = mw.prom_histograms(llm, {"vllm:request_queue_time_seconds",
                                         "vllm:request_decode_time_seconds"},
                                   CAPTURE_METRICS)
        for name, h in sorted(hists.items()):
            cnt, tot = h["count"], h["sum"]
            mean = tot / cnt if cnt else 0.0
            fmt = lambda x: "n/a" if x is None else f"{x:.3f}s"  # noqa: E731
            print(f"[prom] hist {name[len('vllm:'):]}: n={cnt} mean={mean:.3f}s "
                  f"p50={fmt(mw.hist_pct(h['buckets'], cnt, 0.50))} "
                  f"p90={fmt(mw.hist_pct(h['buckets'], cnt, 0.90))} "
                  f"p99={fmt(mw.hist_pct(h['buckets'], cnt, 0.99))}",
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
