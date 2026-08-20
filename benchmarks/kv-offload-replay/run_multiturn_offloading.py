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

    # Default to the 450-conversation / 12-turn ShareGPT workload shared with
    # the Certus connector (data/sharegpt_12turn_450.json). Override
    # with DATASET_PATH to point at a different ShareGPT-format json.
    DEFAULT_DATASET = os.path.join(
        _here, "..", "..", "data", "sharegpt_12turn_450.json"
    )
    SUBSET_PATH = os.environ.get("DATASET_PATH", DEFAULT_DATASET)
    if not os.path.exists(SUBSET_PATH):
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
    SECTOR = 512

    def disk_rw_bytes():
        """Return (bytes_read, bytes_written) cumulative for DISK_DEV, or (None, None)."""
        if not DISK_STAT:
            return None, None
        try:
            with open(DISK_STAT) as _f:
                fields = _f.read().split()
            return int(fields[2]) * SECTOR, int(fields[6]) * SECTOR
        except (OSError, IndexError, ValueError):
            return None, None

    def gib(n):
        return "n/a" if n is None else f"{n / (1024 ** 3):.2f} GiB"

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
    with open(SUBSET_PATH) as f:
        all_data = json.load(f)
    convs = []
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
    # shmq backend, which also uses the plain OffloadingConnector. Set
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

    from vllm import LLM, SamplingParams

    llm = LLM(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        # ENFORCE_EAGER=0 (default) keeps CUDA graphs on (vLLM default) for a fair
        # comparison; =1 forces eager.
        enforce_eager=(os.environ.get("ENFORCE_EAGER", "0") != "0"),
        # async_scheduling MUST be off for the OffloadingConnector: it serializes
        # KV transfers per request, and the async batch-queue scheduler path
        # (step_with_batch_queue) trips a KeyError in the native tiering manager's
        # prepare_store (self._req_state[req_id]) — EngineDeadError at round 1.
        # This is ORTHOGONAL to cudagraph: disabling it keeps the fair cudagraph
        # config while taking the synchronous, connector-correct scheduling path
        # (mirrors run_multiturn_shmq_certus.py's needs_disable_async_scheduling).
        # Override with ASYNC_SCHED=1 to reproduce the crash.
        async_scheduling=(os.environ.get("ASYNC_SCHED", "0") != "0"),
        kv_transfer_config=KV_CONFIG,
        # LOG_STATS=1 keeps vLLM's stats logging on so its PrometheusStatLogger
        # registers metrics (incl. the tiering/kv_offload counters). Default off
        # to keep per-round output clean.
        disable_log_stats=(os.environ.get("LOG_STATS", "0") == "0"),
    )

    # Optional Prometheus exporter. When PROM_PORT is set, expose vLLM's engine
    # + KV-offload metrics over HTTP at :PROM_PORT/metrics for live scraping.
    # Requires LOG_STATS=1 (above) so the metrics are registered — otherwise the
    # endpoint serves an empty registry. No-op when PROM_PORT is unset.
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

    sp = SamplingParams(
        temperature=0.7,
        top_p=0.95,
        max_tokens=OUTPUT_TOKENS,
    )

    tokenizer = llm.get_tokenizer()

    def n_tokens(text: str) -> int:
        return len(tokenizer(text).input_ids)

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
        rd0, wr0 = disk_rw_bytes()
        round_start = time.perf_counter()
        outs = llm.generate(active_prompts, sp)
        round_elapsed = time.perf_counter() - round_start
        rd1, wr1 = disk_rw_bytes()
        d_rd = None if rd0 is None or rd1 is None else rd1 - rd0
        d_wr = None if wr0 is None or wr1 is None else wr1 - wr0
        # Append vLLM's own response for next round's prefix
        for i, out in zip(active_idx, outs):
            response = out.outputs[0].text if out.outputs else ""
            contexts[i] = contexts[i] + response
            next_turn[i] += 1
        total_generations += len(active_prompts)
        n_alive = sum(alive)
        print(f"[run] round {rounds_done}: {len(active_prompts)} prompts in "
              f"{round_elapsed:.1f}s  ({n_alive} convs still alive)  "
              f"disk_read={gib(d_rd)} disk_write={gib(d_wr)}",
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
