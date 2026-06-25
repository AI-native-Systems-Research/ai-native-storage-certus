#!/usr/bin/env python3
"""run_multiturn_offloading.py — drive multi-turn chat through vLLM offline
inference with TracingOffloadingConnector + TracingCPUOffloadingSpec, capturing
KV cache offload behavior under realistic prefix-growing chat workload.

For each ShareGPT conversation, the driver runs one generation per human turn,
batched across all active conversations per round. Round k's prompt for a
conversation is the cumulative concatenation of every prior human turn,
every prior vLLM-generated response (NOT the dataset's gpt response — using
vLLM's own output is what makes prefix tokens match exactly turn-to-turn so
the offload cache sees real read-path traffic), and the k'th human turn.

Configurable via env vars:
    NUM_CONVS      number of conversations to run     (default 500)
    MAX_MODEL_LEN  vLLM context window (tokens)       (default 8192)
    OUTPUT_TOKENS  max generated tokens per round     (default 200)
    MAX_NUM_SEQS   vLLM batch parallelism             (default 64)
    GPU_MEM_UTIL   vLLM gpu_memory_utilization        (default 0.90)
    CPU_BYTES      offload tier size (bytes)          (default 4 GiB)
    MODEL          HF model id                        (default NousResearch/Meta-Llama-3-8B)

Conversations whose next prompt would exceed MAX_MODEL_LEN - OUTPUT_TOKENS are
stopped early and excluded from subsequent rounds.
"""

if __name__ == "__main__":
    import json
    import os
    import sys
    import time

    _here = os.path.dirname(os.path.abspath(__file__))
    if _here not in sys.path:
        sys.path.insert(0, _here)

    SUBSET_PATH = os.path.join(_here, "sharegpt_subset_5000.json")
    if not os.path.exists(SUBSET_PATH):
        print(f"[run] missing {SUBSET_PATH}", file=sys.stderr)
        sys.exit(1)

    NUM_CONVS = int(os.environ.get("NUM_CONVS", 500))
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 200))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    CPU_BYTES = int(os.environ.get("CPU_BYTES", 4 * (1 << 30)))
    MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS
    print(f"[run] model={MODEL}", file=sys.stderr)
    print(f"[run] num_convs={NUM_CONVS} max_model_len={MAX_MODEL_LEN} "
          f"output_tokens={OUTPUT_TOKENS} max_num_seqs={MAX_NUM_SEQS}",
          file=sys.stderr)
    print(f"[run] prompt_budget={PROMPT_BUDGET} tokens (max_model_len - output)",
          file=sys.stderr)
    print(f"[run] cpu_offload_bytes={CPU_BYTES}", file=sys.stderr)

    # ── Load conversations and extract human-turn streams ─────────────────
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
    print(f"[run] loaded {len(convs)} conversations  "
          f"(human-turn count: min={min(len(c) for c in convs)} "
          f"median={sorted(len(c) for c in convs)[len(convs)//2]} "
          f"max={max(len(c) for c in convs)})",
          file=sys.stderr)

    # ── Init vLLM with tracing connector ──────────────────────────────────
    KV_CONFIG = {
        "kv_connector": "TracingOffloadingConnector",
        "kv_connector_module_path": "tracing_offloading_connector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            "cpu_bytes_to_use": CPU_BYTES,
            "spec_name": "TracingCPUOffloadingSpec",
            "spec_module_path": "tracing_offloading_manager",
            "eviction_policy": "lru",
        },
    }

    # Clear stale trace files
    for f in os.listdir(_here):
        if (f.startswith("offloading_trace_")
                or f.startswith("offloading_mgr_")
                or f.startswith("offloading_handler_")) \
                and f.endswith(".jsonl"):
            os.remove(os.path.join(_here, f))

    from vllm import LLM, SamplingParams

    llm = LLM(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        kv_transfer_config=KV_CONFIG,
        disable_log_stats=True,
    )

    sp = SamplingParams(
        temperature=0.7,
        top_p=0.95,
        max_tokens=OUTPUT_TOKENS,
    )

    tokenizer = llm.get_tokenizer()

    def n_tokens(text: str) -> int:
        return len(tokenizer(text).input_ids)

    # contexts[i] = current full prompt accumulated so far for conv i
    contexts = [""] * len(convs)
    # alive[i] = True while conv i is still emitting prompts that fit
    alive = [True] * len(convs)
    # next_turn[i] = index of the next human turn to emit
    next_turn = [0] * len(convs)

    rounds_done = 0
    total_generations = 0
    t_start = time.perf_counter()

    while True:
        # Build this round's batch
        active_idx = []
        active_prompts = []
        for i, conv in enumerate(convs):
            if not alive[i]:
                continue
            k = next_turn[i]
            if k >= len(conv):
                alive[i] = False
                continue
            # Build candidate prompt for this round
            human = conv[k]
            if k == 0:
                candidate = human
            else:
                candidate = contexts[i] + "\n\n" + human
            if n_tokens(candidate) > PROMPT_BUDGET:
                # Conv outgrew budget — drop from further rounds
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
        # Append vLLM's own response for next round's prefix
        for i, out in zip(active_idx, outs):
            response = out.outputs[0].text if out.outputs else ""
            contexts[i] = contexts[i] + response
            next_turn[i] += 1
        total_generations += len(active_prompts)
        round_elapsed = time.perf_counter() - round_start
        n_alive = sum(alive)
        print(f"[run] round {rounds_done}: {len(active_prompts)} prompts in "
              f"{round_elapsed:.1f}s  ({n_alive} convs still alive)",
              file=sys.stderr, flush=True)

    elapsed = time.perf_counter() - t_start
    summary = {
        "elapsed_time": elapsed,
        "num_conversations": len(convs),
        "num_rounds": rounds_done,
        "total_generations": total_generations,
        "model": MODEL,
        "max_model_len": MAX_MODEL_LEN,
        "output_tokens": OUTPUT_TOKENS,
        "cpu_bytes_to_use": CPU_BYTES,
    }
    with open(os.path.join(_here, "sharegpt_multiturn_results.json"), "w") as f:
        json.dump(summary, f, indent=2)
    print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} "
          f"rounds={rounds_done}", file=sys.stderr)

    # List trace files produced
    traces = sorted(
        f for f in os.listdir(_here)
        if (f.startswith("offloading_trace_")
            or f.startswith("offloading_mgr_")
            or f.startswith("offloading_handler_"))
        and f.endswith(".jsonl")
    )
    if traces:
        print("[run] trace files:", file=sys.stderr)
        for f in traces:
            p = os.path.join(_here, f)
            size_mb = os.path.getsize(p) / (1 << 20)
            print(f"   {f}  ({size_mb:.1f} MiB)", file=sys.stderr)
