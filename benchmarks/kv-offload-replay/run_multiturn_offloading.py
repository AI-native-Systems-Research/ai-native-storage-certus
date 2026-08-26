#!/usr/bin/env python3
"""run_multiturn_offloading.py — drive multi-turn chat through vLLM offline
inference with the CPU-offload KV connector under a realistic prefix-growing
chat workload.

By default this uses vLLM's built-in OffloadingConnector + CPUOffloadingSpec
(host-RAM offload tier, no tracing) — the clean baseline for comparing against
the Certus shmq backend, which also uses the plain OffloadingConnector. Set
TRACE_OFFLOAD=1 to swap in the local Tracing* wrappers, which additionally
record per-op offload traces (offloading_mgr_<pid>.jsonl etc.) at some overhead.

For each ShareGPT conversation, the driver runs one generation per human turn,
batched across all active conversations per round. Round k's prompt for a
conversation is the cumulative concatenation of every prior human turn,
every prior vLLM-generated response (NOT the dataset's gpt response — using
vLLM's own output is what makes prefix tokens match exactly turn-to-turn so
the offload cache sees real read-path traffic), and the k'th human turn.

Defaults to the 450-conversation / 12-turn ShareGPT dataset shared with the
Certus connector (../../data/sharegpt_12turn_450.json).

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

    import run_multiturn_common as common
    import run_multiturn_sync_batched as batched

    # Default to the 450-conversation / 12-turn ShareGPT workload shared with
    # the Certus connector (data/sharegpt_12turn_450.json). Override
    # with DATASET_PATH to point at a different ShareGPT-format json.
    DEFAULT_DATASET = os.path.join(
        _here, "..", "..", "data", "sharegpt_12turn_450.json"
    )
    # WORKLOAD_NAME=<name> selects a registered workload (e.g. WORKLOAD_NAME=sharegpt
    # -> the ShareGPT multi-turn subset by human-turn count, default 12/12 = the
    # 450x12 set); DATASET_PATH / NUM_CONVS still override.
    SUBSET_PATH, NUM_CONVS = common.resolve_workload(DEFAULT_DATASET, 450)
    if not os.path.exists(SUBSET_PATH):
        print(f"[run] missing {SUBSET_PATH}", file=sys.stderr)
        sys.exit(1)
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
    OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 200))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    # ACTIVE_SESSIONS — WORKLOAD_MODE=async only. >0 = closed loop: keep this many
    # conversations active, admitting the next as one finishes (steady-state
    # concurrency). 0 (default) = open loop (all launched at once, max_num_seqs
    # bounds the running batch). Keep <= MAX_NUM_SEQS so the driver is the gate.
    ACTIVE_SESSIONS = int(os.environ.get("ACTIVE_SESSIONS", 0))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    CPU_BYTES = int(os.environ.get("CPU_BYTES", 4 * (1 << 30)))
    MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")
    MAX_ROUNDS = int(os.environ.get("MAX_ROUNDS", 0))  # 0 = until convs exhausted

    # OFFLOAD_MODE — top-level backend selector for the unified image. "none" runs
    # the GPU-only baseline (no kv_transfer_config); any other value (incl. the
    # default empty) uses an offload tier, whose kind is picked by the DISK_DIR /
    # SECONDARY_TIER / TRACE_OFFLOAD selectors below (empty => host-RAM CPUOffload,
    # SECONDARY_TIER=fs / DISK_DIR set => CPU+FS Tiered). This lets one image drive
    # NoOffload, CPUOffload, and Tiered by env alone.
    OFFLOAD_MODE = os.environ.get("OFFLOAD_MODE", "").strip().lower()

    # DISK_DIR — when set, add a filesystem (disk) secondary tier below the CPU
    # tier via vLLM 0.26's native TieringOffloadingSpec + "fs" tier. This is the
    # in-tree CPU+disk offload path that replaces the (0.26-broken) SharedStorage
    # llmd_fs_backend connector: CPU_BYTES is the CPU primary tier, DISK_DIR is
    # an unbounded on-disk KV tier. Empty (default) = host-RAM-only CPUOffload.
    DISK_DIR = os.environ.get("DISK_DIR", "").strip()
    DISK_READ_THREADS = int(os.environ.get("DISK_READ_THREADS", 16))
    DISK_WRITE_THREADS = int(os.environ.get("DISK_WRITE_THREADS", 16))

    # Secondary offload tier. "" (default) = CPU-only (CPUOffloadingSpec, host RAM).
    # "fs" = vLLM's native TieringOffloadingManager with the CPU tier as PRIMARY and
    # a filesystem tier as SECONDARY (spills overflow blocks to FS_ROOT_DIR). CPU_BYTES
    # then sizes only the primary tier; blocks the primary evicts fall through to disk.
    SECONDARY_TIER = os.environ.get("SECONDARY_TIER", "").strip().lower()
    FS_ROOT_DIR = os.environ.get("FS_ROOT_DIR", "/mnt/fs-tier/kv-tier")
    FS_READ_THREADS = int(os.environ.get("FS_READ_THREADS", 16))
    FS_WRITE_THREADS = int(os.environ.get("FS_WRITE_THREADS", 16))

    # ── Per-round physical disk I/O accounting ────────────────────────────
    # /sys/block/<dev>/stat exposes cumulative sectors read (field 3) and
    # written (field 7) in 512-byte units. Snapshotting around each generate()
    # gives bytes moved to/from the device per round. For Tiered-CPU-FS the
    # fs secondary tier lives on DISK_DEV (the RAID0 md), so this captures the
    # real SSD read/write of the spill tier; for CPU-only offload it stays ~0.
    # Reading the md device aggregates all RAID0 member I/O in one place.
    DISK_DEV = os.environ.get("DISK_DEV", "").strip()
    DISK_STAT = f"/sys/block/{DISK_DEV}/stat" if DISK_DEV else ""

    def disk_rw_bytes():
        """Return (bytes_read, bytes_written) cumulative for DISK_DEV, or (None, None)."""
        return common.disk_rw_bytes(DISK_STAT)

    gib = common.gib

    if DISK_DEV and disk_rw_bytes()[1] is None:
        print(f"[run] WARNING: {DISK_STAT} unreadable — per-round disk bytes disabled",
              file=sys.stderr, flush=True)
    elif not DISK_DEV:
        print("[run] DISK_DEV unset — per-round disk I/O accounting disabled",
              file=sys.stderr, flush=True)

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS
    print(f"[run] model={MODEL}", file=sys.stderr)
    print(f"[run] num_convs={NUM_CONVS} max_model_len={MAX_MODEL_LEN} "
          f"output_tokens={OUTPUT_TOKENS} max_num_seqs={MAX_NUM_SEQS}",
          file=sys.stderr)
    print(f"[run] prompt_budget={PROMPT_BUDGET} tokens (max_model_len - output)",
          file=sys.stderr)
    print(f"[run] cpu_offload_bytes={CPU_BYTES}", file=sys.stderr)

    # ── Load conversations and extract human-turn streams ─────────────────
    convs = common.load_convs(SUBSET_PATH, NUM_CONVS)
    print(f"[run] loaded {len(convs)} conversations  "
          f"(human-turn count: min={min(len(c) for c in convs)} "
          f"median={sorted(len(c) for c in convs)[len(convs)//2]} "
          f"max={max(len(c) for c in convs)})",
          file=sys.stderr)

    # ── Init vLLM with the CPU-offload connector ──────────────────────────
    # Default is vLLM's built-in OffloadingConnector + CPUOffloadingSpec (no
    # tracing) — this is the clean baseline for comparing against the Certus
    # shmq backend, which also uses the plain OffloadingConnector. Set
    # TRACE_OFFLOAD=1 to instead use the local Tracing* wrappers, which record
    # per-op offload traces (offloading_mgr_<pid>.jsonl etc.) at some overhead.
    if OFFLOAD_MODE == "none":
        # GPU-only baseline: no offload tier at all. Evicted KV is recomputed on
        # the GPU rather than fetched from a tier. kv_transfer_config=None is how
        # vLLM expresses "no connector", so the LLM(...) call below passes None.
        KV_CONFIG = None
        print("[run] OFFLOAD_MODE=none — GPU-only baseline (no kv_transfer_config)",
              file=sys.stderr)
    elif DISK_DIR:
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
    elif SECONDARY_TIER == "fs":
        # Tiered: CPU primary + filesystem secondary. TieringOffloadingSpec is a
        # CPUOffloadingSpec subclass registered by name in vLLM's
        # OffloadingSpecFactory (vLLM >= 0.26); "fs" resolves to FileSystemTierManager
        # via the SecondaryTierFactory. CPU_BYTES sizes the primary tier; primary
        # evictions spill to FS_ROOT_DIR. root_dir must exist and be writable.
        os.makedirs(FS_ROOT_DIR, exist_ok=True)
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
                        "root_dir": FS_ROOT_DIR,
                        "n_read_threads": FS_READ_THREADS,
                        "n_write_threads": FS_WRITE_THREADS,
                    },
                ],
            },
        }
        print(f"[run] secondary_tier=fs root_dir={FS_ROOT_DIR} "
              f"read_threads={FS_READ_THREADS} write_threads={FS_WRITE_THREADS}",
              file=sys.stderr)
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
    if KV_CONFIG is not None:
        print(f"[run] kv_connector={KV_CONFIG['kv_connector']} "
              f"spec={KV_CONFIG['kv_connector_extra_config'].get('spec_name')} "
              f"(TRACE_OFFLOAD={os.environ.get('TRACE_OFFLOAD', '0')})", file=sys.stderr)

    # Clear stale trace files
    for f in os.listdir(_here):
        if (f.startswith("offloading_trace_")
                or f.startswith("offloading_mgr_")
                or f.startswith("offloading_handler_")) \
                and f.endswith(".jsonl"):
            os.remove(os.path.join(_here, f))

    from vllm import SamplingParams

    # Capturing vLLM's Prometheus counters requires the engine's stat logging to
    # be on (it's what advances the metrics). Enabling it has a small overhead, so
    # a capture run is not byte-identical to the stats-off timing baseline —
    # CAPTURE_METRICS=0 restores that baseline and skips the per-round snapshot.
    CAPTURE_METRICS = os.environ.get("CAPTURE_METRICS", "1") != "0"

    # WORKLOAD_MODE=async runs one vLLM coroutine per conversation (V1 AsyncLLM;
    # concurrency bounded by max_num_seqs, the rest queue in WAITING). "batched"
    # (default) runs the synchronous per-round generate loop. Both share the
    # engine_kwargs below — the backend offload config is not duplicated.
    WORKLOAD_MODE = os.environ.get("WORKLOAD_MODE", "batched").strip().lower()

    # Model FLOPs Utilization probe (shared): adds enable_mfu_metrics iff the
    # running vLLM's EngineArgs accepts it, so estimated_flops_per_gpu_total et
    # al. advance and get captured per round / per sample.
    _mfu_kwargs = common.mfu_kwargs(CAPTURE_METRICS)

    engine_kwargs = dict(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        # ENFORCE_EAGER=1 disables CUDA-graph capture / torch.compile. Default 0
        # (graphs on) — matches vLLM's default and the other profile backends.
        enforce_eager=(os.environ.get("ENFORCE_EAGER", "0") != "0"),
        # async_scheduling MUST be off for the OffloadingConnector: it serializes
        # KV transfers per request, and the async batch-queue scheduler path
        # (step_with_batch_queue) trips a KeyError in the native tiering manager's
        # prepare_store (self._req_state[req_id]) — EngineDeadError at round 1.
        # ORTHOGONAL to cudagraph AND to WORKLOAD_MODE=async (that switches the
        # request-submission API, not the scheduler). Override with ASYNC_SCHED=1.
        async_scheduling=(os.environ.get("ASYNC_SCHED", "0") != "0"),
        kv_transfer_config=KV_CONFIG,
        disable_log_stats=not CAPTURE_METRICS,
        **_mfu_kwargs,
    )

    sp = SamplingParams(
        temperature=0.7,
        top_p=0.95,
        max_tokens=OUTPUT_TOKENS,
    )

    if WORKLOAD_MODE == "async":
        # One vLLM coroutine per conversation on a V1 AsyncLLM. The async
        # orchestration (engine build, 1 Hz disk+prom sampler, asyncio.run,
        # latency percentiles, summary) lives in run_multiturn_async so it isn't
        # re-forked here; this branch just supplies the backend engine_kwargs,
        # this driver's disk closure, and its summary fields.
        import run_multiturn_async as async_run

        summary = async_run.run_async_driver(
            engine_kwargs, convs, sp,
            prompt_budget=PROMPT_BUDGET,
            max_rounds=MAX_ROUNDS,
            capture_metrics=CAPTURE_METRICS,
            disk_rw_bytes=disk_rw_bytes,
            active_sessions=ACTIVE_SESSIONS,
            summary_base={
                "model": MODEL,
                "max_model_len": MAX_MODEL_LEN,
                "output_tokens": OUTPUT_TOKENS,
                "cpu_bytes_to_use": CPU_BYTES,
                "disk_dir": DISK_DIR or None,
                "tier": "cpu+disk" if DISK_DIR else "cpu",
            },
        )
        elapsed = summary["elapsed_time"]
        rounds_done = summary["num_rounds"]
        total_generations = summary["total_generations"]
    else:
        llm = common.build_engine(engine_kwargs, async_mode=False)
        common.start_prom_exporter()

        tokenizer = llm.get_tokenizer()
        n_tokens = common.make_n_tokens(tokenizer)

        # ── vLLM Prometheus counters (per round) ──────────────────────────────
        # Snapshot each vllm: counter at the end of every round and log the delta;
        # the full per-round series is also dumped to JSON. prom_counters/
        # prom_histograms/hist_pct live in run_multiturn_common (get_metrics() +
        # REGISTRY branches).
        prom_prev = [common.prom_counters(llm, CAPTURE_METRICS)]
        prom_rounds = []  # (round, {counter_name: delta})
        # Disk bytes are bracketed around generate(): snapshot in on_round_start
        # (pre-generate), diff in on_round_end (post-generate).
        disk_pre = [None, None]

        def on_round_start(round_idx, n_prompts):
            disk_pre[0], disk_pre[1] = disk_rw_bytes()

        def on_round_end(round_idx, n_prompts, round_elapsed, n_alive):
            rd0, wr0 = disk_pre
            rd1, wr1 = disk_rw_bytes()
            d_rd = None if rd0 is None or rd1 is None else rd1 - rd0
            d_wr = None if wr0 is None or wr1 is None else wr1 - wr0
            print(f"[run] round {round_idx}: {n_prompts} prompts in "
                  f"{round_elapsed:.1f}s  ({n_alive} convs still alive)  "
                  f"disk_read={gib(d_rd)} disk_write={gib(d_wr)}",
                  file=sys.stderr, flush=True)
            if CAPTURE_METRICS:
                prom_now = common.prom_counters(llm, CAPTURE_METRICS)
                d_prom = {k: prom_now.get(k, 0.0) - prom_prev[0].get(k, 0.0)
                          for k in prom_now}
                prom_prev[0] = prom_now
                prom_rounds.append((round_idx, d_prom))
                shown = " ".join(f"{k[len('vllm:'):]}={d_prom[k]:.0f}"
                                 for k in sorted(d_prom) if d_prom[k])
                print(f"[prom] round {round_idx}: {shown or '(no counter movement)'}",
                      file=sys.stderr, flush=True)

        result = batched.run_batched(
            llm, convs, sp,
            prompt_budget=PROMPT_BUDGET,
            max_rounds=MAX_ROUNDS,
            n_tokens=n_tokens,
            skip_empty=False,
            on_round_start=on_round_start,
            on_round_end=on_round_end,
        )
        elapsed = result["elapsed"]
        rounds_done = result["rounds_done"]
        total_generations = result["total_generations"]

        if CAPTURE_METRICS and prom_rounds:
            try:
                with open(os.path.join(_here, "prom_counters_rounds.json"), "w") as f:
                    json.dump([{"round": r, "counters": d} for r, d in prom_rounds],
                              f, indent=2)
            except OSError as e:
                print(f"[prom] could not save json: {e}", file=sys.stderr)

        # Latency-distribution histograms: sampled once here (cumulative over the
        # run, not per round) — queue time (WAITING) and decode time (DECODE).
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
                # Full buckets on one stderr line so the per-variant teed log is a
                # complete source (the shared-name JSON below is overwritten when
                # two variants share this driver / dir).
                print(f"[prom] histjson {json.dumps(hists, separators=(',', ':'))}",
                      file=sys.stderr, flush=True)
                try:
                    with open(os.path.join(_here, "prom_histograms.json"), "w") as f:
                        json.dump(hists, f, indent=2)
                except OSError as e:
                    print(f"[prom] could not save histogram json: {e}", file=sys.stderr)

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
