#!/usr/bin/env python3
"""run_multiturn_shmq_certus.py — multi-turn e2e workload via CertusShmqOffloadingSpec.

Drives the *shared-memory* connector (CertusShmqOffloadingSpec) against a
running certus-server over a /dev/shm mailbox. The control plane rides the shmq
ring; KV bytes move GPU<->DRAM<->SSD via CUDA IPC + SPDK DMA.

Defaults target the 450-conversation / 12-turn dataset shipped with the
in-process connector:
    DATASET_PATH = ../data/sharegpt_12turn_450.json
    NUM_CONVS    = 450

Environment:
    SHM_PATH         shared-memory mailbox file (default /dev/shm/certus-shmq)
    MODEL            HF model id (default ibm-granite/granite-4.1-8b)
    NUM_CONVS        conversations to load (default 450)
    MAX_MODEL_LEN    (default 8192)
    OUTPUT_TOKENS    per generation (default 150)
    MAX_NUM_SEQS     (default 64)
    GPU_MEM_UTIL     (default 0.90)
    LOG_STATS        emit vLLM engine + KV-offload stats (default 0 = off)
    TENSOR_PARALLEL_SIZE    GPUs to shard each layer across (default 1)
    PIPELINE_PARALLEL_SIZE  pipeline stages across GPUs/nodes (default 1)
    ENFORCE_EAGER    "0" (default) enables CUDA graphs + torch.compile (faster,
                     and what vLLM uses by default — keeps comparisons vs the
                     native cputier backend fair); "1" forces eager mode
                     (rougher with some connectors, useful for debugging)
    KV_CACHE_DTYPE   KV-cache dtype: "auto" (default, = model dtype) or "fp8"
                     to halve per-sequence KV footprint (may reduce accuracy)
    CONV_MULTIPLIER  replicate the conversation set N× for a larger concurrent
                     working set (default 1); each replica's turn-0 is tagged
                     so contexts hash distinctly
    MAX_ROUNDS       cap the number of rounds/turns (default 0 = until convs
                     exhausted)
    SLAB_SIZE_BYTES  offload block size (default 131072)
    DATASET_PATH     override dataset json

NOTE vs the gRPC driver: the ring transport has no GetIoStats op (that RPC was a
gRPC-only side channel for per-round SSD I/O deltas), so the per-round output
here reports generations/rounds only. Read SSD I/O from the server's own
telemetry (its stderr / iostat) when it is built with --features rw-telemetry.
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
        _here, "..", "data", "sharegpt_12turn_450.json"
    )
    DATASET_PATH = os.environ.get("DATASET_PATH", DEFAULT_DATASET)
    if not os.path.exists(DATASET_PATH):
        print(f"[run] missing dataset {DATASET_PATH}", file=sys.stderr)
        sys.exit(1)

    SHM_PATH = os.environ.get("SHM_PATH", "/dev/shm/certus-shmq")
    NUM_CONVS = int(os.environ.get("NUM_CONVS", 450))
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    # Multi-GPU: shard each layer's weights/KV across TENSOR_PARALLEL_SIZE GPUs
    # (default 1 = single GPU). PIPELINE_PARALLEL_SIZE splits layers into stages
    # across GPUs/nodes; total GPUs used = TP * PP. For a single 8B model on one
    # box, tensor parallelism is the usual choice — set it to the GPU count.
    #
    # Constraints:
    #   - TENSOR_PARALLEL_SIZE must divide the model's attention head counts.
    #     granite-4.1-8b has 32 attention heads / 8 KV heads, so TP in {1,2,4,8}.
    #   - Total GPUs used = TP * PP; must be <= visible GPU count. Restrict which
    #     GPUs are used with CUDA_VISIBLE_DEVICES, e.g.
    #     CUDA_VISIBLE_DEVICES=0,1 TENSOR_PARALLEL_SIZE=2.
    #   - With TP>1 vLLM spawns one worker process per GPU, and each worker
    #     instantiates its own CertusShmqOffloadingSpec. All of them attach the
    #     SAME /dev/shm mailbox (Ring is a per-path process singleton) and claim
    #     distinct channels, so the server must have >= (peak client threads)
    #     channels across all workers.
    TENSOR_PARALLEL_SIZE = int(os.environ.get("TENSOR_PARALLEL_SIZE", 1))
    PIPELINE_PARALLEL_SIZE = int(os.environ.get("PIPELINE_PARALLEL_SIZE", 1))
    SLAB_SIZE_BYTES = int(os.environ.get("SLAB_SIZE_BYTES", 131072))
    MODEL = os.environ.get("MODEL", "ibm-granite/granite-4.1-8b")
    # Replicate the conversation set to raise the concurrent working set per
    # round, and cap rounds to keep total generations constant. Default 1/0 =
    # original 450x12 behaviour. E.g. CONV_MULTIPLIER=4 MAX_ROUNDS=3 gives
    # 1800 convs over 3 rounds == 5400 gens, but 4x the peak KV footprint.
    CONV_MULTIPLIER = int(os.environ.get("CONV_MULTIPLIER", 1))
    MAX_ROUNDS = int(os.environ.get("MAX_ROUNDS", 0))  # 0 = until convs exhausted

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS
    print(f"[run] model={MODEL} shm_path={SHM_PATH}", file=sys.stderr)
    print(
        f"[run] num_convs={NUM_CONVS} max_model_len={MAX_MODEL_LEN} "
        f"output_tokens={OUTPUT_TOKENS} max_num_seqs={MAX_NUM_SEQS}",
        file=sys.stderr,
    )

    KV_CONFIG = {
        "kv_connector": "OffloadingConnector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            "spec_name": "CertusShmqOffloadingSpec",
            "spec_module_path": "certus_shmq_connector.spec",
            "shm_path": SHM_PATH,
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

    # Replicate for a larger concurrent working set. Each replica's first turn
    # is tagged with a unique marker so the accumulated context hashes
    # distinctly per replica -- otherwise byte-identical copies would dedup at
    # the prefix cache / KV-block layer and store no extra data.
    if CONV_MULTIPLIER > 1:
        base = convs
        convs = []
        for r in range(CONV_MULTIPLIER):
            for conv in base:
                tagged = list(conv)
                tagged[0] = f"[r{r}] {tagged[0]}"
                convs.append(tagged)
        print(
            f"[run] replicated x{CONV_MULTIPLIER} -> {len(convs)} conversations",
            file=sys.stderr,
        )

    from vllm import LLM, SamplingParams

    from certus_shmq_connector.compat import CAPS as _CAPS
    from certus_shmq_connector.compat import VERSION as _VLLM_VERSION

    # Version-specific engine flags, resolved from the compat capability matrix.
    # v0.26's OffloadingConnector requires the hybrid KV-cache manager to be
    # disabled (the connector assumes a single, uniform KV-cache group).
    _engine_kwargs = {}
    if _CAPS.needs_disable_hybrid_kv_cache_manager:
        _engine_kwargs["disable_hybrid_kv_cache_manager"] = True
        print(
            f"[run] vLLM {_VLLM_VERSION[0]}.{_VLLM_VERSION[1]}: "
            f"disable_hybrid_kv_cache_manager=True (OffloadingConnector requirement)",
            file=sys.stderr,
        )
    if _CAPS.needs_disable_async_scheduling:
        # 0.22+ auto-enables async scheduling, which breaks the OffloadingConnector's
        # per-request transfer serialization (a re-scheduled load races an in-flight
        # store -> `assert not req_status.transfer_jobs` -> EngineDeadError). Opt out.
        _engine_kwargs["async_scheduling"] = False
        print(
            f"[run] vLLM {_VLLM_VERSION[0]}.{_VLLM_VERSION[1]}: "
            f"async_scheduling=False (OffloadingConnector serializes transfers per request)",
            file=sys.stderr,
        )

    print("Running across ", TENSOR_PARALLEL_SIZE, " GPUs")
    llm = LLM(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        tensor_parallel_size=TENSOR_PARALLEL_SIZE,
        pipeline_parallel_size=PIPELINE_PARALLEL_SIZE,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype=os.environ.get("DTYPE", "float16"),
        enable_prefix_caching=True,
        enforce_eager=(os.environ.get("ENFORCE_EAGER", "0") != "0"),
        **_engine_kwargs,
        # KV_CACHE_DTYPE="fp8" stores KV-cache blocks in 8-bit, halving the
        # per-sequence KV footprint so larger MAX_NUM_SEQS fits before OOM.
        # Default "auto" = same as model dtype (fp16 here).
        kv_cache_dtype=os.environ.get("KV_CACHE_DTYPE", "auto"),
        kv_transfer_config=KV_CONFIG,
        # LOG_STATS=1 surfaces vLLM's periodic engine stats, including the
        # OffloadingConnector's KVConnectorStats (per-interval blocks/tokens
        # loaded and stored over the KV-offload API). Default off to keep the
        # per-round output clean.
        disable_log_stats=(os.environ.get("LOG_STATS", "0") == "0"),
    )

    # Optional Prometheus exporter. When PROM_PORT is set, expose vLLM's engine
    # + KV-offload metrics over HTTP at :PROM_PORT/metrics for live scraping.
    # Requires LOG_STATS=1 (above) so the PrometheusStatLogger is registered in
    # the global client registry — otherwise the endpoint serves an empty
    # registry. No-op when PROM_PORT is unset, so normal bench runs are
    # unchanged.
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

    # NOTE: no per-round SSD I/O accounting here. The gRPC driver polled the
    # server's GetIoStats RPC each round for read/write byte deltas; the ring
    # transport has no equivalent op (it carries only the connector control
    # plane). Read SSD I/O from the server side instead (its stderr telemetry
    # when built with --features rw-telemetry, or host `iostat`).

    rounds_done = 0
    total_generations = 0
    t_start = time.perf_counter()

    while True:
        if MAX_ROUNDS and rounds_done >= MAX_ROUNDS:
            break
        active_idx = []
        active_prompts = []
        active_sps = []
        for i, conv in enumerate(convs):
            if not alive[i]:
                continue
            k = next_turn[i]
            if k >= len(conv):
                alive[i] = False
                continue
            human = conv[k]
            candidate = human if k == 0 else contexts[i] + "\n\n" + human
            nt = n_tokens(candidate)
            if nt == 0 or nt > PROMPT_BUDGET:
                alive[i] = False
                continue
            contexts[i] = candidate
            active_idx.append(i)
            active_prompts.append(candidate)
            # Tag each request with its conversation as the KV-offload session_id.
            # The conversation index is stable across rounds, so every turn of
            # the same conversation shares one session_id; the connector forwards
            # it (hashed to u64) on Reserve -> the dispatcher logs it.
            # +1 so conversation 0 gets a non-zero id (0 == "unset" sentinel).
            sp_i = sp.clone()
            sp_i.extra_args = {"kv_transfer_params": {"session_id": i + 1}}
            active_sps.append(sp_i)

        if not active_prompts:
            break

        outputs = llm.generate(active_prompts, active_sps)
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
