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

    llm = LLM(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        # ENFORCE_EAGER=1 disables CUDA graphs + torch.compile (pure eager);
        # default "0" captures graphs for lower decode latency.
        enforce_eager=(os.environ.get("ENFORCE_EAGER", "0") != "0"),
        # LOG_STATS=1 keeps vLLM's stats logging on so its PrometheusStatLogger
        # registers metrics. Default off to keep per-round output clean.
        disable_log_stats=(os.environ.get("LOG_STATS", "0") == "0"),
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

    elapsed = time.perf_counter() - t_start
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
