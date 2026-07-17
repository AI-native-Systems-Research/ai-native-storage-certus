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
    LOG_STATS        emit vLLM engine + KV-offload stats (default 1; 0 = off)
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
        # LOG_STATS surfaces vLLM's periodic engine stats, including the
        # OffloadingConnector's KVConnectorStats (per-interval blocks/tokens
        # loaded and stored over the KV-offload API). On by default; set
        # LOG_STATS=0 to silence it. The SSD I/O deltas below print regardless.
        disable_log_stats=(os.environ.get("LOG_STATS", "1") == "0"),
    )

    sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=OUTPUT_TOKENS)
    tokenizer = llm.get_tokenizer()

    def n_tokens(text: str) -> int:
        return len(tokenizer(text).input_ids)

    contexts = [""] * len(convs)
    alive = [True] * len(convs)
    next_turn = [0] * len(convs)

    # --- Per-round SSD I/O accounting via the server's GetIoStats RPC ---------
    # The server aggregates per-direction read/write byte/op/latency counters
    # across all data drives (requires the server built with --features
    # rw-telemetry). We open our own channel to poll it around each round for
    # deltas — the gRPC analogue of the in-process iostat file.
    import grpc
    from certus_grpc_connector import dispatcher_pb2 as _pb
    from certus_grpc_connector import dispatcher_pb2_grpc as _pbg

    _io_chan = grpc.insecure_channel(CERTUS_SERVER)
    _io_stub = _pbg.DispatcherStub(_io_chan)

    def io_stats():
        # Returns (read_ops, read_bytes, read_lat_ns_sum, write_ops,
        # write_bytes, write_lat_ns_sum); zeros if the server lacks the feature.
        try:
            r = _io_stub.GetIoStats(_pb.GetIoStatsRequest())
            return (r.read_ops, r.read_bytes, r.read_latency_ns_sum,
                    r.write_ops, r.write_bytes, r.write_latency_ns_sum)
        except Exception as e:  # noqa: BLE001
            print(f"[run] GetIoStats failed: {e}", file=sys.stderr, flush=True)
            return (0, 0, 0, 0, 0, 0)

    def gib(n):
        return f"{n / (1024**3):.2f}GiB"

    def mean_us(lat_ns_sum, ops):
        return f"{(lat_ns_sum / ops) / 1000:.1f}us" if ops else "n/a"

    io_prev = io_stats()

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

        # Per-round SSD I/O deltas from the server counters.
        io_now = io_stats()
        d = [io_now[k] - io_prev[k] for k in range(6)]
        io_prev = io_now
        d_rops, d_rb, d_rlat, d_wops, d_wb, d_wlat = d
        print(
            f"[run] round {rounds_done}: {len(active_prompts)} prompts, "
            f"{total_generations} total generations  "
            f"ssd_read={gib(d_rb)} ssd_write={gib(d_wb)} "
            f"r_ops={d_rops} w_ops={d_wops} "
            f"r_lat={mean_us(d_rlat, d_rops)} w_lat={mean_us(d_wlat, d_wops)}",
            file=sys.stderr,
        )

    elapsed = time.perf_counter() - t_start
    print(
        f"[run] DONE rounds={rounds_done} generations={total_generations} "
        f"elapsed={elapsed:.1f}s ({total_generations / elapsed:.1f} gen/s)",
        file=sys.stderr,
    )
