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

## Generating the corpus (`trace-gen/`)

The corpus is derived from an **inference-perf `conversation_replay` workload**,
not captured from a live service, so it is fully reproducible from `(seed,
config)`. `trace-gen/` holds the exact scripts and configs used to build the
shipped `otel_1k`, plus a wrapper that chains them:

| File | Role |
|---|---|
| `conversation_replay_50turn.yaml` | inference-perf `conversation_replay` workload (seed 42, 1000 convs, ~50 turns/conv, lognormal input / normal output token sizes, skew-normal tool-call delays). |
| `gen_conversation_trace.py` | **Stage 1.** Runs inference-perf's `ConversationReplayDataGenerator` and dumps the workload **plan** — per-turn `[input_tokens, output_tokens, tool_call_latency_sec]` — as a compact JSON. Filler *text* is not materialised here (only sizes), so this stays small and fast. |
| `trace_to_otel.py` | **Stage 2.** Expands the plan into the `otel_trace_replay` corpus: one JSON file per conversation, each turn an LLM span with `gen_ai.usage.*` token sizes; user messages carry Qwen-tokenised random filler, assistant messages are short markers substituted at replay; inter-turn delays become timestamp gaps; a 120k sliding window caps context. |
| `otel_replay_1k.yaml` | The inference-perf `otel_trace_replay` config that replays the resulting corpus (`trace_directory`, `concurrent_sessions 50`, `max_wait_ms`). Reference config; the container drivers here replay the same corpus directly. |
| `generate-otel-corpus.sh` | Convenience wrapper that runs both stages with the defaults that reproduce `otel_1k`. |

```bash
cd benchmarks/kv-offload-otel-replay/trace-gen

# Reproduce the shipped otel_1k (seed 42, 1000 convs, 120k cap, Qwen2.5-7B):
./generate-otel-corpus.sh
# -> writes /mnt/certus1/inference-perf-syn-data/otel_1000/ (+ .plan.json alongside)

# Smaller corpus / different knobs (all env-overridable):
NUM_CONVS=100 CAP=120000 OUT_DIR=/mnt/certus1/inference-perf-syn-data/otel_100 \
    ./generate-otel-corpus.sh

# Or the two stages by hand:
python3.12 gen_conversation_trace.py conversation_replay_50turn.yaml plan.json 1000
python3.12 trace_to_otel.py plan.json /mnt/certus1/inference-perf-syn-data/otel_1k \
    --num 1000 --cap 120000 --model Qwen/Qwen2.5-7B-Instruct
```

Requires the `inference-perf` package importable under `python3.12` and the
model tokenizer available to `transformers` (HF cache or network). The corpus is
large (~22 GB at 1000 convs / 120k cap) — write it to `/mnt/certus1`, then point
a run at it with `OTEL_HOST=<out_dir>` (see below).

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

## Run — cputier (`run-docker-otel-cputier.sh`)

Dedicated entry point for the **CPU+Disk tiered** arm (vLLM 0.26 native
`OffloadingConnector` → `TieringOffloadingSpec`: CPU primary tier backed by an
`fs` disk secondary tier) — the OTel counterpart of
`../kv-offload-replay/run-docker-cputier.sh`. It is a thin wrapper that pins
`SECONDARY_TIER=fs` and execs `run-docker-otel-offload.sh` (which owns the
tiering mechanics: `--shm-size` for the `/dev/shm` CPU-tier mmap, the fs-tier
bind, disk IO threads), so it's equivalent to the `SECONDARY_TIER=fs` invocation
above but with sensible cputier defaults and its own log name.

```bash
cd benchmarks/kv-offload-otel-replay
./run-docker-otel-cputier.sh                                    # CPU 8G primary + fs disk tier
CPU_BYTES=$((32*(1<<30))) DISK_DIR_HOST=/mnt/certus1/kv-fs-tier ./run-docker-otel-cputier.sh
NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-cputier.sh           # quick smoke
```

`CPU_BYTES` (primary tier, default 8 GiB), `DISK_DIR_HOST` (fs-tier backing dir,
default `/mnt/certus1/kv-fs-tier`, must be writable by the mapped container uid),
`SHM_BYTES` (default `CPU_BYTES + 4 GiB`), and `DISK_{READ,WRITE}_THREADS`
(default 16) tune the tiers. Writes `otel_replay_results.json`; log teed to
`otel_cputier_<HHMMSS>.log`.

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

## Prometheus metrics (`*-prom.sh`)

The OTel counterpart of `../kv-offload-replay/run-docker-*-prom.sh`. The bench
drives vLLM through the offline `LLM(...)` engine (no OpenAI server), so there is
no `/metrics` endpoint unless the driver opens one — which the OTel driver does
(`run_otel_async` calls `start_prom_exporter()`, **baked into both images**). So
exposing metrics needs no rebuild: set `PROM_PORT` + `LOG_STATS=1` and publish
the port. The `*-prom.sh` wrappers set those defaults and delegate to the base
runners; the base runners also honor `PROM_PORT` directly.

```bash
cd benchmarks/kv-offload-otel-replay
./run-docker-otel-offload-prom.sh                 # CPUOffload + exporter on :8000
OFFLOAD_MODE=none ./run-docker-otel-offload-prom.sh
./run-docker-otel-cputier-prom.sh                 # cputier (CPU+fs tiered) + exporter
./run-docker-otel-shmq-prom.sh                    # shmq client + exporter (host server up)
PROM_PORT=9100 ./run-docker-otel-offload-prom.sh  # pick another port
```

Scrape from the host at `http://127.0.0.1:${PROM_PORT}/metrics` (podman
publishes IPv4 only — use `127.0.0.1`, not `localhost`/`::1`). For shmq these are
the **client-side** vLLM + KV-offload metrics; the SPDK/SSD counters live in the
host `certus-server` and are not in this registry. All base-script knobs pass
through unchanged; `DRIVER_SRC=<dir>` optionally re-mounts the repo drivers over
the baked copies so driver edits take effect without a rebuild.

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
Prometheus (both): `PROM_PORT` (unset = off; the `*-prom.sh` wrappers default it
to `8000`), `LOG_STATS` (default `1` under `*-prom.sh`), `DRIVER_SRC` (no-rebuild
driver re-mount).

> **Real timing runs long.** At `TIME_SCALE=1.0` the deepest conversations carry
> ~2000–2800 s of recorded think/tool-call delay, so a full-corpus run's
> wall-clock is dominated by the longest conversation. Use a small `NUM_CONVS`
> and `TIME_SCALE=0` for smoke tests.
