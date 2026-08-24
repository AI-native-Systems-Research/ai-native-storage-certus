import os, sys, json, time

_here = os.path.dirname(os.path.abspath(__file__))
if _here not in sys.path:
    sys.path.insert(0, _here)
import run_multiturn_common as common
import run_multiturn_sync_batched as batched


def _pin_to_numa_node():
    """Pin CPUs + memory to one NUMA node (default 0); the GPU stays wherever it
    is. Done IN-PROCESS rather than via an external `numactl` wrapper because
    vLLM spawns its EngineCore worker as a child that resets its own CPU
    affinity — an outer numactl does not hold, but os.sched_setaffinity here is
    inherited across the spawn. Memory is bound with set_mempolicy(MPOL_BIND).

    Env: BENCH_NUMA_NODE (default "0"), BENCH_CPUS (default node-0 cores
    "0-15,32-47"), NO_PIN=1 to disable. Failures are non-fatal (logged only) so
    the bench still runs on hosts with a different topology.
    """
    if os.environ.get("NO_PIN"):
        return
    node = int(os.environ.get("BENCH_NUMA_NODE", "0"))
    cpu_spec = os.environ.get("BENCH_CPUS", "0-15,32-47")
    try:
        cpus = set()
        for part in cpu_spec.split(","):
            lo, _, hi = part.partition("-")
            cpus.update(range(int(lo), int(hi or lo) + 1))
        os.sched_setaffinity(0, cpus)
        # Bind memory allocations to `node` via set_mempolicy(MPOL_BIND). glibc
        # does not export a set_mempolicy wrapper, so invoke the raw syscall.
        # __NR_set_mempolicy: 238 on x86-64, 276 on x86-32, 237 on the generic
        # ABI (aarch64 etc.). Inherited across the EngineCore spawn.
        import ctypes, platform
        _NR = {"x86_64": 238, "i386": 276, "i686": 276,
               "aarch64": 237, "armv7l": 321}.get(platform.machine(), 238)
        libc = ctypes.CDLL("libc.so.6", use_errno=True)
        MPOL_BIND = 2
        nodemask = ctypes.c_ulong(1 << node)
        if libc.syscall(_NR, MPOL_BIND, ctypes.byref(nodemask), 64) != 0:
            raise OSError(ctypes.get_errno(), "set_mempolicy syscall failed")
        print(f"[pin] bound to NUMA node {node}: {len(cpus)} cpus + mem MPOL_BIND",
              flush=True)
    except Exception as e:
        print(f"[pin] WARNING: NUMA pin failed ({e}); running unpinned", flush=True)


# WORKLOAD_NAME=<name> selects a registered workload (e.g. WORKLOAD_NAME=sharegpt
# -> the ShareGPT multi-turn subset by human-turn count, default 12/12 = the
# 450x12 set); DATASET_PATH / NUM_CONVS still override.
SUBSET_PATH, NUM_CONVS = common.resolve_workload("sharegpt_12turn_450.json", 450)
MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")
MAX_ROUNDS = int(os.environ.get("MAX_ROUNDS", 0))  # 0 = until convs exhausted
DRAM = int(os.environ.get("DRAM", 8589934592))
PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS

KV_PATH = "/mnt/fs-backend-bench/shared-kv"

KV_CONFIG = {
    "kv_connector": "OffloadingConnector",
    "kv_role": "kv_both",
    "kv_connector_extra_config": {
        "spec_name": "SharedStorageOffloadingSpec",
        "spec_module_path": "llmd_fs_backend.spec",
        "shared_storage_path": KV_PATH,
        "max_staging_memory_gb": DRAM // (1024**3),
        "threads_per_gpu": 64,
    },
}

# --- Per-round disk I/O accounting -------------------------------------------
# /sys/block/<dev>/stat exposes cumulative sectors read (field 3) and sectors
# written (field 7), always in 512-byte units. Snapshot around each generate()
# to get bytes moved to/from the physical device per round. Reading md0 captures
# all filesystem I/O across the RAID0 members in one place.
DISK_DEV = os.environ.get("DISK_DEV", "md0")
DISK_STAT = f"/sys/block/{DISK_DEV}/stat"


def disk_rw_bytes():
    """Return (bytes_read, bytes_written) cumulative for DISK_DEV, or (None, None)."""
    return common.disk_rw_bytes(DISK_STAT)


gib = common.gib


def preflight():
    """Verify the box is configured for the SharedStorage bench before we spend
    minutes loading a model into a mis-configured system. Fail fast and loud.
    Configure with: sudo ./tools/configure-bench.sh sharedstorage (then reboot
    for the mem= cap). Skip with SKIP_PREFLIGHT=1."""
    if os.environ.get("SKIP_PREFLIGHT"):
        return
    errs = []
    # 1. KV path is a writable dir on the mounted RAID (not a bare rootfs dir).
    if not os.path.ismount("/mnt/fs-backend-bench"):
        errs.append("/mnt/fs-backend-bench not mounted (run configure-bench.sh sharedstorage)")
    elif not os.access(KV_PATH, os.W_OK):
        errs.append(f"{KV_PATH} missing or not writable")
    # 2. RAM must be capped — page cache is SS's DRAM, and a faithful run needs
    #    the hard mem= cap active, not the full box.
    with open("/proc/meminfo") as f:
        total_gib = int(f.readline().split()[1]) / (1024**2)
    if total_gib > 100:
        errs.append(f"RAM not capped ({total_gib:.0f} GiB) — reboot for mem= cap "
                    "(see configure-bench.sh kernel mode)")
    if errs:
        sys.exit("[preflight] NOT ready for sharedstorage:\n  - " + "\n  - ".join(errs))
    print(f"[preflight] ok: RAID mounted, {KV_PATH} writable, RAM ~{total_gib:.0f} GiB",
          file=sys.stderr, flush=True)


def main():
    # NUMA pin must happen in the parent so the spawned EngineCore inherits the
    # CPU/mem affinity. Guarded under __main__ (below) so vLLM's spawn-based
    # EngineCore re-import of this module does NOT re-run the workload.
    _pin_to_numa_node()
    preflight()

    t0 = time.perf_counter()
    print(f"[trace] +0.0s loading dataset", file=sys.stderr, flush=True)
    convs = common.load_convs(SUBSET_PATH, NUM_CONVS)
    print(f"[trace] +{time.perf_counter()-t0:.1f}s loaded {len(convs)} conversations", file=sys.stderr, flush=True)

    print(f"[trace] +{time.perf_counter()-t0:.1f}s importing vllm", file=sys.stderr, flush=True)
    from vllm import SamplingParams

    # Capturing vLLM's Prometheus counters requires the engine's stat logging to
    # be on (it's what advances the metrics). Enabling it has a small overhead, so
    # a capture run is not byte-identical to the stats-off timing baseline —
    # CAPTURE_METRICS=0 restores that baseline and skips the per-round snapshot.
    CAPTURE_METRICS = os.environ.get("CAPTURE_METRICS", "1") != "0"

    # WORKLOAD_MODE=async runs one vLLM coroutine per conversation (V1 AsyncLLM);
    # "batched" (default) runs the synchronous per-round generate loop. Both share
    # the engine_kwargs below (same SharedStorage kv_transfer_config).
    WORKLOAD_MODE = os.environ.get("WORKLOAD_MODE", "batched").strip().lower()

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
        kv_transfer_config=KV_CONFIG,
        disable_log_stats=not CAPTURE_METRICS,
    )

    sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=OUTPUT_TOKENS)

    if WORKLOAD_MODE == "async":
        # Async model: one coroutine per conv. The elaborate per-round disk-I/O
        # table below is a batched-only artifact; here the 1 Hz sampler in
        # run_async_driver captures md0 read/write bytes into the summary's
        # `samples` instead, and prints whole-run counter movement + the KV
        # REGISTRY dump equivalent via counter_movement.
        import run_multiturn_async as async_run

        print(f"[trace] +{time.perf_counter()-t0:.1f}s WORKLOAD_MODE=async",
              file=sys.stderr, flush=True)
        summary = async_run.run_async_driver(
            engine_kwargs, convs, sp,
            prompt_budget=PROMPT_BUDGET,
            max_rounds=MAX_ROUNDS,
            capture_metrics=CAPTURE_METRICS,
            disk_rw_bytes=disk_rw_bytes,
            n_tokens_flavor="encode",
            skip_empty=False,
            summary_base={"model": MODEL, "max_model_len": MAX_MODEL_LEN,
                          "output_tokens": OUTPUT_TOKENS, "backend": "sharedstorage",
                          "dev": DISK_DEV},
        )
        elapsed = summary["elapsed_time"]
        rounds_done = summary["num_rounds"]
        total_generations = summary["total_generations"]
        try:
            with open(os.path.join(_here, f"ss_async_results_{int(elapsed)}.json"),
                      "w") as f:
                json.dump(summary, f, indent=2)
        except OSError as e:
            print(f"[io] could not save json: {e}", file=sys.stderr)
        print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} "
              f"rounds={rounds_done}", file=sys.stderr)
        return

    print(f"[trace] +{time.perf_counter()-t0:.1f}s creating LLM", file=sys.stderr, flush=True)
    llm = common.build_engine(engine_kwargs, async_mode=False)
    print(f"[trace] +{time.perf_counter()-t0:.1f}s LLM ready", file=sys.stderr, flush=True)

    common.start_prom_exporter()

    tokenizer = llm.get_tokenizer()
    n_tokens = common.make_n_tokens(tokenizer, "encode")

    if disk_rw_bytes()[1] is None:
        print(f"[trace] WARNING: {DISK_STAT} unreadable — per-round disk bytes disabled "
              f"(set DISK_DEV or check mode)", file=sys.stderr, flush=True)

    round_io = []  # (round, prompts, read_bytes, write_bytes)

    # ── vLLM Prometheus counters (per round) ──────────────────────────────
    # Snapshot each vllm: counter at the end of every round and log the delta;
    # the full per-round series is also dumped to JSON. prom_counters lives in
    # run_multiturn_common (get_metrics() + REGISTRY branches).
    prom_prev = [common.prom_counters(llm, CAPTURE_METRICS)]
    prom_rounds = []  # (round, {counter_name: delta})
    disk_pre = [None, None]

    def on_round_start(round_idx, n_prompts):
        print(f"[trace] +{time.perf_counter()-t0:.1f}s calling generate round "
              f"{round_idx} ({n_prompts} prompts)", file=sys.stderr, flush=True)
        disk_pre[0], disk_pre[1] = disk_rw_bytes()

    def on_round_end(round_idx, n_prompts, round_elapsed, n_alive):
        rd0, wr0 = disk_pre
        rd1, wr1 = disk_rw_bytes()
        # Delta of cumulative counters = bytes moved during this round.
        d_rd = (rd1 - rd0) if (rd0 is not None and rd1 is not None) else None
        d_wr = (wr1 - wr0) if (wr0 is not None and wr1 is not None) else None
        round_io.append((round_idx, n_prompts, d_rd, d_wr))
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

    print(f"[trace] +{time.perf_counter()-t0:.1f}s entering generate loop", file=sys.stderr, flush=True)
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
            _prom_path = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                      "prom_counters_rounds.json")
            with open(_prom_path, "w") as f:
                json.dump([{"round": r, "counters": d} for r, d in prom_rounds],
                          f, indent=2)
            print(f"[prom] saved {_prom_path}", file=sys.stderr)
        except OSError as e:
            print(f"[prom] could not save json: {e}", file=sys.stderr)
    print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} rounds={rounds_done}",
          file=sys.stderr)

    # --- Per-round disk I/O summary ------------------------------------------
    tot_rd = sum(r for _, _, r, _ in round_io if r is not None)
    tot_wr = sum(w for _, _, _, w in round_io if w is not None)
    print(f"\n[io] per-round disk bytes (dev={DISK_DEV}):", file=sys.stderr)
    print(f"[io] {'round':>5} {'prompts':>7} {'read':>12} {'written':>12}", file=sys.stderr)
    for rnd, npr, rd, wr in round_io:
        print(f"[io] {rnd:>5} {npr:>7} {gib(rd):>12} {gib(wr):>12}", file=sys.stderr)
    print(f"[io] {'TOTAL':>5} {'':>7} {gib(tot_rd):>12} {gib(tot_wr):>12}", file=sys.stderr)
    try:
        io_path = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                               f"ss_round_io_{int(elapsed)}.json")
        with open(io_path, "w") as f:
            json.dump({"dev": DISK_DEV, "wall": elapsed, "rounds": [
                {"round": r, "prompts": n, "read_bytes": rd, "write_bytes": wr}
                for r, n, rd, wr in round_io],
                "total_read_bytes": tot_rd, "total_write_bytes": tot_wr}, f, indent=2)
        print(f"[io] saved {io_path}", file=sys.stderr)
    except OSError as e:
        print(f"[io] could not save json: {e}", file=sys.stderr)

    try:
        from prometheus_client import REGISTRY
        print("\n[PROM] vLLM Offloading Metrics:", file=sys.stderr)
        for metric in REGISTRY.collect():
            if "kv_offload" in metric.name:
                for sample in metric.samples:
                    if sample.value > 0:
                        if "_bucket" in sample.name:
                            continue
                        print(f"[PROM] {sample.name} {sample.labels} = {sample.value}", file=sys.stderr)
    except Exception as e:
        print(f"[PROM] error: {e}", file=sys.stderr)


if __name__ == "__main__":
    main()
