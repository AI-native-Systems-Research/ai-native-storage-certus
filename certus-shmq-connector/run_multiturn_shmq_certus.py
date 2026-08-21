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

Per-round SSD device I/O: the server's cumulative NVMe read/write byte counters
(its rw-telemetry) are queried over the shmq ring's GetIoStats op each round and
printed as ssd_read_bytes / ssd_write_bytes deltas on the [prom] line, which the
kvprofile renderer plots. This is the same mechanism the old gRPC driver used
(its GetIoStats RPC), now carried over shm-queue. The byte counts are real only
when the server is built with --features rw-telemetry (zero otherwise).
"""

if __name__ == "__main__":
    import json
    import os
    import sys
    import time

    _here = os.path.dirname(os.path.abspath(__file__))
    if _here not in sys.path:
        sys.path.insert(0, _here)
    # The shared workload module lives in the benchmarks dir, not here.
    _bench_dir = os.path.join(_here, "..", "benchmarks", "kv-offload-replay")
    if _bench_dir not in sys.path:
        sys.path.insert(0, _bench_dir)
    import run_multiturn_common as common
    import run_multiturn_sync_batched as batched

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

    convs = common.load_convs(DATASET_PATH, NUM_CONVS, CONV_MULTIPLIER)
    # load_convs returns the replicated set; report the base count first (as the
    # inline loop did), then the replicated total.
    _base_convs = len(convs) // CONV_MULTIPLIER if CONV_MULTIPLIER > 1 else len(convs)
    print(f"[run] loaded {_base_convs} conversations", file=sys.stderr)
    if CONV_MULTIPLIER > 1:
        print(
            f"[run] replicated x{CONV_MULTIPLIER} -> {len(convs)} conversations",
            file=sys.stderr,
        )

    from vllm import SamplingParams

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
        # Orthogonal to WORKLOAD_MODE=async (which switches the request-submission
        # API, not the scheduler) — this stays set in both modes.
        _engine_kwargs["async_scheduling"] = False
        print(
            f"[run] vLLM {_VLLM_VERSION[0]}.{_VLLM_VERSION[1]}: "
            f"async_scheduling=False (OffloadingConnector serializes transfers per request)",
            file=sys.stderr,
        )

    # WORKLOAD_MODE=async runs one vLLM coroutine per conversation (V1 AsyncLLM);
    # "batched" (default) runs the synchronous per-round generate loop. Both share
    # the engine_kwargs below (same CertusShmq kv_transfer_config + compat flags).
    WORKLOAD_MODE = os.environ.get("WORKLOAD_MODE", "batched").strip().lower()
    # LOG_STATS gates the PrometheusStatLogger; async metrics read the global
    # REGISTRY, so counters only populate when LOG_STATS=1 (disable_log_stats off).
    _log_stats_off = os.environ.get("LOG_STATS", "0") == "0"

    engine_kwargs = dict(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        tensor_parallel_size=TENSOR_PARALLEL_SIZE,
        pipeline_parallel_size=PIPELINE_PARALLEL_SIZE,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype=os.environ.get("DTYPE", "float16"),
        enable_prefix_caching=True,
        enforce_eager=(os.environ.get("ENFORCE_EAGER", "0") != "0"),
        # KV_CACHE_DTYPE="fp8" stores KV-cache blocks in 8-bit, halving the
        # per-sequence KV footprint so larger MAX_NUM_SEQS fits before OOM.
        # Default "auto" = same as model dtype (fp16 here).
        kv_cache_dtype=os.environ.get("KV_CACHE_DTYPE", "auto"),
        kv_transfer_config=KV_CONFIG,
        # LOG_STATS=1 surfaces vLLM's periodic engine stats, including the
        # OffloadingConnector's KVConnectorStats (per-interval blocks/tokens
        # loaded and stored over the KV-offload API). Default off to keep the
        # per-round output clean.
        disable_log_stats=_log_stats_off,
        **_engine_kwargs,
    )

    sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=OUTPUT_TOKENS)

    print("Running across ", TENSOR_PARALLEL_SIZE, " GPUs")

    # Tag each request with its conversation as the KV-offload session_id. The
    # conversation index is stable across rounds, so every turn of the same
    # conversation shares one session_id; the connector forwards it (hashed to
    # u64) on Reserve -> the dispatcher logs it. +1 so conversation 0 gets a
    # non-zero id (0 == "unset" sentinel).
    _session_id_fn = lambda i: i + 1  # noqa: E731

    if WORKLOAD_MODE == "async":
        import run_multiturn_async as async_run

        summary = async_run.run_async_driver(
            engine_kwargs, convs, sp,
            prompt_budget=PROMPT_BUDGET,
            max_rounds=MAX_ROUNDS,
            capture_metrics=(not _log_stats_off),
            session_id_fn=_session_id_fn,
            skip_empty=True,
            summary_base={
                "model": MODEL, "shm_path": SHM_PATH,
                "max_model_len": MAX_MODEL_LEN, "output_tokens": OUTPUT_TOKENS,
                "num_convs": _base_convs, "conv_multiplier": CONV_MULTIPLIER,
                "backend": "certus-shmq",
            },
        )
        rounds_done = summary["num_rounds"]
        total_generations = summary["total_generations"]
        elapsed = summary["elapsed_time"]
        try:
            with open(os.path.join(_here, "shmq_async_results.json"), "w") as f:
                json.dump(summary, f, indent=2)
        except OSError as e:
            print(f"[run] could not save async results: {e}", file=sys.stderr)
    else:
        llm = common.build_engine(engine_kwargs, async_mode=False)
        common.start_prom_exporter()

        tokenizer = llm.get_tokenizer()
        n_tokens = common.make_n_tokens(tokenizer)

        CAPTURE_METRICS = not _log_stats_off

        # Per-round SSD device I/O. The server's cumulative NVMe read/write byte
        # counters (its rw-telemetry) are queried over the shmq ring's GetIoStats
        # op (translate.rs op_get_io_stats -> dispatcher read_write_stats) and the
        # per-round deltas are emitted as ssd_read_bytes / ssd_write_bytes on the
        # [prom] line, which the kvprofile renderer plots. Same mechanism the old
        # gRPC driver used (its GetIoStats RPC), now carried over shm-queue. Byte
        # counts are real only when the server is built with --features
        # rw-telemetry (zero otherwise).
        from certus_shmq_connector.ring import Ring

        try:
            io_ring = Ring(SHM_PATH)
        except Exception as e:  # noqa: BLE001 - ring may be absent; degrade gracefully
            io_ring = None
            print(f"[io] GetIoStats unavailable ({e}); per-round SSD bytes disabled",
                  file=sys.stderr)

        def io_rw_bytes():
            """(cumulative read_bytes, write_bytes) from the server, or (None, None)."""
            if io_ring is None:
                return None, None
            try:
                s = io_ring.get_io_stats()
                return int(s["read_bytes"]), int(s["write_bytes"])
            except Exception as e:  # noqa: BLE001
                print(f"[io] GetIoStats query failed: {e}", file=sys.stderr)
                return None, None

        # ── vLLM Prometheus counters + SSD device bytes (per round) ───────────
        # prom counters snapshot at round end; SSD device bytes bracketed around
        # generate() (snapshot pre in on_round_start, diff post in on_round_end).
        prom_prev = [common.prom_counters(llm, CAPTURE_METRICS)]
        prom_rounds = []  # (round, {counter_name: delta})
        io_pre = [None, None]

        def on_round_start(round_idx, n_prompts):
            io_pre[0], io_pre[1] = io_rw_bytes()

        def on_round_end(round_idx, n_prompts, round_elapsed, n_alive):
            rd0, wr0 = io_pre
            rd1, wr1 = io_rw_bytes()
            ssd_shown = ""
            if rd0 is not None and rd1 is not None:
                ssd_shown = (f"ssd_read_bytes={rd1 - rd0} "
                             f"ssd_write_bytes={wr1 - wr0}")
            print(f"[run] round {round_idx}: {n_prompts} prompts in "
                  f"{round_elapsed:.1f}s  ({n_alive} convs still alive)",
                  file=sys.stderr, flush=True)
            if CAPTURE_METRICS:
                prom_now = common.prom_counters(llm, CAPTURE_METRICS)
                d_prom = {k: prom_now.get(k, 0.0) - prom_prev[0].get(k, 0.0)
                          for k in prom_now}
                prom_prev[0] = prom_now
                prom_rounds.append((round_idx, d_prom))
                shown = " ".join(f"{k[len('vllm:'):]}={d_prom[k]:.0f}"
                                 for k in sorted(d_prom) if d_prom[k])
            else:
                shown = ""
            line = " ".join(x for x in (shown, ssd_shown) if x)
            print(f"[prom] round {round_idx}: {line or '(no counter movement)'}",
                  file=sys.stderr, flush=True)

        result = batched.run_batched(
            llm, convs, sp,
            prompt_budget=PROMPT_BUDGET,
            max_rounds=MAX_ROUNDS,
            n_tokens=n_tokens,
            skip_empty=True,
            session_id_fn=_session_id_fn,
            on_round_start=on_round_start,
            on_round_end=on_round_end,
        )
        rounds_done = result["rounds_done"]
        total_generations = result["total_generations"]
        elapsed = result["elapsed"]

        if CAPTURE_METRICS and prom_rounds:
            try:
                with open(os.path.join(_here, "prom_counters_rounds.json"), "w") as f:
                    json.dump([{"round": r, "counters": d} for r, d in prom_rounds],
                              f, indent=2)
            except OSError as e:
                print(f"[prom] could not save json: {e}", file=sys.stderr)

        # Latency-distribution histograms: sampled once (cumulative over the run).
        if CAPTURE_METRICS:
            hists = common.prom_histograms(llm, {"vllm:request_queue_time_seconds",
                                                 "vllm:request_decode_time_seconds"},
                                           CAPTURE_METRICS)
            for name, h in sorted(hists.items()):
                cnt, tot = h["count"], h["sum"]
                mean = tot / cnt if cnt else 0.0
                fmt = lambda x: "n/a" if x is None else f"{x:.3f}s"  # noqa: E731
                print(f"[prom] hist {name[len('vllm:'):]}: n={cnt} mean={mean:.3f}s "
                      f"p50={fmt(common.hist_pct(h['buckets'], cnt, 0.50))} "
                      f"p90={fmt(common.hist_pct(h['buckets'], cnt, 0.90))} "
                      f"p99={fmt(common.hist_pct(h['buckets'], cnt, 0.99))}",
                      file=sys.stderr, flush=True)
            if hists:
                print(f"[prom] histjson {json.dumps(hists, separators=(',', ':'))}",
                      file=sys.stderr, flush=True)
                try:
                    with open(os.path.join(_here, "prom_histograms.json"), "w") as f:
                        json.dump(hists, f, indent=2)
                except OSError as e:
                    print(f"[prom] could not save histogram json: {e}", file=sys.stderr)

    print(
        f"[run] DONE rounds={rounds_done} generations={total_generations} "
        f"elapsed={elapsed:.1f}s ({total_generations / elapsed:.1f} gen/s)",
        file=sys.stderr,
    )
