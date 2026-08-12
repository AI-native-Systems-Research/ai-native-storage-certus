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

    # TOKEN_TRACE — replay a Qwen-derived synthetic token trace instead of the
    # ShareGPT text workload (see qwen_trace_to_tokentrace.py). DATASET_PATH is
    # not needed in this mode.
    TOKEN_TRACE = os.environ.get("TOKEN_TRACE", "").strip()
    BLOCK_SIZE = int(os.environ.get("BLOCK_SIZE", 16))

    SUBSET_PATH = os.environ.get("DATASET_PATH",
                                   os.path.join(_here, "sharegpt_subset_5000.json"))
    if not TOKEN_TRACE and not os.path.exists(SUBSET_PATH):
        print(f"[run] missing {SUBSET_PATH}", file=sys.stderr)
        sys.exit(1)

    NUM_CONVS = int(os.environ.get("NUM_CONVS", 500))
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")
    MAX_ROUNDS = int(os.environ.get("MAX_ROUNDS", 0))  # 0 = until convs exhausted

    # Deterministic hash -> token-id expansion (see run_multiturn_offloading.py).
    def _splitmix64(x):
        x = (x + 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF
        z = x
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
        return (z ^ (z >> 31)) & 0xFFFFFFFFFFFFFFFF

    def expand_hashes(hash_ids, vocab_size, block):
        ids = []
        for h in hash_ids:
            base = (int(h) * block) & 0xFFFFFFFFFFFFFFFF
            for j in range(block):
                ids.append(_splitmix64(base + j) % vocab_size)
        return ids

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS
    print(f"[run] model={MODEL}", file=sys.stderr)
    print(f"[run] NO-OFFLOAD baseline (no kv_transfer_config)", file=sys.stderr)
    print(f"[run] num_convs={NUM_CONVS} max_model_len={MAX_MODEL_LEN} "
          f"output_tokens={OUTPUT_TOKENS} max_num_seqs={MAX_NUM_SEQS}",
          file=sys.stderr)

    sessions = None
    convs = []
    if TOKEN_TRACE:
        with open(TOKEN_TRACE) as f:
            tt = json.load(f)
        sessions = tt["sessions"] if isinstance(tt, dict) else tt
        tt_block = (tt.get("meta", {}).get("block_size", BLOCK_SIZE)
                    if isinstance(tt, dict) else BLOCK_SIZE)
        if tt_block != BLOCK_SIZE:
            print(f"[run] WARNING: token-trace block_size={tt_block} != "
                  f"BLOCK_SIZE={BLOCK_SIZE}; using {tt_block}", file=sys.stderr)
            BLOCK_SIZE = tt_block
        if NUM_CONVS and len(sessions) > NUM_CONVS:
            sessions = sessions[:NUM_CONVS]
        n_turns = sum(len(s["turns"]) for s in sessions)
        print(f"[run] TOKEN_TRACE {os.path.basename(TOKEN_TRACE)}: "
              f"{len(sessions)} sessions, {n_turns} turns, block_size={BLOCK_SIZE}",
              file=sys.stderr)
    else:
        with open(SUBSET_PATH) as f:
            all_data = json.load(f)
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
        enforce_eager=True,
        block_size=BLOCK_SIZE,
        disable_log_stats=True,
    )

    sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=OUTPUT_TOKENS)
    tokenizer = llm.get_tokenizer()

    def n_tokens(text: str) -> int:
        return len(tokenizer(text).input_ids)

    # ── Token-trace replay path (see run_multiturn_offloading.py) ─────────
    if TOKEN_TRACE:
        vocab_size = tokenizer.vocab_size
        max_turns = max(len(s["turns"]) for s in sessions)
        rounds_done = 0
        total_generations = 0
        total_out_tokens = 0
        t_start = time.perf_counter()
        for k in range(max_turns):
            if MAX_ROUNDS and rounds_done >= MAX_ROUNDS:
                break
            active_prompts = []
            active_sps = []
            for s in sessions:
                if k >= len(s["turns"]):
                    continue
                turn = s["turns"][k]
                ptoks = len(turn["hash_ids"]) * BLOCK_SIZE
                if ptoks == 0 or ptoks >= MAX_MODEL_LEN:
                    continue
                mt = min(int(turn["max_tokens"]), MAX_MODEL_LEN - ptoks)
                if mt < 1:
                    continue
                ids = expand_hashes(turn["hash_ids"], vocab_size, BLOCK_SIZE)
                active_prompts.append({"prompt_token_ids": ids})
                active_sps.append(SamplingParams(
                    temperature=0.7, top_p=0.95, max_tokens=mt, ignore_eos=True))
            if not active_prompts:
                break
            rounds_done += 1
            round_start = time.perf_counter()
            llm.generate(active_prompts, active_sps)
            total_generations += len(active_prompts)
            total_out_tokens += sum(sp.max_tokens for sp in active_sps)
            round_elapsed = time.perf_counter() - round_start
            print(f"[run] round {rounds_done}: {len(active_prompts)} prompts in "
                  f"{round_elapsed:.1f}s", file=sys.stderr, flush=True)
        elapsed = time.perf_counter() - t_start
        tok_s = total_out_tokens / elapsed if elapsed else 0
        summary = {
            "elapsed_time": elapsed,
            "num_sessions": len(sessions),
            "num_rounds": rounds_done,
            "total_generations": total_generations,
            "total_output_tokens": total_out_tokens,
            "tokens_per_sec": tok_s,
            "model": MODEL,
            "max_model_len": MAX_MODEL_LEN,
            "block_size": BLOCK_SIZE,
            "token_trace": os.path.basename(TOKEN_TRACE),
            "offload": "none",
        }
        with open(os.path.join(_here, "nooffload_multiturn_results.json"), "w") as f:
            json.dump(summary, f, indent=2)
        print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} "
              f"rounds={rounds_done}  out_tokens={total_out_tokens}  "
              f"tok/s={tok_s:.0f}", file=sys.stderr)
        sys.exit(0)

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
