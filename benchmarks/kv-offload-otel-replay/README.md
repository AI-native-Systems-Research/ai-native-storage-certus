# kv-offload-otel-replay — containerized

Container harness for **kv-offload-otel-replay**: the timestamp-scheduled replay
of the `otel_trace_replay` corpus (built by `configs/trace_to_otel.py`, default
`/mnt/certus1/inference-perf-syn-data/otel_1k`) across the KV-offload backends.
The drivers live in `../kv-offload-replay/` (`run_otel_replay.py`,
`run_otel_async.py`, `otel_corpus.py`) and `../../certus-shmq-connector/`
(`run_otel_shmq_certus.py`); this directory packages them into images and run
wrappers, the OTel counterpart of `../kv-offload-replay/`'s `Dockerfile.offload`
/ `certus-shmq-connector/Dockerfile` + `run-docker-*.sh`.

Each conversation is one async coroutine that **sleeps the recorded inter-turn
gap** (`start_time[k] − end_time[k−1]`) before each turn, so wall-clock turn
spacing reproduces the workload instead of saturating the engine. Per-turn
`max_tokens` come from the spans; context is rebuilt from vLLM's own generations
under a 120k sliding window.

## The corpus is bind-mounted, not baked in

Unlike the ShareGPT images (small JSON `COPY`'d in), the OTel corpus is **~22 GB**
(1000 files). It is **bind-mounted read-only at run time** onto `OTEL_DIR`
(`/workspace/otel-corpus`); the entrypoints fail fast if the mount is missing.
The run wrappers mount `OTEL_HOST` (default
`/mnt/certus1/inference-perf-syn-data/otel_1k`) for you.

## Two images

| Image | Backend(s) | Driver | Notes |
|---|---|---|---|
| `certus-otel-offload-bench` | NoOffload / CPUOffload / Tiered-CPU-FS (by env) | `run_otel_replay.py` | Self-contained: one vLLM process, no server, no `--ipc=host`. Bakes the 0.26 tiering fix by default. |
| `certus-otel-shmq-bench` | Certus shmq | `run_otel_shmq_certus.py` | Client only — needs a host `certus-server` on `SHM_PATH` + `--ipc=host`. Lives in the `/mnt/certus1` podman store. |

## Build

```bash
# both images (vLLM 0.26.0); offload -> default store, shmq -> /mnt/certus1 store
bash benchmarks/kv-offload-otel-replay/build-otel.sh
# or just one:  ONLY=offload ./build-otel.sh   |   ONLY=shmq ./build-otel.sh
# or by hand (context = repo root):
podman build -f benchmarks/kv-offload-otel-replay/Dockerfile.otel-offload -t certus-otel-offload-bench .
```

## Run — offload family (`run-docker-otel-offload.sh`)

```bash
cd benchmarks/kv-offload-otel-replay

# CPUOffload (default) — pinned host RAM tier
./run-docker-otel-offload.sh

# quick smoke: 8 conversations, saturating (skip the recorded waits)
NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-offload.sh

# NoOffload GPU-only baseline
OFFLOAD_MODE=none ./run-docker-otel-offload.sh

# Tiered CPU+FS (adds --shm-size >= CPU_BYTES + a host fs-tier bind)
SECONDARY_TIER=fs CPU_BYTES=$((8*(1<<30))) DISK_DIR_HOST=/mnt/certus1/kv-fs-tier \
    ./run-docker-otel-offload.sh
```

Writes `otel_replay_results.json` inside the container; the run log is teed to
`otel_offload_<HHMMSS>.log`.

## Run — Certus shmq (`run-docker-otel-shmq.sh`)

Client-only: start a `certus-server` on the host first (vfio-pci + hugepages +
`target/release/certus-server --shm-path /dev/shm/certus-shmq ...`; see
`../kv-offload-replay/run-docker-certus-shmq.sh` for the full server invocation
and `../../certus-shmq-connector/setup-host.sh` for host setup). Then:

```bash
cd benchmarks/kv-offload-otel-replay
NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-shmq.sh          # quick smoke
SHM_PATH=/dev/shm/certus-shmq SLAB_SIZE_BYTES=2097152 ./run-docker-otel-shmq.sh
```

Writes `otel_shmq_results.json` (`"backend": "certus-shmq"`); log teed to
`otel_shmq_<HHMMSS>.log`.

## Common knobs (both backends)

| Env | Default | Meaning |
|---|---|---|
| `OTEL_HOST` | `/mnt/certus1/inference-perf-syn-data/otel_1k` | Host corpus dir (bind-mounted → `OTEL_DIR`). |
| `NUM_CONVS` | *(unset = ALL files)* | Conversations to replay. |
| `TIME_SCALE` | `1.0` | `1.0` = real recorded timing (can run **very** long — wall ≈ longest conv); `0` = saturating; `0.1` = 10× faster. |
| `MODEL` | `Qwen/Qwen2.5-7B-Instruct` | Corpus tokenizer. |
| `MAX_MODEL_LEN` / `CONTEXT_CAP` | `131072` / `120000` | Window / sliding-context cap. |
| `ACTIVE_SESSIONS` | `0` | `0` = open loop; `N` = closed loop (N active convs). |
| `MAX_NUM_SEQS`, `GPU_MEM_UTIL`, `GPU`, `TENSOR_PARALLEL_SIZE`, `ENFORCE_EAGER`, `DP_RANK`/`DP_SIZE`, `HF_CACHE` | — | As in `../kv-offload-replay/run-docker-common.sh`. |

Offload-only: `OFFLOAD_MODE=none`, `SECONDARY_TIER=fs`, `CPU_BYTES`, `DISK_DIR_HOST`.
shmq-only: `SHM_PATH`, `SLAB_SIZE_BYTES`, `WAIT_SECS`.

> **Real timing runs long.** At `TIME_SCALE=1.0` the deepest conversations carry
> ~2000–2800 s of recorded think/tool-call delay, so a full-corpus run's
> wall-clock is dominated by the longest conversation. Use a small `NUM_CONVS`
> and `TIME_SCALE=0` for smoke tests.
