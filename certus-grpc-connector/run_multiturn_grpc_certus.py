#!/usr/bin/env python3
"""run_multiturn_grpc_certus.py — multi-turn e2e workload via CertusGrpcOffloadingSpec.

Same ShareGPT multi-turn workload as certus-connector/run_multiturn_certus.py,
but drives the *gRPC* connector (CertusGrpcOffloadingSpec) against a running
certus-server instead of the in-process PyO3 engine.

Defaults target the 450-conversation / 12-turn dataset shipped with the
in-process connector:
    DATASET_PATH = ../certus-connector/sharegpt_12turn_450.json
    NUM_CONVS    = 450

Environment:
    CERTUS_SERVER    gRPC server address (default localhost:50051)
    MODEL            HF model id (default NousResearch/Meta-Llama-3-8B)
    NUM_CONVS        conversations to load (default 450)
    MAX_MODEL_LEN    (default 8192)
    OUTPUT_TOKENS    per generation (default 150)
    MAX_NUM_SEQS     (default 64)
    GPU_MEM_UTIL     (default 0.90)
    SLAB_SIZE_BYTES  offload block size (default 131072)
    DATASET_PATH     override dataset json
"""

if __name__ == "__main__":
    import json
    import os
    import sys
    import time

    _here = os.path.dirname(os.path.abspath(__file__))
    if _here not in sys.path:
        sys.path.insert(0, _here)

    DEFAULT_DATASET = os.path.join(
        _here, "..", "certus-connector", "sharegpt_12turn_450.json"
    )
    DATASET_PATH = os.environ.get("DATASET_PATH", DEFAULT_DATASET)
    if not os.path.exists(DATASET_PATH):
        print(f"[run] missing dataset {DATASET_PATH}", file=sys.stderr)
        sys.exit(1)

    CERTUS_SERVER = os.environ.get("CERTUS_SERVER", "localhost:50051")
    NUM_CONVS = int(os.environ.get("NUM_CONVS", 450))
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    SLAB_SIZE_BYTES = int(os.environ.get("SLAB_SIZE_BYTES", 131072))
    MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS
    print(f"[run] model={MODEL} server={CERTUS_SERVER}", file=sys.stderr)
    print(
        f"[run] num_convs={NUM_CONVS} max_model_len={MAX_MODEL_LEN} "
        f"output_tokens={OUTPUT_TOKENS} max_num_seqs={MAX_NUM_SEQS}",
        file=sys.stderr,
    )

    KV_CONFIG = {
        "kv_connector": "OffloadingConnector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            "spec_name": "CertusGrpcOffloadingSpec",
            "spec_module_path": "certus_grpc_connector.spec",
            "server": CERTUS_SERVER,
            "slab_size_bytes": SLAB_SIZE_BYTES,
        },
    }

    with open(DATASET_PATH) as f:
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

        outputs = llm.generate(active_prompts, sp)
        for j, out in enumerate(outputs):
            i = active_idx[j]
            gen = out.outputs[0].text
            contexts[i] = contexts[i] + gen
            next_turn[i] += 1
            total_generations += 1
        rounds_done += 1
        print(
            f"[run] round {rounds_done}: {len(active_prompts)} prompts, "
            f"{total_generations} total generations",
            file=sys.stderr,
        )

    elapsed = time.perf_counter() - t_start
    print(
        f"[run] DONE rounds={rounds_done} generations={total_generations} "
        f"elapsed={elapsed:.1f}s ({total_generations / elapsed:.1f} gen/s)",
        file=sys.stderr,
    )
