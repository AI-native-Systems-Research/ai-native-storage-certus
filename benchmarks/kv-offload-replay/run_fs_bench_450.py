import os, sys, json, time


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


SUBSET_PATH = os.environ.get("DATASET_PATH", "sharegpt_12turn_450.json")
NUM_CONVS = int(os.environ.get("NUM_CONVS", 450))
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
SECTOR = 512


def disk_rw_bytes():
    """Return (bytes_read, bytes_written) cumulative for DISK_DEV, or (None, None)."""
    try:
        with open(DISK_STAT) as f:
            fields = f.read().split()
        return int(fields[2]) * SECTOR, int(fields[6]) * SECTOR
    except (OSError, IndexError, ValueError):
        return None, None


def gib(n):
    return "n/a" if n is None else f"{n / (1024**3):.2f} GiB"


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
    print(f"[trace] +{time.perf_counter()-t0:.1f}s loaded {len(convs)} conversations", file=sys.stderr, flush=True)

    print(f"[trace] +{time.perf_counter()-t0:.1f}s importing vllm", file=sys.stderr, flush=True)
    from vllm import LLM, SamplingParams

    print(f"[trace] +{time.perf_counter()-t0:.1f}s creating LLM", file=sys.stderr, flush=True)
    # Capturing vLLM's Prometheus counters requires the engine's stat logging to
    # be on (it's what advances the metrics). Enabling it has a small overhead, so
    # a capture run is not byte-identical to the stats-off timing baseline —
    # CAPTURE_METRICS=0 restores that baseline and skips the per-round snapshot.
    CAPTURE_METRICS = os.environ.get("CAPTURE_METRICS", "1") != "0"
    llm = LLM(
        model=MODEL,
        max_model_len=MAX_MODEL_LEN,
        max_num_seqs=MAX_NUM_SEQS,
        gpu_memory_utilization=GPU_MEM_UTIL,
        dtype="float16",
        enable_prefix_caching=True,
        enforce_eager=True,
        kv_transfer_config=KV_CONFIG,
        disable_log_stats=not CAPTURE_METRICS,
    )
    print(f"[trace] +{time.perf_counter()-t0:.1f}s LLM ready", file=sys.stderr, flush=True)

    # Optional Prometheus exporter. When PROM_PORT is set, expose vLLM's engine
    # + KV-offload metrics over HTTP at :PROM_PORT/metrics for live scraping.
    # Requires LOG_STATS=1 (above) so metrics are registered — otherwise the
    # endpoint serves an empty registry. No-op when PROM_PORT is unset. (The
    # end-of-run REGISTRY dump below still works independently of this.)
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

    def n_tokens(text):
        return len(tokenizer.encode(text))

    if disk_rw_bytes()[1] is None:
        print(f"[trace] WARNING: {DISK_STAT} unreadable — per-round disk bytes disabled "
              f"(set DISK_DEV or check mode)", file=sys.stderr, flush=True)

    alive = [True] * len(convs)
    next_turn = [0] * len(convs)
    contexts = [""] * len(convs)
    total_generations = 0
    rounds_done = 0
    round_io = []  # (round, prompts, read_bytes, write_bytes)

    # ── vLLM Prometheus counters (per round) ──────────────────────────────
    # vLLM registers every metric on the default prometheus_client REGISTRY under
    # the `vllm:` prefix, updated as the engine steps (present on both the V0 and
    # V1 engines whenever stat logging is on). Snapshot each counter (samples
    # named `vllm:*_total`, summed across label sets) at the end of every round
    # and log the delta; the full per-round series is also dumped to JSON.
    def prom_counters():
        vals = {}
        if not CAPTURE_METRICS:
            return vals
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

    prom_prev = prom_counters()
    prom_rounds = []  # (round, {counter_name: delta})

    t_start = time.perf_counter()
    print(f"[trace] +{time.perf_counter()-t0:.1f}s entering generate loop", file=sys.stderr, flush=True)

    while True:
        if MAX_ROUNDS and rounds_done >= MAX_ROUNDS:
            break
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

        rounds_done += 1
        print(f"[trace] +{time.perf_counter()-t0:.1f}s calling generate round {rounds_done} ({len(active_prompts)} prompts)", file=sys.stderr, flush=True)
        rd0, wr0 = disk_rw_bytes()
        round_start = time.perf_counter()
        outs = llm.generate(active_prompts, sp)
        round_elapsed = time.perf_counter() - round_start
        rd1, wr1 = disk_rw_bytes()
        for i, out in zip(active_idx, outs):
            response = out.outputs[0].text if out.outputs else ""
            contexts[i] = contexts[i] + response
            next_turn[i] += 1
        total_generations += len(active_prompts)
        n_alive = sum(alive)
        # Delta of cumulative counters = bytes moved during this round.
        d_rd = (rd1 - rd0) if (rd0 is not None and rd1 is not None) else None
        d_wr = (wr1 - wr0) if (wr0 is not None and wr1 is not None) else None
        round_io.append((rounds_done, len(active_prompts), d_rd, d_wr))
        print(f"[run] round {rounds_done}: {len(active_prompts)} prompts in "
              f"{round_elapsed:.1f}s  ({n_alive} convs still alive)  "
              f"disk_read={gib(d_rd)} disk_write={gib(d_wr)}",
              file=sys.stderr, flush=True)
        if CAPTURE_METRICS:
            prom_now = prom_counters()
            d_prom = {k: prom_now.get(k, 0.0) - prom_prev.get(k, 0.0)
                      for k in prom_now}
            prom_prev = prom_now
            prom_rounds.append((rounds_done, d_prom))
            shown = " ".join(f"{k[len('vllm:'):]}={d_prom[k]:.0f}"
                             for k in sorted(d_prom) if d_prom[k])
            print(f"[prom] round {rounds_done}: {shown or '(no counter movement)'}",
                  file=sys.stderr, flush=True)

    elapsed = time.perf_counter() - t_start
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
