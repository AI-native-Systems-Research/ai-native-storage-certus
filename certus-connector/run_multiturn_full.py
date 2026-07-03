#!/usr/bin/env python3
"""run_multiturn_full.py — multi-turn benchmark on full ShareGPT (94K conversations).

Connector selection via MODE env var: "cpu", "certus", or "sharedstorage".
"""

if __name__ == "__main__":
    import json
    import os
    import sys
    import time

    _here = os.path.dirname(os.path.abspath(__file__))
    _root = os.path.dirname(os.path.dirname(_here))  # kvconn-trace

    SOURCE_PATH = os.path.join(_root, "sharegpt_v3.json")
    if not os.path.exists(SOURCE_PATH):
        print(f"[run] missing {SOURCE_PATH}", file=sys.stderr)
        sys.exit(1)

    MODE = os.environ.get("MODE", "cpu")
    NUM_CONVS = int(os.environ.get("NUM_CONVS", 0))  # 0 = all eligible
    MIN_TURNS = int(os.environ.get("MIN_TURNS", 12))
    MAX_TURNS = int(os.environ.get("MAX_TURNS", 12))
    DRAM_BYTES = int(os.environ.get("DRAM_GB", 16)) * 1024**3
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS

    if MODE == "cpu":
        KV_CONFIG = {
            "kv_connector": "OffloadingConnector",
            "kv_role": "kv_both",
            "kv_connector_extra_config": {
                "cpu_bytes_to_use": DRAM_BYTES,
            },
        }
    elif MODE == "certus":
        KV_CONFIG = {
            "kv_connector": "OffloadingConnector",
            "kv_role": "kv_both",
            "kv_connector_extra_config": {
                "spec_name": "CertusOffloadingSpec",
                "spec_module_path": "certus_connector.spec",
                "data_pci_addrs": ["0000:61:00.0", "0000:62:00.0",
                                   "0000:63:00.0", "0000:64:00.0"],
                "metadata_pci_addr": "0000:62:00.0",
                "slab_size_bytes": 2097152,
                "dram_cache_bytes": DRAM_BYTES,
                "io_queue_depth": 128,
                "numa_node": 0,
            },
        }
    elif MODE == "sharedstorage":
        storage_path = "/mnt/fs-backend-bench/kv-storage"
        os.makedirs(storage_path, exist_ok=True)
        KV_CONFIG = {
            "kv_connector": "OffloadingConnector",
            "kv_role": "kv_both",
            "kv_connector_extra_config": {
                "spec_name": "SharedStorageOffloadingSpec",
                "spec_module_path": "llmd_fs_backend.spec",
                "shared_storage_path": storage_path,
                "threads_per_gpu": 16,
                "block_size": 256,
                "max_staging_memory_gb": 8,
                "gds_mode": "disabled",
                "read_preferring_ratio": 0.75,
            },
        }
    else:
        print(f"Unknown MODE={MODE}", file=sys.stderr)
        sys.exit(1)

    print(f"[run] mode={MODE} model={MODEL}", file=sys.stderr)
    print(f"[run] min_turns={MIN_TURNS} dram={DRAM_BYTES // 1024**3} GiB", file=sys.stderr)
    print(f"[run] max_model_len={MAX_MODEL_LEN} output_tokens={OUTPUT_TOKENS} "
          f"max_num_seqs={MAX_NUM_SEQS}", file=sys.stderr)

    with open(SOURCE_PATH) as f:
        all_data = json.load(f)
    convs = []
    for entry in all_data:
        if NUM_CONVS and len(convs) >= NUM_CONVS:
            break
        turns = entry.get("conversations", [])
        human_turns = [t["value"] for t in turns if t.get("from") == "human"]
        if len(human_turns) >= MIN_TURNS and len(human_turns) <= MAX_TURNS:
            convs.append(human_turns)
    print(f"[run] loaded {len(convs)} conversations ({MIN_TURNS}-{MAX_TURNS} turns, "
          f"from {len(all_data)} total)", file=sys.stderr)

    from vllm import LLM, SamplingParams

    llm = LLM(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        enforce_eager=True,
        kv_transfer_config=KV_CONFIG,
        disable_log_stats=True,
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
    t_start = time.perf_counter()

    while True:
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
            if n_tokens(candidate) > PROMPT_BUDGET:
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
        for i, out in zip(active_idx, outs):
            response = out.outputs[0].text if out.outputs else ""
            contexts[i] = contexts[i] + response
            next_turn[i] += 1
        total_generations += len(active_prompts)
        round_elapsed = time.perf_counter() - round_start
        n_alive = sum(alive)
        elapsed_so_far = time.perf_counter() - t_start
        print(f"[run] round {rounds_done}: {len(active_prompts)} prompts in "
              f"{round_elapsed:.1f}s  ({n_alive} convs still alive)  "
              f"wall={elapsed_so_far:.0f}s",
              file=sys.stderr, flush=True)

    elapsed = time.perf_counter() - t_start
    summary = {
        "mode": MODE,
        "elapsed_time": elapsed,
        "num_conversations": len(convs),
        "num_rounds": rounds_done,
        "total_generations": total_generations,
        "model": MODEL,
        "max_model_len": MAX_MODEL_LEN,
        "output_tokens": OUTPUT_TOKENS,
    }
    results_path = os.path.join(_root, f"full_multiturn_{MODE}_results.json")
    with open(results_path, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} "
          f"rounds={rounds_done}", file=sys.stderr)
    print(f"[run] results saved to {results_path}", file=sys.stderr)
