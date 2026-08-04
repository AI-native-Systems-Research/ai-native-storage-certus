# Runbook — CPU-offload and SharedStorage 450×12 backends

How to run the 450-conversation / 12-turn multi-turn workload against the two
**non-Certus** KV-offload backends, for head-to-head comparison with the Certus
gRPC connector (see `../../certus-grpc-connector/README.md` for the Certus path).

All three backends run the **same** ShareGPT 450-conv × 12-turn workload
(Llama-3-8B, `max_model_len=8192`, `output_tokens=150`, `max_num_seqs=64`) so
per-round time and IO are directly comparable. Dataset:
`../../certus-connector/sharegpt_12turn_450.json` (tracked in-repo).

> **Neither driver configures the host.** They run (SS also *preflight-checks*).
> Host setup is always `sudo tools/configure-bench.sh <mode>` first.

---

## Backend 0 — No offload (GPU-only baseline)

The reference point for the other three: a single vLLM process with **no
`kv_transfer_config` at all**. Prefix caching stays on (matching the offload
runs), so the only difference is that evicted KV is **recomputed on the GPU**
rather than fetched from an offload tier. No tier, no server, no IO.

**Driver:** `run_multiturn_nooffload.py` — same workload/driver loop as the
other backends (same dataset/turns/sampling), just with the connector removed.

### Host setup
None. It needs only a free GPU. (Hugepages left over from Certus mode don't hurt
this backend — there is no host-RAM tier to squeeze.)

### Run
```bash
V=~/kvconn-trace/.venv-v0.20.0/bin/python   # vLLM 0.20.0 venv
DATASET_PATH=$PWD/../../certus-connector/sharegpt_12turn_450.json \
NUM_CONVS=450 MAX_MODEL_LEN=8192 OUTPUT_TOKENS=150 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.90 \
MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 \
$V run_multiturn_nooffload.py 2>&1 | tee nooffload_450.log
```

### Env knobs
`NUM_CONVS` (450) · `MAX_MODEL_LEN` (8192) · `OUTPUT_TOKENS` (150) ·
`MAX_NUM_SEQS` (64) · `GPU_MEM_UTIL` (0.90) · `MODEL` · `DATASET_PATH`.
There is no offload tier, so no `CPU_BYTES` / `DRAM`.

### Container
A self-contained image (`Dockerfile.nooffload`) packages the driver + dataset on
a `vllm/vllm-openai` base (default `v0.20.0`; override with `--build-arg
VLLM_VERSION=...`) — the same base family as the other bench images. No server, no
gRPC, no `--ipc=host`, and no offload tier to size. Its `ENV` defaults match this
section (`NUM_CONVS=450`, 450×12 dataset).
```bash
# build from the repo root (context needs the bench dir + dataset)
podman build -f benchmarks/kv-offload-replay/Dockerfile.nooffload -t certus-nooffload-bench .
# ...or pin a newer vLLM (tag the image so versions don't collide):
podman build --build-arg VLLM_VERSION=0.26.0 \
    -f benchmarks/kv-offload-replay/Dockerfile.nooffload -t certus-nooffload-bench:vllm0.26 .
# run (GPU required; mount the HF cache)
podman run --rm --device nvidia.com/gpu=all \
    -v $HOME/.cache/huggingface:/root/.cache/huggingface \
    certus-nooffload-bench
```

### Notes / gotchas
- This is the **upper bound on recompute cost** — every KV miss is a full GPU
  recompute. The offload backends should beat it once the tier hit rate is high
  enough to offset transfer cost; if one doesn't, the tier isn't paying for itself.

---

## Backend 1 — CPU offload (vLLM built-in `CPUOffloadingSpec`)

KV offload tier lives in **host RAM** (a CUDA pinned buffer). No NVMe, no server
— it is a single self-contained vLLM process. "IO" for this backend is
virtual-memory paging (`/proc/vmstat`), captured by the `_iostat` variant.

**Driver:** `run_multiturn_offloading.py` — uses vLLM's built-in
`OffloadingConnector` + `CPUOffloadingSpec` by default (the same connector
family the Certus gRPC driver uses, so the two are directly comparable). Set
`TRACE_OFFLOAD=1` to swap in the local `Tracing*` wrappers, which additionally
write per-op offload traces (`offloading_mgr_<pid>.jsonl` etc.) at some overhead
— use that only when you want the traces, not for a throughput baseline.

### Host setup
None strictly required — CPU offload needs only RAM + a free GPU. **But** if the
host was previously in Certus mode, **free the hugepages first** — they are
reserved out of RAM and will shrink the tier budget / cause OOM:
```bash
# only if hugepages are reserved (check: grep HugePages_Total /proc/meminfo)
echo 0 | sudo tee /sys/devices/system/node/node0/hugepages/hugepages-1048576kB/nr_hugepages
```
The CPU tier is a **pinned, unswappable** allocation, so it must fit in
*available* RAM — size `CPU_BYTES` below the free-RAM figure or vLLM OOMs at init.

### Run
```bash
V=~/kvconn-trace/.venv-v0.20.0/bin/python   # vLLM 0.20.0 venv
DATASET_PATH=$PWD/../../certus-connector/sharegpt_12turn_450.json \
NUM_CONVS=450 MAX_MODEL_LEN=8192 OUTPUT_TOKENS=150 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.90 \
CPU_BYTES=$((16 * (1<<30))) \                # 16 GiB tier; keep < free RAM
MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 \
$V run_multiturn_offloading.py 2>&1 | tee cpu_offload_450.log
```

### Env knobs
`NUM_CONVS` (450) · `MAX_MODEL_LEN` (8192) · `OUTPUT_TOKENS` (150) ·
`MAX_NUM_SEQS` (64) · `GPU_MEM_UTIL` (0.90) · `CPU_BYTES` (offload tier bytes,
default 4 GiB) · `TRACE_OFFLOAD` (0 = built-in connector, no tracing — default;
1 = Tracing* wrappers) · `MODEL` · `DATASET_PATH`.

### Container
A self-contained image (`Dockerfile.cpu-offload`) packages the driver + dataset
on a `vllm/vllm-openai` base (default `v0.20.0`; override with `--build-arg
VLLM_VERSION=...`). No server, no gRPC, no `--ipc=host`. Its `ENV` defaults match
this section (`NUM_CONVS=450`, `CPU_BYTES=16 GiB`, `TRACE_OFFLOAD=0`, 450×12
dataset). Note: the in-process `CPUOffloadingSpec`/`OffloadingConnector` API
shifted across vLLM releases (the same multi-region change that broke the gRPC
path at 0.23+), so a newer-version image builds but the driver may need
connector-side fixes to run.
```bash
# build from the repo root (context needs the bench dir + dataset)
podman build -f benchmarks/kv-offload-replay/Dockerfile.cpu-offload -t certus-cpu-offload-bench .
# ...or pin a newer vLLM:
podman build --build-arg VLLM_VERSION=0.26.0 \
    -f benchmarks/kv-offload-replay/Dockerfile.cpu-offload -t certus-cpu-offload-bench:vllm0.26 .
# run (GPU required; mount the HF cache; free hugepages first if host was in Certus mode)
podman run --rm --device nvidia.com/gpu=all \
    -v $HOME/.cache/huggingface:/root/.cache/huggingface \
    certus-cpu-offload-bench
```

### Notes / gotchas
- **No preflight.** If the host is mis-set (hugepages eating RAM, GPU busy) the
  run just OOMs or fails mid-init with no host-level hint. Check `nvidia-smi`
  (GPU free) and `free -g` (RAM) before launching.
- The pinned tier never swaps, so paging IO is ≈0 — the interesting result is
  that its per-round *time* still climbs from KV-cache pressure with no IO.

---

## Backend 2 — SharedStorage (`llmd_fs_backend`, RAID0 + XFS)

KV offload tier is a **filesystem** on a 4-drive RAID0. Requires the RAID mounted
and a separate connector package on `PYTHONPATH`.

**Driver:** `run_fs_bench_450.py` — the faithful 450×12 runner with a `preflight()`
that fails fast on a mis-configured box and pins to NUMA node 0 in-process.
(Do **not** use the base `run_fs_bench.py` — that is the multidoc `/data/kv-storage`
bench, not this workload.)

### Host setup
```bash
sudo tools/configure-bench.sh sharedstorage    # builds RAID0+XFS at
                                                # /mnt/fs-backend-bench, binds
                                                # 61-64 to nvme, frees hugepages,
                                                # sets readahead (RAID_READAHEAD_KB)
sudo chown -R "$USER":"$(id -gn)" /mnt/fs-backend-bench/shared-kv   # driver runs as you
```
`configure-bench.sh` sets kernel `mem=32G`; if it reports "reboot required" for
the mem cap, reboot before a *faithful* run (page cache is SS's DRAM tier and
must be capped). Preflight enforces this.

### Run
`run_fs_bench_450.py` pins NUMA itself — do **not** wrap it in `numactl`.
```bash
V=~/kvconn-trace/.venv-v0.20.0/bin/python
DATASET_PATH=$PWD/../../certus-connector/sharegpt_12turn_450.json \
NUM_CONVS=450 MAX_MODEL_LEN=8192 OUTPUT_TOKENS=150 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.90 \
DRAM=$((8 * (1<<30))) \                       # staging RAM (max_staging_memory_gb)
MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 \
PYTHONPATH=/home/bdh/llm-d-kv-cache/kv_connectors/llmd_fs_backend \
$V run_fs_bench_450.py 2>&1 | tee ss_450.log
```

### Env knobs
Same workload knobs as above, plus: `DRAM` (staging buffer bytes →
`max_staging_memory_gb`, default 8 GiB) · `DISK_DEV` (device for
`/sys/block/<dev>/stat` byte accounting, default `md0`) · `BENCH_NUMA_NODE` (0) ·
`BENCH_CPUS` (`0-15,32-47`) · `SKIP_PREFLIGHT=1` to bypass the host checks.

### Container
`Dockerfile.sharedstorage` packages the driver + dataset on a `vllm/vllm-openai`
base (default `v0.20.0`) as the other backends. The catch: the `llmd_fs_backend`
connector is a **compiled torch C++ extension** living in a separate repo, so it
must be built into a wheel whose torch/CUDA/GPU-arch match this base image.
`build-sharedstorage.sh` does that in two steps — it reuses the connector repo's
*own* `Dockerfile.wheel` (no bespoke build logic) with build args overridden to
match, then builds the runtime image installing that wheel.

To target a newer vLLM, pass `VLLM_VERSION` to the helper — **but** because the
wheel's ABI is pinned to the base's torch/CUDA, you must also set the matching
`TORCH_VERSION`/`TORCH_CUDA_INDEX`/`CUDA_BASE_TAG` (below), or the extension
fails to load at runtime. `VLLM_VERSION` alone only re-bases the runtime image,
not the wheel.

```bash
# 1+2. Build the wheel (matched args) then the image. Defaults target this host
#      (torch 2.11.0/cu130, A30 = sm_80). Override via env if the base image's
#      torch or the GPU differs:
#        TORCH_VERSION / TORCH_CUDA_INDEX  — match the base image's torch
#          (check: <venv>/bin/python -c "import torch;print(torch.__version__,torch.version.cuda)")
#        CUDA_BASE_TAG                     — CUDA devel base for that cu index
#        TORCH_CUDA_ARCH_LIST              — target GPU compute cap
#          (check: nvidia-smi --query-gpu=compute_cap --format=csv,noheader)
#        FS_BACKEND_DIR                    — path to the llmd_fs_backend repo
#        VLLM_VERSION                      — runtime base tag (default 0.20.0);
#          if bumped, set the torch args above to match or the wheel won't load
benchmarks/kv-offload-replay/build-sharedstorage.sh

# Run: bind-mount the host RAID (from configure-bench.sh sharedstorage) + HF cache.
# The KV tier path (/mnt/fs-backend-bench/shared-kv) is fixed in the driver.
podman run --rm --device nvidia.com/gpu=all \
    -v /mnt/fs-backend-bench:/mnt/fs-backend-bench \
    -v $HOME/.cache/huggingface:/root/.cache/huggingface \
    certus-sharedstorage-bench
#   docker: --gpus all. Add -e SKIP_PREFLIGHT=1 to bypass the mount/RAM-cap
#   checks (e.g. a smoke run where the bind mount isn't a real mountpoint).
```
The built wheel lands in `benchmarks/kv-offload-replay/wheels/` (gitignored —
rebuilt by the script, not committed). The wheel is installed `--no-deps` so it
can't replace the base image's torch/vLLM.

### Notes / gotchas
- **Preflight checks, does not configure** — it verifies the RAID is mounted, the
  KV path is writable, and RAM is capped, then exits loud if not. Fix with
  `configure-bench.sh sharedstorage`.
- **Known instability:** SharedStorage can **deadlock the vLLM engine** under
  write pressure (`Write queue full … dropped writes` → requests stuck in
  `WAITING_FOR_REMOTE_KVS`, GPU 0% / disk 0 B/s). Observed ~4/10 runs; the lever
  is a larger `DRAM` staging cap, not readahead.

---

## Capturing vLLM-layer offload/recompute metrics (`_iostat` variants)

The base drivers set `disable_log_stats=True`. The **`_iostat`** variants flip it
off and poll `LLM.get_metrics()` per round, emitting the vLLM-internal counters:

- **CPU:** `run_multiturn_offloading_iostat.py` → per-round `pgin/pgout/swin/swout/majflt`
  (virtual-memory paging deltas from `/proc/vmstat`).
- **SS:** `run_fs_bench_450_iostat.py` → per-round `offload_q / offload_hit /
  recompute / tier-hit% / gpu-hit% / preempt` (from `vllm:external_prefix_cache_*`
  and `vllm:num_preemptions`) alongside the `/sys/block/md0/stat` disk bytes.

Same invocation as the base driver, just swap the script name. Use these when you
need to explain *why* a backend is slow (tier misses → recompute, preemptions)
rather than only *how much* IO it moved.

## The backends at a glance

| Backend | Driver (this dir unless noted) | Offload tier | Host setup |
|---|---|---|---|
| No offload | `run_multiturn_nooffload.py` | none (GPU recompute) | none |
| Certus | `certus-grpc-connector/run_multiturn_grpc_certus.py` + `run-bench.sh` | DRAM (SPDK hugepages) + NVMe | `configure-bench.sh certus` |
| CPU offload | `run_multiturn_offloading.py` | host RAM (pinned) | free hugepages; else none |
| SharedStorage | `run_fs_bench_450.py` | RAID0 XFS filesystem | `configure-bench.sh sharedstorage` |
