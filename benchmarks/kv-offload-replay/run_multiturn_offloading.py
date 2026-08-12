#!/usr/bin/env python3
"""run_multiturn_offloading.py — drive multi-turn chat through vLLM offline
inference with the CPU-offload KV connector under a realistic prefix-growing
chat workload.

By default this uses vLLM's built-in OffloadingConnector + CPUOffloadingSpec
(host-RAM offload tier, no tracing) — the clean baseline for comparing against
the Certus gRPC backend, which also uses the plain OffloadingConnector. Set
TRACE_OFFLOAD=1 to swap in the local Tracing* wrappers, which additionally
record per-op offload traces (offloading_mgr_<pid>.jsonl etc.) at some overhead.

For each ShareGPT conversation, the driver runs one generation per human turn,
batched across all active conversations per round. Round k's prompt for a
conversation is the cumulative concatenation of every prior human turn,
every prior vLLM-generated response (NOT the dataset's gpt response — using
vLLM's own output is what makes prefix tokens match exactly turn-to-turn so
the offload cache sees real read-path traffic), and the k'th human turn.

Defaults to the 450-conversation / 12-turn ShareGPT dataset shared with the
gRPC connector (../../certus-connector/sharegpt_12turn_450.json).

Configurable via env vars:
    DATASET_PATH   override ShareGPT-format json       (default 450x12 dataset)
    NUM_CONVS      number of conversations to run     (default 450)
    MAX_MODEL_LEN  vLLM context window (tokens)       (default 8192)
    OUTPUT_TOKENS  max generated tokens per round     (default 200)
    MAX_NUM_SEQS   vLLM batch parallelism             (default 64)
    GPU_MEM_UTIL   vLLM gpu_memory_utilization        (default 0.90)
    CPU_BYTES      offload tier size (bytes)          (default 4 GiB)
    DISK_DIR       if set, add a native "fs" disk tier below the CPU tier via
                   vLLM 0.26 TieringOffloadingSpec (CPU+disk offload, the in-tree
                   replacement for SharedStorage). Empty = host-RAM-only.
    DISK_READ_THREADS / DISK_WRITE_THREADS   fs tier I/O threads (default 16/16)
    TRACE_OFFLOAD  1 = use Tracing* connector (writes offload traces);
                   0 = built-in OffloadingConnector, no tracing  (default 0)
                   (ignored when DISK_DIR is set)
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

    # TOKEN_TRACE — when set, replay a Qwen-derived synthetic *token* trace
    # (see qwen_trace_to_tokentrace.py) instead of the ShareGPT text workload.
    # Each turn's prompt is the recorded hash_ids expanded deterministically to
    # BLOCK_SIZE token-ids per hash, so the prefix-cache / offload hit pattern of
    # the original trace is reproduced exactly without the (unavailable) tokens.
    # In this mode DATASET_PATH is not needed.
    TOKEN_TRACE = os.environ.get("TOKEN_TRACE", "").strip()
    BLOCK_SIZE = int(os.environ.get("BLOCK_SIZE", 16))

    # Default to the 450-conversation / 12-turn ShareGPT workload shared with
    # the gRPC connector (certus-connector/sharegpt_12turn_450.json). Override
    # with DATASET_PATH to point at a different ShareGPT-format json.
    DEFAULT_DATASET = os.path.join(
        _here, "..", "..", "certus-connector", "sharegpt_12turn_450.json"
    )
    SUBSET_PATH = os.environ.get("DATASET_PATH", DEFAULT_DATASET)
    if not TOKEN_TRACE and not os.path.exists(SUBSET_PATH):
        print(f"[run] missing {SUBSET_PATH}", file=sys.stderr)
        sys.exit(1)

    NUM_CONVS = int(os.environ.get("NUM_CONVS", 450))
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 200))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    CPU_BYTES = int(os.environ.get("CPU_BYTES", 4 * (1 << 30)))
    MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")
    MAX_ROUNDS = int(os.environ.get("MAX_ROUNDS", 0))  # 0 = until convs exhausted

    # Deterministic hash -> token-id expansion, shared by all token-trace
    # drivers. splitmix64 keyed by (hash_id * BLOCK_SIZE + j) makes the j-th
    # token of a block a pure function of the hash_id alone, so identical
    # hash_ids expand identically everywhere (block 0 = shared system prompt,
    # shared leading blocks across turns => real prefix-cache hits).
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

    # DISK_DIR — when set, add a filesystem (disk) secondary tier below the CPU
    # tier via vLLM 0.26's native TieringOffloadingSpec + "fs" tier. This is the
    # in-tree CPU+disk offload path that replaces the (0.26-broken) SharedStorage
    # llmd_fs_backend connector: CPU_BYTES is the CPU primary tier, DISK_DIR is
    # an unbounded on-disk KV tier. Empty (default) = host-RAM-only CPUOffload.
    DISK_DIR = os.environ.get("DISK_DIR", "").strip()
    DISK_READ_THREADS = int(os.environ.get("DISK_READ_THREADS", 16))
    DISK_WRITE_THREADS = int(os.environ.get("DISK_WRITE_THREADS", 16))

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS
    print(f"[run] model={MODEL}", file=sys.stderr)
    print(f"[run] num_convs={NUM_CONVS} max_model_len={MAX_MODEL_LEN} "
          f"output_tokens={OUTPUT_TOKENS} max_num_seqs={MAX_NUM_SEQS}",
          file=sys.stderr)
    print(f"[run] prompt_budget={PROMPT_BUDGET} tokens (max_model_len - output)",
          file=sys.stderr)
    print(f"[run] cpu_offload_bytes={CPU_BYTES}", file=sys.stderr)

    # ── Load workload ─────────────────────────────────────────────────────
    # Token-trace mode replays recorded hash_id prefixes; ShareGPT mode extracts
    # cumulative human-turn text streams (the original path).
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
        print(f"[run] loaded {len(convs)} conversations  "
              f"(human-turn count: min={min(len(c) for c in convs)} "
              f"median={sorted(len(c) for c in convs)[len(convs)//2]} "
              f"max={max(len(c) for c in convs)})",
              file=sys.stderr)

    # ── Init vLLM with the CPU-offload connector ──────────────────────────
    # Default is vLLM's built-in OffloadingConnector + CPUOffloadingSpec (no
    # tracing) — this is the clean baseline for comparing against the Certus
    # gRPC backend, which also uses the plain OffloadingConnector. Set
    # TRACE_OFFLOAD=1 to instead use the local Tracing* wrappers, which record
    # per-op offload traces (offloading_mgr_<pid>.jsonl etc.) at some overhead.
    if DISK_DIR:
        # CPU + disk offload via vLLM 0.26's native multi-tier framework.
        # OffloadingConnector -> TieringOffloadingSpec: CPU primary tier
        # (cpu_bytes_to_use) with an "fs" secondary tier rooted at DISK_DIR
        # (FileSystemTierManager, registered in SecondaryTierFactory). Both are
        # registered by name in vLLM, so no *_module_path is needed. This is the
        # in-tree replacement for the SharedStorage llmd_fs_backend connector.
        os.makedirs(DISK_DIR, exist_ok=True)
        KV_CONFIG = {
            "kv_connector": "OffloadingConnector",
            "kv_role": "kv_both",
            "kv_connector_extra_config": {
                "cpu_bytes_to_use": CPU_BYTES,
                "spec_name": "TieringOffloadingSpec",
                "eviction_policy": "lru",
                "secondary_tiers": [
                    {
                        "type": "fs",
                        "root_dir": DISK_DIR,
                        "n_read_threads": DISK_READ_THREADS,
                        "n_write_threads": DISK_WRITE_THREADS,
                    }
                ],
            },
        }
        print(f"[run] disk tier (fs) root_dir={DISK_DIR} "
              f"read_threads={DISK_READ_THREADS} write_threads={DISK_WRITE_THREADS}",
              file=sys.stderr)
    elif os.environ.get("TRACE_OFFLOAD", "0") == "1":
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
    else:
        # CPUOffloadingSpec is registered by name in vLLM's OffloadingSpecFactory,
        # so no spec_module_path is needed.
        KV_CONFIG = {
            "kv_connector": "OffloadingConnector",
            "kv_role": "kv_both",
            "kv_connector_extra_config": {
                "cpu_bytes_to_use": CPU_BYTES,
                "spec_name": "CPUOffloadingSpec",
                "eviction_policy": "lru",
            },
        }
    print(f"[run] kv_connector={KV_CONFIG['kv_connector']} "
          f"(TRACE_OFFLOAD={os.environ.get('TRACE_OFFLOAD', '0')})", file=sys.stderr)

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
        # block_size aligns the KV-block granularity to the trace's hash-block
        # size (16) so one recorded Qwen block == one vLLM KV block; harmless in
        # ShareGPT mode (16 is vLLM's default).
        block_size=BLOCK_SIZE,
        kv_transfer_config=KV_CONFIG,
        # OffloadingConnector requirements (both CPUOffloadingSpec and
        # TieringOffloadingSpec run under it). These mirror the gRPC connector's
        # compat layer, which sets both for the same reasons:
        #   * disable_hybrid_kv_cache_manager: the connector assumes a single,
        #     uniform KV-cache group.
        #   * async_scheduling=False: 0.22+ auto-enables async scheduling, which
        #     races the connector's per-request transfer serialization. Under the
        #     TieringOffloadingSpec store path this surfaces as a KeyError in
        #     manager._req_state (prepare_store) -> EngineDeadError. The lighter
        #     workloads tolerated the default; deep multi-turn traces do not.
        disable_hybrid_kv_cache_manager=True,
        async_scheduling=False,
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

    # ── Token-trace replay path ───────────────────────────────────────────
    # Each round k drives turn k of every session that still has one. A turn's
    # prompt is the recorded hash_ids expanded to token-ids (NOT accumulated
    # from vLLM's own output): because hash_ids is the full cumulative input and
    # identical hashes expand identically, shared leading blocks across turns
    # and sessions produce real prefix-cache/offload hits. Output is generated
    # with ignore_eos for exactly the recorded output_length so per-turn compute
    # and the KV pressure it creates match the original trace.
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
            "cpu_bytes_to_use": CPU_BYTES,
            "disk_dir": DISK_DIR or None,
            "tier": "cpu+disk" if DISK_DIR else "cpu",
        }
        with open(os.path.join(_here, "sharegpt_multiturn_results.json"), "w") as f:
            json.dump(summary, f, indent=2)
        print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} "
              f"rounds={rounds_done}  out_tokens={total_out_tokens}  "
              f"tok/s={tok_s:.0f}", file=sys.stderr)
        sys.exit(0)

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
        if MAX_ROUNDS and rounds_done >= MAX_ROUNDS:
            break
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
        "disk_dir": DISK_DIR or None,
        "tier": "cpu+disk" if DISK_DIR else "cpu",
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
