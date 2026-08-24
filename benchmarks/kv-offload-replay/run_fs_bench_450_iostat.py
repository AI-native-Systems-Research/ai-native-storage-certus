import os, sys, json, time

_here = os.path.dirname(os.path.abspath(__file__))
if _here not in sys.path:
    sys.path.insert(0, _here)
import run_multiturn_common as common
import run_multiturn_sync_batched as batched

os.chdir("/home/bdh/kvconn-trace")


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


_pin_to_numa_node()

# WORKLOAD=<name> selects a registered workload (e.g. WORKLOAD=sharegpt -> the
# data/sharegpt 10k chunks); DATASET_PATH / NUM_CONVS still override.
SUBSET_PATH, NUM_CONVS = common.resolve_workload("sharegpt_12turn_450.json", 450)
MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")
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


preflight()

t0 = time.perf_counter()
print(f"[trace] +0.0s loading dataset", file=sys.stderr, flush=True)
convs = common.load_convs(SUBSET_PATH, NUM_CONVS)
print(f"[trace] +{time.perf_counter()-t0:.1f}s loaded {len(convs)} conversations", file=sys.stderr, flush=True)

print(f"[trace] +{time.perf_counter()-t0:.1f}s importing vllm", file=sys.stderr, flush=True)
from vllm import SamplingParams

# WORKLOAD_MODE=async runs one vLLM coroutine per conversation (V1 AsyncLLM);
# "batched" (default) runs the synchronous per-round generate loop. Both share
# engine_kwargs below.
WORKLOAD_MODE = os.environ.get("WORKLOAD_MODE", "batched").strip().lower()
MAX_ROUNDS = int(os.environ.get("MAX_ROUNDS", 0))  # async per-conv turn cap

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
    disable_log_stats=False,  # enable built-in metrics: prefix-cache + preemption counters
)

sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=OUTPUT_TOKENS)

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

if disk_rw_bytes()[1] is None:
    print(f"[trace] WARNING: {DISK_STAT} unreadable — per-round disk bytes disabled "
          f"(set DISK_DEV or check mode)", file=sys.stderr, flush=True)

if WORKLOAD_MODE == "async":
    # The rich per-round prefix_stats table below is a batched-only artifact —
    # under the one-coroutine-per-conv model there are no rounds, so this path is
    # aggregate-only: the 1 Hz sampler captures md0 read/write bytes, and the
    # whole-run vllm: counter movement (external_prefix_cache_queries/hits,
    # num_preemptions) lands in the summary instead of per-round deltas.
    import run_multiturn_async as async_run
    print(f"[trace] +{time.perf_counter()-t0:.1f}s WORKLOAD_MODE=async "
          "(aggregate-only; no per-round prefix table)", file=sys.stderr, flush=True)
    summary = async_run.run_async_driver(
        engine_kwargs, convs, sp,
        prompt_budget=PROMPT_BUDGET,
        max_rounds=MAX_ROUNDS,
        capture_metrics=True,
        disk_rw_bytes=disk_rw_bytes,
        n_tokens_flavor="encode",
        skip_empty=False,
        summary_base={"model": MODEL, "max_model_len": MAX_MODEL_LEN,
                      "output_tokens": OUTPUT_TOKENS, "backend": "sharedstorage",
                      "dev": DISK_DEV},
    )
    try:
        with open(os.path.join(_here,
                  f"ss_async_results_{int(summary['elapsed_time'])}.json"), "w") as f:
            json.dump(summary, f, indent=2)
    except OSError as e:
        print(f"[io] could not save json: {e}", file=sys.stderr)
    print(f"\n[run] done. wall={summary['elapsed_time']:.1f}s  "
          f"generations={summary['total_generations']} rounds={summary['num_rounds']}",
          file=sys.stderr)
    sys.exit(0)

print(f"[trace] +{time.perf_counter()-t0:.1f}s creating LLM", file=sys.stderr, flush=True)
llm = common.build_engine(engine_kwargs, async_mode=False)
print(f"[trace] +{time.perf_counter()-t0:.1f}s LLM ready", file=sys.stderr, flush=True)

tokenizer = llm.get_tokenizer()
n_tokens = common.make_n_tokens(tokenizer, "encode")

# --- vLLM-layer offload / recompute counters ---------------------------------
# LLM.get_metrics() returns a Prometheus snapshot of cumulative counters. The
# ones that explain SharedStorage's cost:
#   vllm:prefix_cache_queries / _hits          -> GPU-side KV cache
#   vllm:external_prefix_cache_queries / _hits -> the OFFLOAD tier (SharedStorage);
#                                                 queries - hits = tier misses -> recompute
#   vllm:num_preemptions                       -> requests evicted mid-flight & re-run
# Poll around each round for deltas (the vLLM-layer analogue of the disk counters).
_PREFIX_KEYS = (
    "vllm:prefix_cache_queries", "vllm:prefix_cache_hits",
    "vllm:external_prefix_cache_queries", "vllm:external_prefix_cache_hits",
    "vllm:num_preemptions",
)

def prefix_stats():
    """Return (gpu_q, gpu_hit, ext_q, ext_hit, preempt); zeros if unavailable."""
    vals = dict.fromkeys(_PREFIX_KEYS, 0.0)
    try:
        for m in llm.get_metrics():
            name = getattr(m, "name", "")
            if name in vals:
                vals[name] = float(getattr(m, "value", 0.0) or 0.0)
    except Exception as e:  # noqa: BLE001
        print(f"[run] get_metrics failed: {e}", file=sys.stderr, flush=True)
    return tuple(int(vals[k]) for k in _PREFIX_KEYS)

def hitrate(hit, q):
    return f"{100.0*hit/q:.1f}%" if q else "n/a"

ps_prev = [prefix_stats()]
round_io = []  # (round, prompts, read_bytes, write_bytes, ...)
disk_pre = [None, None]

def on_round_start(round_idx, n_prompts):
    print(f"[trace] +{time.perf_counter()-t0:.1f}s calling generate round {round_idx} ({n_prompts} prompts)", file=sys.stderr, flush=True)
    disk_pre[0], disk_pre[1] = disk_rw_bytes()

def on_round_end(round_idx, n_prompts, round_elapsed, n_alive):
    rd0, wr0 = disk_pre
    rd1, wr1 = disk_rw_bytes()
    # Delta of cumulative counters = bytes moved during this round.
    d_rd = (rd1 - rd0) if (rd0 is not None and rd1 is not None) else None
    d_wr = (wr1 - wr0) if (wr0 is not None and wr1 is not None) else None
    # vLLM-layer offload/recompute deltas for this round.
    ps_now = prefix_stats()
    d_gq, d_gh, d_eq, d_eh, d_pre = (ps_now[k] - ps_prev[0][k] for k in range(5))
    ps_prev[0] = ps_now
    d_recompute = d_eq - d_eh  # offload-tier misses -> recomputed on GPU
    round_io.append((round_idx, n_prompts, d_rd, d_wr,
                     round_elapsed, d_eq, d_eh, d_recompute, d_pre, d_gq, d_gh))
    print(f"[run] round {round_idx}: {n_prompts} prompts in "
          f"{round_elapsed:.1f}s  ({n_alive} convs still alive)  "
          f"disk_read={gib(d_rd)} disk_write={gib(d_wr)}  "
          f"offload_q={d_eq} offload_hit={d_eh} recompute={d_recompute} "
          f"({hitrate(d_eh, d_eq)} tier hit)  gpu_hit={hitrate(d_gh, d_gq)} "
          f"preempt={d_pre}",
          file=sys.stderr, flush=True)

print(f"[trace] +{time.perf_counter()-t0:.1f}s entering generate loop", file=sys.stderr, flush=True)
result = batched.run_batched(
    llm, convs, sp,
    prompt_budget=PROMPT_BUDGET,
    max_rounds=0,
    n_tokens=n_tokens,
    skip_empty=False,
    on_round_start=on_round_start,
    on_round_end=on_round_end,
)
elapsed = result["elapsed"]
rounds_done = result["rounds_done"]
total_generations = result["total_generations"]
print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} rounds={rounds_done}",
      file=sys.stderr)

# --- Per-round disk I/O + offload/recompute summary --------------------------
# round_io tuple: (round, prompts, d_rd, d_wr, time, ext_q, ext_hit,
#                  recompute, preempt, gpu_q, gpu_hit)
tot_rd = sum(t[2] for t in round_io if t[2] is not None)
tot_wr = sum(t[3] for t in round_io if t[3] is not None)
tot_recompute = sum(t[7] for t in round_io)
tot_preempt = sum(t[8] for t in round_io)
print(f"\n[io] per-round disk bytes + offload counters (dev={DISK_DEV}):", file=sys.stderr)
print(f"[io] {'rnd':>3} {'prmpt':>5} {'read':>11} {'written':>11} "
      f"{'time':>7} {'off_q':>8} {'off_hit':>8} {'recomp':>8} {'preempt':>7}", file=sys.stderr)
for (rnd, npr, rd, wr, te, eq, eh, rc, pre, gq, gh) in round_io:
    print(f"[io] {rnd:>3} {npr:>5} {gib(rd):>11} {gib(wr):>11} "
          f"{te:>6.1f}s {eq:>8} {eh:>8} {rc:>8} {pre:>7}", file=sys.stderr)
print(f"[io] {'TOT':>3} {'':>5} {gib(tot_rd):>11} {gib(tot_wr):>11} "
      f"{'':>7} {'':>8} {'':>8} {tot_recompute:>8} {tot_preempt:>7}", file=sys.stderr)
try:
    io_path = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           f"ss_round_io_{int(elapsed)}.json")
    with open(io_path, "w") as f:
        json.dump({"dev": DISK_DEV, "wall": elapsed, "rounds": [
            {"round": r, "prompts": n, "read_bytes": rd, "write_bytes": wr,
             "time_s": te, "offload_queries": eq, "offload_hits": eh,
             "recompute": rc, "preemptions": pre,
             "gpu_queries": gq, "gpu_hits": gh}
            for (r, n, rd, wr, te, eq, eh, rc, pre, gq, gh) in round_io],
            "total_read_bytes": tot_rd, "total_write_bytes": tot_wr,
            "total_recompute": tot_recompute, "total_preemptions": tot_preempt}, f, indent=2)
    print(f"[io] saved {io_path}", file=sys.stderr)
except OSError as e:
    print(f"[io] could not save json: {e}", file=sys.stderr)

import sys
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

