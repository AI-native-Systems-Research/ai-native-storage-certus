#!/usr/bin/env python3
"""run_otel_shmq_certus.py — kv-offload-otel-replay on the Certus shmq backend.

The shmq analogue of ``benchmarks/kv-offload-replay/run_otel_replay.py``: replays
the ``otel_trace_replay`` corpus (built by ``benchmarks/kv-offload-otel-replay/trace-gen/trace_to_otel.py``) with the
recorded per-turn timing against the *shared-memory* connector
(CertusShmqOffloadingSpec) driving a running certus-server over a /dev/shm
mailbox — the same connector/compat/session-id/SSD-telemetry setup as
``run_multiturn_shmq_certus.py``, but fed the OTel corpus instead of ShareGPT and
run through the timestamp-scheduled async execution model
(``run_otel_async.run_otel_driver``).

Why a separate driver (not a BACKEND=shmq branch in run_otel_replay.py): the shmq
path imports the ``certus_shmq_connector`` package (spec + compat + ring), which
only exists where that package is installed (the shmq container / a dev checkout),
so it stays out of the connector-agnostic benchmarks dir — exactly the split
between run_multiturn_offloading.py and run_multiturn_shmq_certus.py.

OTel-specific options (CLI flags, env fallbacks):
    --dir DIR      / OTEL_DIR    corpus directory of per-conversation json files
                                 (default /mnt/certus1/inference-perf-syn-data/otel_1k)
    --num N        / NUM_CONVS   conversations to replay (default: ALL files)
    --time-scale F / TIME_SCALE  scale recorded inter-turn gaps
                                 (default 1.0 = real timing; 0 = no waits)

Backend env (same as run_multiturn_shmq_certus.py): SHM_PATH, SLAB_SIZE_BYTES,
MODEL (default Qwen/Qwen2.5-7B-Instruct — the corpus tokenizer),
MAX_MODEL_LEN (default 131072), MAX_NUM_SEQS, ACTIVE_SESSIONS (0=open loop),
GPU_MEM_UTIL, TENSOR_PARALLEL_SIZE, PIPELINE_PARALLEL_SIZE, KV_CACHE_DTYPE, DTYPE,
ENFORCE_EAGER, CAPTURE_METRICS/LOG_STATS, TRACE_OFFLOAD, DP_SIZE/DP_RANK,
MAX_OUTPUT_TOKENS (clamp per-turn max_tokens; default: recorded value),
CONTEXT_CAP (sliding-window token cap, default 120000), OUTPUT_HEADROOM (default 2048).
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
    # The shared workload + OTel modules live in the benchmarks dir, not here.
    _bench_dir = os.path.join(_here, "..", "benchmarks", "kv-offload-replay")
    if _bench_dir not in sys.path:
        sys.path.insert(0, _bench_dir)
    import run_multiturn_common as common
    import run_otel_async as otel_run
    from otel_corpus import load_otel_convs

    DEFAULT_DIR = "/mnt/certus1/inference-perf-syn-data/otel_1k"

    ap = argparse.ArgumentParser(description="kv-offload-otel-replay on certus-shmq")
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

    SHM_PATH = os.environ.get("SHM_PATH", "/dev/shm/certus-shmq")
    MODEL = os.environ.get("MODEL", "Qwen/Qwen2.5-7B-Instruct")
    MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 131072))
    MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
    ACTIVE_SESSIONS = int(os.environ.get("ACTIVE_SESSIONS", 0))
    GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
    TENSOR_PARALLEL_SIZE = int(os.environ.get("TENSOR_PARALLEL_SIZE", 1))
    PIPELINE_PARALLEL_SIZE = int(os.environ.get("PIPELINE_PARALLEL_SIZE", 1))
    SLAB_SIZE_BYTES = int(os.environ.get("SLAB_SIZE_BYTES", 131072))
    MAX_OUTPUT_TOKENS = (int(os.environ["MAX_OUTPUT_TOKENS"])
                         if os.environ.get("MAX_OUTPUT_TOKENS") else None)
    OUTPUT_HEADROOM = int(os.environ.get("OUTPUT_HEADROOM", 2048))
    CONTEXT_CAP = int(os.environ.get("CONTEXT_CAP", 120000))

    DP_SIZE = max(1, int(os.environ.get("DP_SIZE", 1)))
    DP_RANK = max(0, int(os.environ.get("DP_RANK", 0)))

    PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_HEADROOM
    print(f"[run] model={MODEL} shm_path={SHM_PATH}", file=sys.stderr)
    print(f"[run] corpus={args.dir} num={args.num or 'ALL'} "
          f"time_scale={args.time_scale:g}", file=sys.stderr)
    print(f"[run] max_model_len={MAX_MODEL_LEN} max_num_seqs={MAX_NUM_SEQS} "
          f"context_cap={CONTEXT_CAP} prompt_budget={PROMPT_BUDGET}",
          file=sys.stderr)

    # ── shmq connector config (identical to run_multiturn_shmq_certus.py) ─────
    _extra_config = {
        "spec_name": "CertusShmqOffloadingSpec",
        "spec_module_path": "certus_shmq_connector.spec",
        "shm_path": SHM_PATH,
        "slab_size_bytes": SLAB_SIZE_BYTES,
    }
    if os.environ.get("TRACE_OFFLOAD", "0") != "0":
        _extra_config["traced_kv_connector"] = "OffloadingConnector"
        KV_CONFIG = {
            "kv_connector": "TracingConnector",
            "kv_connector_module_path": "tracing_connector",
            "kv_role": "kv_both",
            "kv_connector_extra_config": _extra_config,
        }
        print("[run] TRACE_OFFLOAD=1: TracingConnector wrapping OffloadingConnector",
              file=sys.stderr)
    else:
        KV_CONFIG = {
            "kv_connector": "OffloadingConnector",
            "kv_role": "kv_both",
            "kv_connector_extra_config": _extra_config,
        }

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

    from vllm import SamplingParams

    from certus_shmq_connector.compat import CAPS as _CAPS
    from certus_shmq_connector.compat import VERSION as _VLLM_VERSION

    # Version-specific engine flags, resolved from the compat capability matrix
    # (same as run_multiturn_shmq_certus.py).
    _engine_kwargs = {}
    if _CAPS.needs_disable_hybrid_kv_cache_manager:
        _engine_kwargs["disable_hybrid_kv_cache_manager"] = True
        print(f"[run] vLLM {_VLLM_VERSION[0]}.{_VLLM_VERSION[1]}: "
              f"disable_hybrid_kv_cache_manager=True (OffloadingConnector requirement)",
              file=sys.stderr)
    if _CAPS.needs_disable_async_scheduling:
        _engine_kwargs["async_scheduling"] = False
        print(f"[run] vLLM {_VLLM_VERSION[0]}.{_VLLM_VERSION[1]}: "
              f"async_scheduling=False (OffloadingConnector serializes transfers per request)",
              file=sys.stderr)

    _log_stats_on = os.environ.get("LOG_STATS", "0") != "0"
    CAPTURE_METRICS = os.environ.get("CAPTURE_METRICS", "1") != "0" or _log_stats_on

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
        kv_cache_dtype=os.environ.get("KV_CACHE_DTYPE", "auto"),
        kv_transfer_config=KV_CONFIG,
        disable_log_stats=not CAPTURE_METRICS,
        **_engine_kwargs,
    )
    # Qwen2.x native window is 32768; the corpus needs 131072, so enable YaRN
    # when MAX_MODEL_LEN exceeds native (no-op otherwise). ROPE_YARN=0 opts out.
    _yarn = common.yarn_hf_overrides(MODEL, MAX_MODEL_LEN)
    if _yarn:
        engine_kwargs["hf_overrides"] = _yarn

    # Base sampling params; per-turn max_tokens comes from each span in run_otel.
    sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=256)

    # Tag each conversation with a stable KV-offload session_id (GLOBAL index +1
    # so ids stay unique across DP replicas sharing one server; +1 keeps conv 0
    # non-zero, since 0 == "unset"). Same scheme as run_multiturn_shmq_certus.py.
    _session_id_fn = lambda i: (DP_RANK + i * DP_SIZE) + 1  # noqa: E731

    # ── SSD device I/O over the shmq ring's GetIoStats op ─────────────────────
    from certus_shmq_connector.ring import Ring

    try:
        io_ring = Ring(SHM_PATH)
    except Exception as e:  # noqa: BLE001 - ring may be absent; degrade gracefully
        io_ring = None
        print(f"[io] GetIoStats unavailable ({e}); per-tick SSD bytes disabled",
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

    summary = otel_run.run_otel_driver(
        engine_kwargs, convs, sp,
        prompt_budget=PROMPT_BUDGET,
        context_cap=CONTEXT_CAP,
        time_scale=args.time_scale,
        max_output_tokens=MAX_OUTPUT_TOKENS,
        capture_metrics=CAPTURE_METRICS,
        disk_rw_bytes=io_rw_bytes,
        session_id_fn=_session_id_fn,
        active_sessions=ACTIVE_SESSIONS,
        summary_base={
            "workload": "otel-replay",
            "corpus": args.dir,
            "backend": "certus-shmq",
            "model": MODEL,
            "shm_path": SHM_PATH,
            "max_model_len": MAX_MODEL_LEN,
            "context_cap": CONTEXT_CAP,
        },
    )
    elapsed = summary["elapsed_time"]
    total_generations = summary["total_generations"]
    rounds_done = summary["num_rounds"]

    try:
        with open(os.path.join(_here, "otel_shmq_results.json"), "w") as f:
            json.dump(summary, f, indent=2)
    except OSError as e:
        print(f"[run] could not save results: {e}", file=sys.stderr)

    print(f"[run] DONE max_turns={rounds_done} generations={total_generations} "
          f"elapsed={elapsed:.1f}s "
          f"({total_generations / elapsed:.1f} gen/s)", file=sys.stderr)
