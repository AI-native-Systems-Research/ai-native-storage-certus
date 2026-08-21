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

    # Capturing vLLM's Prometheus counters requires the engine's stat logging to
    # be on (it's what advances the metrics). Enabling it has a small overhead, so
    # a capture run is not byte-identical to the stats-off timing baseline —
    # CAPTURE_METRICS=0 restores that baseline and skips the per-round snapshot.
    # Either CAPTURE_METRICS or LOG_STATS turns the stat logger on.
    CAPTURE_METRICS = os.environ.get("CAPTURE_METRICS", "1") != "0"
    _log_stats_on = os.environ.get("LOG_STATS", "0") != "0"

    # Model FLOPs Utilization (MFU): with enable_mfu_metrics the engine emits
    # estimated per-GPU FLOPs / read-bytes / write-bytes as monotonic Counters
    # (vllm:estimated_flops_per_gpu_total et al.); average TFLOP/s per GPU is the
    # per-interval delta / seconds / 1e12. They're plain prometheus Counters, so
    # the existing per-round prom_counters() snapshot captures them automatically
    # once the flag is on — no separate sampling path needed. The arg only exists
    # on newer vLLM, so detect support at runtime rather than guessing by version.
    _mfu_on = False
    if CAPTURE_METRICS:
        try:
            import dataclasses as _dc
            from vllm.engine.arg_utils import EngineArgs as _EA
            if any(f.name == "enable_mfu_metrics" for f in _dc.fields(_EA)):
                _engine_kwargs["enable_mfu_metrics"] = True
                _mfu_on = True
        except Exception as e:  # noqa: BLE001
            print(f"[prom] enable_mfu_metrics unavailable: {e}", file=sys.stderr)
        print(f"[prom] enable_mfu_metrics={_mfu_on} "
              f"(estimated_flops_per_gpu_total {'captured per round' if _mfu_on else 'ABSENT'})",
              file=sys.stderr)

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
        # per-round output clean; per-round SSD device-byte deltas ride the
        # [prom] line below (see the GetIoStats snapshot).
        disable_log_stats=not (CAPTURE_METRICS or _log_stats_on),
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

    # ── Per-round SSD device I/O (Certus rw-telemetry over the shmq ring) ──
    # The server exposes its cumulative NVMe read/write byte counters via the
    # ring's GetIoStats op (translate.rs op_get_io_stats -> dispatcher
    # read_write_stats). Attach our own Ring to the same mailbox — Ring(path)
    # mmaps the segment and claims its own sticky channel, independent of the
    # connector's worker channels — and snapshot it around each round to print
    # per-round ssd_read_bytes / ssd_write_bytes deltas on the [prom] line, which
    # the kvprofile renderer plots. Real only in an --features rw-telemetry
    # server build; otherwise the counters stay zero and the renderer drops the
    # empty series. (Restores what the gRPC driver did, now over shm-queue.)
    from certus_shmq_connector.ring import Ring

    try:
        io_ring = Ring(SHM_PATH)
    except Exception as e:  # noqa: BLE001
        io_ring = None
        print(f"[io] GetIoStats unavailable ({e}); per-round SSD bytes disabled",
              file=sys.stderr)

    def io_rw_bytes():
        """(read_bytes, write_bytes) cumulative from the server, or (None, None)."""
        if io_ring is None:
            return None, None
        try:
            s = io_ring.get_io_stats()
            return int(s["read_bytes"]), int(s["write_bytes"])
        except Exception as e:  # noqa: BLE001
            print(f"[io] GetIoStats query failed: {e}", file=sys.stderr)
            return None, None

    # ── vLLM Prometheus counters (per round) ──────────────────────────────
    # vLLM registers every metric on the default prometheus_client REGISTRY under
    # the `vllm:` prefix, updated as the engine steps (present on both the V0 and
    # V1 engines whenever stat logging is on). Snapshot each counter (samples
    # named `vllm:*_total`, summed across label sets) at the end of every round
    # and log the delta; the full per-round series is also dumped to JSON.
    def prom_counters():
        # Prefer the V1 offline snapshot: engine counters (prompt/generation
        # tokens, prefix-cache queries/hits, preemptions) are exposed by
        # llm.get_metrics() and are NOT on the global prometheus REGISTRY in
        # offline mode — reading only the REGISTRY (or the log_stats logger) misses
        # them. Then supplement with the REGISTRY for older (V0) engines, where
        # get_metrics() is absent, and for connector-registered metrics
        # (vllm:kv_offload_*) which only ever live on the global registry.
        # Names differ by source: get_metrics() uses bare names
        # (vllm:prefix_cache_queries); REGISTRY counter samples carry the _total
        # suffix — we keep both keys so the delta shows under whichever the
        # running version populates.
        vals = {}
        if not CAPTURE_METRICS:
            return vals
        try:
            for m in llm.get_metrics():
                if type(m).__name__ != "Counter":
                    continue
                name = getattr(m, "name", "")
                val = getattr(m, "value", None)
                if name.startswith("vllm:") and isinstance(val, (int, float)):
                    # Sum across label sets: get_metrics() emits one Counter per
                    # sample, so labeled metrics (request_success by finish_reason,
                    # prompt_tokens_by_source, per-engine under TP>1) share a name.
                    vals[name] = vals.get(name, 0.0) + float(val)
        except Exception:  # noqa: BLE001 - get_metrics() is V1-only; skip on V0
            pass
        try:
            from prometheus_client import REGISTRY
            for metric in REGISTRY.collect():
                if not metric.name.startswith("vllm:"):
                    continue
                for s in metric.samples:
                    if s.name.endswith("_total"):
                        vals[s.name] = vals.get(s.name, 0.0) + float(s.value)
        except Exception as e:  # noqa: BLE001
            print(f"[prom] collect failed: {e}", file=sys.stderr, flush=True)
        return vals

    def prom_histograms(names):
        # Sample the named vLLM latency histograms ONCE (cumulative over the whole
        # run). get_metrics() exposes each as Histogram(count, sum, buckets) where
        # buckets maps an upper bound `le` -> cumulative count <= le; sum across
        # label sets (per-engine / finish-reason) into one distribution per name.
        out = {}
        if not CAPTURE_METRICS:
            return out
        try:
            for m in llm.get_metrics():
                if type(m).__name__ != "Histogram":
                    continue
                name = getattr(m, "name", "")
                if name not in names:
                    continue
                agg = out.setdefault(name, {"count": 0, "sum": 0.0, "buckets": {}})
                agg["count"] += int(getattr(m, "count", 0))
                agg["sum"] += float(getattr(m, "sum", 0.0))
                for le, c in (getattr(m, "buckets", {}) or {}).items():
                    agg["buckets"][le] = agg["buckets"].get(le, 0) + int(c)
        except Exception as e:  # noqa: BLE001 - get_metrics() is V1-only
            print(f"[prom] histogram sample failed: {e}", file=sys.stderr)
        return out

    def _hist_pct(buckets, count, p):
        # Percentile from cumulative buckets: the smallest upper bound `le` whose
        # cumulative count first reaches p*count. Bucket-granular approximation;
        # returns inf when the crossing lands in the +Inf bucket.
        if not count:
            return None
        target = p * count

        def _le(k):
            return float("inf") if k in ("+Inf", "inf", "Inf") else float(k)

        for le in sorted(buckets, key=_le):
            if buckets[le] >= target:
                return _le(le)
        return float("inf")

    prom_prev = prom_counters()
    prom_rounds = []  # (round, {counter_name: delta})
    io_prev = io_rw_bytes()  # cumulative SSD device bytes before round 1

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

        if CAPTURE_METRICS:
            prom_now = prom_counters()
            d_prom = {k: prom_now.get(k, 0.0) - prom_prev.get(k, 0.0)
                      for k in prom_now}
            prom_prev = prom_now
            prom_rounds.append((rounds_done, d_prom))
            vllm_shown = " ".join(f"{k[len('vllm:'):]}={d_prom[k]:.0f}"
                                  for k in sorted(d_prom) if d_prom[k])
            # Per-round SSD device byte deltas from the server (Certus rw-telemetry
            # via GetIoStats). Emitted as ssd_read_bytes / ssd_write_bytes so the
            # kvprofile renderer picks them up on the same [prom] line.
            ssd_shown = ""
            io_now = io_rw_bytes()
            if io_now[0] is not None and io_prev[0] is not None:
                ssd_shown = (f"ssd_read_bytes={io_now[0] - io_prev[0]} "
                             f"ssd_write_bytes={io_now[1] - io_prev[1]}")
            if io_now[0] is not None:
                io_prev = io_now
            shown = " ".join(x for x in (vllm_shown, ssd_shown) if x)
            print(f"[prom] round {rounds_done}: {shown or '(no counter movement)'}",
                  file=sys.stderr, flush=True)

    elapsed = time.perf_counter() - t_start
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
        hists = prom_histograms({"vllm:request_queue_time_seconds",
                                 "vllm:request_decode_time_seconds"})
        for name, h in sorted(hists.items()):
            cnt, tot = h["count"], h["sum"]
            mean = tot / cnt if cnt else 0.0
            fmt = lambda x: "n/a" if x is None else f"{x:.3f}s"  # noqa: E731
            print(f"[prom] hist {name[len('vllm:'):]}: n={cnt} mean={mean:.3f}s "
                  f"p50={fmt(_hist_pct(h['buckets'], cnt, 0.50))} "
                  f"p90={fmt(_hist_pct(h['buckets'], cnt, 0.90))} "
                  f"p99={fmt(_hist_pct(h['buckets'], cnt, 0.99))}",
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

    print(
        f"[run] DONE rounds={rounds_done} generations={total_generations} "
        f"elapsed={elapsed:.1f}s ({total_generations / elapsed:.1f} gen/s)",
        file=sys.stderr,
    )
