#!/usr/bin/env python3
"""run_otel_replay.py — kv-offload-otel-replay: the timestamp-scheduled sibling
of run_multiturn_offloading.py.

Where run_multiturn_offloading.py replays ShareGPT human-turn streams as fast as
the engine will take them, this driver replays the `otel_trace_replay` corpus
produced by benchmarks/kv-offload-otel-replay/trace-gen/trace_to_otel.py (default
/mnt/certus1/inference-perf-syn-data/otel_1k) and **reproduces the recorded turn
timing**: every conversation is one async coroutine, and before each turn the
coroutine sleeps the recorded inter-turn gap (start_time[k] - end_time[k-1], the
tool-call / think delay the trace encodes), so the wall-clock spacing of turns
matches the workload rather than saturating the engine. Each turn is generated to
its own recorded `gen_ai.request.max_tokens`, and context is rebuilt from prior
human turns + vLLM's own generations under a 120k-style sliding window (see
run_otel_async).

Backend selection, engine kwargs, and telemetry are IDENTICAL to
run_multiturn_offloading.py — the same OFFLOAD_MODE / DISK_DIR / SECONDARY_TIER /
TRACE_OFFLOAD env selectors build the same kv_transfer_config, so this tool drops
into the same head-to-head harness (NoOffload / CPUOffload / Tiered-CPU-FS /
Certus-shmq) and the same containers/wrappers. It always runs the async
execution model (the timing is the point); there is no batched mode.

OTel-specific options (CLI flags, with env fallbacks so container entrypoints can
drive them too):
    --dir DIR        / OTEL_DIR    corpus directory of per-conversation json files
                                   (default /mnt/certus1/inference-perf-syn-data/otel_1k)
    --num N          / NUM_CONVS   number of conversations to replay
                                   (default: ALL files in --dir)
    --time-scale F   / TIME_SCALE  multiply every recorded inter-turn gap by F
                                   (default 1.0 = real timing; 0 = no waits /
                                   saturating; 0.1 = replay 10x faster)

Engine / backend env (same names and defaults as run_multiturn_offloading.py):
    MODEL (default Qwen/Qwen2.5-7B-Instruct — the corpus's tokenizer),
    MAX_MODEL_LEN (default 131072 for the Qwen corpus), MAX_NUM_SEQS,
    ACTIVE_SESSIONS (0=open loop), GPU_MEM_UTIL, CPU_BYTES, MAX_OUTPUT_TOKENS
    (clamp per-turn max_tokens; default: use the recorded value), OFFLOAD_MODE,
    DISK_DIR / SECONDARY_TIER / FS_ROOT_DIR / TRACE_OFFLOAD, DISK_DEV,
    DP_SIZE / DP_RANK, CAPTURE_METRICS, ENFORCE_EAGER, ASYNC_SCHED,
    TENSOR_PARALLEL_SIZE. See run_multiturn_offloading.py for the full descriptions.
"""

if __name__ == "__main__":
    import argparse
    import json
    import os
    import sys
    import time

    _here = os.path.dirname(os.path.abspath(__file__))
    if _here not in sys.path:
        sys.path.insert(0, _here)

    import run_multiturn_common as common
    import run_otel_async as otel_run
    from otel_corpus import load_otel_convs

    DEFAULT_DIR = "/mnt/certus1/inference-perf-syn-data/otel_1k"

    ap = argparse.ArgumentParser(description="kv-offload-otel-replay")
    ap.add_argument("--dir", default=os.environ.get("OTEL_DIR", DEFAULT_DIR),
                    help="corpus directory of per-conversation OTel json files")
    ap.add_argument("--num", type=int,
                    default=(int(os.environ["NUM_CONVS"])
                             if os.environ.get("NUM_CONVS") else None),
                    help="number of conversations to replay (default: all files)")
    ap.add_argument("--time-scale", type=float,
                    default=float(os.environ.get("TIME_SCALE", "1.0")),
                    help="scale recorded inter-turn gaps (1.0=real, 0=no waits)")
    args = ap.parse_args()

    # The OTel corpus loader (spans -> per-conversation turn streams) lives in
    # otel_corpus.load_otel_convs, shared with the shmq OTel driver.

    MODEL = os.environ.get("MODEL", "Qwen/Qwen2.5-7B-Instruct")
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 131072))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    ACTIVE_SESSIONS = int(os.environ.get("ACTIVE_SESSIONS", 0))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    CPU_BYTES = int(os.environ.get("CPU_BYTES", 4 * (1 << 30)))
    # Per-turn max_tokens comes from each span; clamp with MAX_OUTPUT_TOKENS if set.
    MAX_OUTPUT_TOKENS = (int(os.environ["MAX_OUTPUT_TOKENS"])
                         if os.environ.get("MAX_OUTPUT_TOKENS") else None)
    # Prompt budget headroom below the window (reserve room for generation). The
    # sliding window also caps context here; the corpus was written for 120k.
    OUTPUT_HEADROOM = int(os.environ.get("OUTPUT_HEADROOM", 2048))
    CONTEXT_CAP = int(os.environ.get("CONTEXT_CAP", 120000))

    DP_SIZE = max(1, int(os.environ.get("DP_SIZE", 1)))
    DP_RANK = max(0, int(os.environ.get("DP_RANK", 0)))

    OFFLOAD_MODE = os.environ.get("OFFLOAD_MODE", "").strip().lower()
    DISK_DIR = os.environ.get("DISK_DIR", "").strip()
    DISK_READ_THREADS = int(os.environ.get("DISK_READ_THREADS", 16))
    DISK_WRITE_THREADS = int(os.environ.get("DISK_WRITE_THREADS", 16))
    SECONDARY_TIER = os.environ.get("SECONDARY_TIER", "").strip().lower()
    FS_ROOT_DIR = os.environ.get("FS_ROOT_DIR", "/mnt/fs-tier/kv-tier")
    FS_READ_THREADS = int(os.environ.get("FS_READ_THREADS", 16))
    FS_WRITE_THREADS = int(os.environ.get("FS_WRITE_THREADS", 16))

    DISK_DEV = os.environ.get("DISK_DEV", "").strip()
    DISK_STAT = f"/sys/block/{DISK_DEV}/stat" if DISK_DEV else ""

    def disk_rw_bytes():
        return common.disk_rw_bytes(DISK_STAT)

    gib = common.gib

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_HEADROOM
    print(f"[run] model={MODEL}", file=sys.stderr)
    print(f"[run] corpus={args.dir} num={args.num or 'ALL'} "
          f"time_scale={args.time_scale:g}", file=sys.stderr)
    print(f"[run] max_model_len={MAX_MODEL_LEN} max_num_seqs={MAX_NUM_SEQS} "
          f"context_cap={CONTEXT_CAP} prompt_budget={PROMPT_BUDGET}",
          file=sys.stderr)
    print(f"[run] cpu_offload_bytes={CPU_BYTES}", file=sys.stderr)

    if DISK_DEV and disk_rw_bytes()[1] is None:
        print(f"[run] WARNING: {DISK_STAT} unreadable — per-round disk bytes disabled",
              file=sys.stderr, flush=True)
    elif not DISK_DEV:
        print("[run] DISK_DEV unset — per-round disk I/O accounting disabled",
              file=sys.stderr, flush=True)

    # ── Load the OTel corpus ──────────────────────────────────────────────
    t_load = time.time()
    convs = load_otel_convs(args.dir, args.num)
    if not convs:
        print(f"[run] no conversations loaded from {args.dir}", file=sys.stderr)
        sys.exit(1)
    if DP_SIZE > 1:
        _global_total = len(convs)
        convs = convs[DP_RANK::DP_SIZE]
        print(f"[run] DP shard rank={DP_RANK}/{DP_SIZE}: {len(convs)} of "
              f"{_global_total} conversations", file=sys.stderr)
    _turns = sorted(len(c) for c in convs)
    print(f"[run] loaded {len(convs)} conversations in {time.time()-t_load:.1f}s "
          f"(turns: min={_turns[0]} median={_turns[len(_turns)//2]} "
          f"max={_turns[-1]}  total={sum(_turns)})", file=sys.stderr)

    # ── Backend / kv_transfer_config (identical to run_multiturn_offloading) ─
    if OFFLOAD_MODE == "none":
        KV_CONFIG = None
        print("[run] OFFLOAD_MODE=none — GPU-only baseline (no kv_transfer_config)",
              file=sys.stderr)
    elif DISK_DIR:
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
            "kv_connector": "TracingConnector",
            "kv_connector_module_path": "tracing_connector",
            "kv_role": "kv_both",
            "kv_connector_extra_config": {
                "traced_kv_connector": "OffloadingConnector",
                "cpu_bytes_to_use": CPU_BYTES,
                "spec_name": "TracingCPUOffloadingSpec",
                "spec_module_path": "tracing_offloading_manager",
                "eviction_policy": "lru",
            },
        }
    elif SECONDARY_TIER == "fs":
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

    # Clear stale trace files (only meaningful under TRACE_OFFLOAD=1).
    for f in os.listdir(_here):
        if (f.startswith("offloading_trace_")
                or f.startswith("offloading_mgr_")
                or f.startswith("offloading_handler_")) \
                and f.endswith(".jsonl"):
            os.remove(os.path.join(_here, f))

    from vllm import SamplingParams

    CAPTURE_METRICS = os.environ.get("CAPTURE_METRICS", "1") != "0"
    _mfu_kwargs = common.mfu_kwargs(CAPTURE_METRICS)

    engine_kwargs = dict(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        enforce_eager=(os.environ.get("ENFORCE_EAGER", "0") != "0"),
        async_scheduling=(os.environ.get("ASYNC_SCHED", "0") != "0"),
        tensor_parallel_size=(max(1, int(tp)) if (tp := os.environ.get("TENSOR_PARALLEL_SIZE", "1").strip()).isdigit() else 1),
        kv_transfer_config=KV_CONFIG,
        disable_log_stats=not CAPTURE_METRICS,
        **_mfu_kwargs,
    )
    # Qwen2.x native window is 32768; the corpus needs 131072, so enable YaRN
    # when MAX_MODEL_LEN exceeds native (no-op otherwise). ROPE_YARN=0 opts out.
    _yarn = common.yarn_hf_overrides(MODEL, MAX_MODEL_LEN)
    if _yarn:
        engine_kwargs["hf_overrides"] = _yarn

    # Base sampling params; per-turn max_tokens is set from each span in run_otel.
    sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=256)

    summary = otel_run.run_otel_driver(
        engine_kwargs, convs, sp,
        prompt_budget=PROMPT_BUDGET,
        context_cap=CONTEXT_CAP,
        time_scale=args.time_scale,
        max_output_tokens=MAX_OUTPUT_TOKENS,
        capture_metrics=CAPTURE_METRICS,
        disk_rw_bytes=disk_rw_bytes,
        active_sessions=ACTIVE_SESSIONS,
        summary_base={
            "workload": "otel-replay",
            "corpus": args.dir,
            "model": MODEL,
            "max_model_len": MAX_MODEL_LEN,
            "context_cap": CONTEXT_CAP,
            "cpu_bytes_to_use": CPU_BYTES,
            "disk_dir": DISK_DIR or None,
            "tier": "cpu+disk" if DISK_DIR else "cpu",
        },
    )
    elapsed = summary["elapsed_time"]
    rounds_done = summary["num_rounds"]
    total_generations = summary["total_generations"]

    with open(os.path.join(_here, "otel_replay_results.json"), "w") as f:
        json.dump(summary, f, indent=2)
    print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} "
          f"max_turns={rounds_done}", file=sys.stderr)

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
