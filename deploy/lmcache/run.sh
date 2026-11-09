#!/bin/bash
# run.sh — start LMCache (MP server: CPU L1 spilling to an SSD L2) and vLLM 0.30
# wired to it, and wait until the OpenAI endpoint is healthy (rootless; no sudo).
#
#   ./run.sh                                  # Qwen2.5-7B, :8000, 8G L1 + 200G L2 on /cache/lmcache
#   CPU_GB=2 ./run.sh                         # small L1: chunks reach the SSD quickly (smoke.sh default)
#   DISK_GB=1000 DISK_DIR=/other/nvme ./run.sh
#   USE_ODIRECT=0 ./run.sh                    # buffered L2 I/O (page cache absorbs reads — not an SSD test)
#   MODEL=Qwen/Qwen2.5-14B-Instruct SERVED_MODEL_NAME=qwen2.5-14b ./run.sh
#   FOREGROUND=1 ./run.sh                     # vLLM attached; Ctrl-C stops both (the bench scripts use this)
#
#  ┌ certus-lmcache-server ─────────────┐  ZMQ :5555   ┌ certus-lmcache-vllm ────────────┐
#  │ lmcache server                     │◀────────────▶│ vllm serve … :8000              │
#  │  L1 pinned RAM  CPU_GB   (LRU)     │  CUDA IPC    │  kv_connector=LMCacheMPConnector│
#  │  L2 fs_native   DISK_GB  O_DIRECT ─┼──▶ DISK_DIR  │  GPU prefix cache (HBM)         │
#  │  HTTP /status /metrics :8080       │  (SSD)       └─────────────────────────────────┘
#  └────────────────────────────────────┘
#
# MP mode (not the in-process LMCacheConnectorV1) because the in-process GPU
# connector corrupts KV on vLLM 0.30's fused-K/V layout — see Containerfile.
#
# Both containers:
#   --device nvidia.com/gpu=$GPU   the server maps vLLM's KV tensors by CUDA IPC
#   --security-opt label=disable   SELinux enforcing blocks the CDI GPU nodes otherwise
#                                  ("Failed to initialize NVML: Insufficient Permissions")
#   --ipc host --network host      CUDA IPC handles + ZMQ on 127.0.0.1
#   --ulimit memlock=-1            L1 is pinned (locked) host memory
# vLLM: VLLM_SERVER_DEV_MODE=1 exposes POST /reset_prefix_cache, which smoke.sh and
# the bench scripts use to drop the GPU prefix cache so the next hit is LMCache's.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

NAME="${NAME:-certus-lmcache}"
SRV="${NAME}-server"
VLM="${NAME}-vllm"
IMAGE="${IMAGE:-localhost/vllm-lmcache-bench:vllm0.30.0}"
HOST="${HOST:-0.0.0.0}"
PORT="${PORT:-8000}"

# ── Model / engine ────────────────────────────────────────────────────────────
MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b}"
DTYPE="${DTYPE:-float16}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"
GPU="${GPU:-0}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.90}"
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"
KV_CACHE_BYTES="${KV_CACHE_BYTES:-}"            # e.g. 4G: cap GPU KV so reuse must come from LMCache
QWEN_NATIVE_CTX="${QWEN_NATIVE_CTX:-32768}"

# ── LMCache server ────────────────────────────────────────────────────────────
LMC_PORT="${LMC_PORT:-5555}"                    # ZMQ (vLLM ⇄ server)
LMC_HTTP_PORT="${LMC_HTTP_PORT:-8080}"          # /status, /metrics
CHUNK_SIZE="${CHUNK_SIZE:-256}"                 # tokens per chunk = per L2 file
CPU_GB="${CPU_GB:-8}"                           # L1, pinned host RAM
DISK_DIR="${DISK_DIR:-/cache/lmcache}"          # host SSD dir (nvme0n1 XFS here)
DISK_GB="${DISK_GB:-200}"                       # L2 capacity, evicted beyond it
USE_ODIRECT="${USE_ODIRECT:-1}"                 # bypass the page cache so L2 reads hit the SSD
DISK_IO_THREADS="${DISK_IO_THREADS:-8}"         # fs_native C++ I/O workers (LMCache default 4)
LMC_WORKERS="${LMC_WORKERS:-1}"                 # server STORE/RETRIEVE + LOOKUP worker threads
# L2 is cleared on start: a fresh server has an empty index, so old chunk files
# would only take space. KEEP_DISK=1 leaves them.
KEEP_DISK="${KEEP_DISK:-0}"
L2_HOST="${DISK_DIR}/l2"

HF_CACHE="${HF_CACHE:-}"
if [[ -z "$HF_CACHE" ]]; then
  HF_CACHE=/mnt/certus1/hf-cache
  [[ -d "$HF_CACHE" ]] || HF_CACHE="$HOME/.cache/huggingface"
fi

# ── Preflight ─────────────────────────────────────────────────────────────────
podman image exists "$IMAGE" || { echo "error: $IMAGE not in the default podman store — ./build.sh first" >&2; exit 1; }
for c in "$SRV" "$VLM"; do
  if podman container exists "$c"; then
    echo "error: container '$c' already exists (running or stopped). ./teardown.sh first." >&2
    exit 1
  fi
done
for p in "$PORT" "$LMC_PORT" "$LMC_HTTP_PORT"; do
  if ss -ltnH "sport = :$p" | grep -q .; then
    echo "error: port $p is already in use (--network host)" >&2; exit 1
  fi
done
mkdir -p "$L2_HOST" "$HF_CACHE"
if [[ "$KEEP_DISK" != "1" ]] && [[ -n "$(find "$L2_HOST" -mindepth 1 -print -quit)" ]]; then
  echo "[run] clearing $(du -sh "$L2_HOST" | cut -f1) of old L2 chunks in ${L2_HOST} (KEEP_DISK=1 to keep)"
  find "$L2_HOST" -mindepth 1 -delete
fi
avail_ram_gb=$(( $(awk '/^MemAvailable:/{print $2}' /proc/meminfo) / 1024 / 1024 ))
(( CPU_GB < avail_ram_gb )) || echo "warning: CPU_GB=${CPU_GB} >= MemAvailable ${avail_ram_gb}G — pinning L1 may fail" >&2

COMMON=(
  --pull=never
  --device "nvidia.com/gpu=${GPU}"
  --security-opt label=disable
  --ipc host --network host
  --ulimit memlock=-1:-1
)

# ── LMCache server ────────────────────────────────────────────────────────────
ODIRECT_JSON=$([[ "$USE_ODIRECT" == "1" ]] && echo true || echo false)
L2_ADAPTER="{\"type\":\"fs_native\",\"base_path\":\"/lmcache-disk/l2\",\"use_odirect\":${ODIRECT_JSON},\"num_workers\":${DISK_IO_THREADS},\"max_capacity_gb\":${DISK_GB}}"
SRV_ARGS=(
  lmcache-server
  --host 127.0.0.1 --port "$LMC_PORT"
  --http-host 127.0.0.1 --http-port "$LMC_HTTP_PORT"
  --chunk-size "$CHUNK_SIZE"
  --l1-size-gb "$CPU_GB"
  --eviction-policy LRU
  --max-workers "$LMC_WORKERS"
  --l2-adapter "$L2_ADAPTER"
)
if [[ -n "${EXTRA_LMCACHE_ARGS:-}" ]]; then
  set +B
  # shellcheck disable=SC2206  # intentional word-split of caller-provided flags
  SRV_ARGS+=(${EXTRA_LMCACHE_ARGS})
  set -B
fi

echo "[run] lmcache: chunk=${CHUNK_SIZE} L1=${CPU_GB}G -> L2 ${L2_HOST} ${DISK_GB}G (odirect=${ODIRECT_JSON}, io_threads=${DISK_IO_THREADS})"
podman run -d --name "$SRV" "${COMMON[@]}" \
  -e L2_DIR=/lmcache-disk/l2 -e DISK_GB="$DISK_GB" -e USE_ODIRECT="$USE_ODIRECT" \
  -v "${DISK_DIR}:/lmcache-disk" \
  "$IMAGE" "${SRV_ARGS[@]}" >/dev/null
for _ in $(seq 1 120); do
  curl -sf "http://127.0.0.1:${LMC_HTTP_PORT}/status" 2>/dev/null | grep -q '"is_healthy":true' && break
  if [[ "$(podman inspect -f '{{.State.Running}}' "$SRV" 2>/dev/null)" != "true" ]]; then
    echo "[run] lmcache server exited during startup:" >&2
    podman logs --tail 40 "$SRV" >&2
    exit 1
  fi
  sleep 1
done
curl -sf "http://127.0.0.1:${LMC_HTTP_PORT}/status" | grep -q '"is_healthy":true' \
  || { echo "[run] lmcache server not healthy; see: podman logs ${SRV}" >&2; exit 1; }
echo "[run] lmcache server up: zmq 127.0.0.1:${LMC_PORT}, http://127.0.0.1:${LMC_HTTP_PORT}/status"

# ── vLLM ──────────────────────────────────────────────────────────────────────
# --no-async-scheduling: as with the sibling connectors (HF3FS, Certus-SHMQ),
# KV-transfer connectors and async scheduling don't mix.
KV_CONFIG="{\"kv_connector\":\"LMCacheMPConnector\",\"kv_role\":\"kv_both\",\"kv_connector_extra_config\":{\"lmcache.mp.host\":\"tcp://127.0.0.1\",\"lmcache.mp.port\":${LMC_PORT}}}"
SERVE_ARGS=(
  "$MODEL"
  --host "$HOST" --port "$PORT"
  --served-model-name "$SERVED_MODEL_NAME"
  --dtype "$DTYPE"
  --max-model-len "$MAX_MODEL_LEN"
  --tensor-parallel-size "$TENSOR_PARALLEL"
  --gpu-memory-utilization "$GPU_MEM_UTIL"
  --enable-prefix-caching
  --no-async-scheduling
  --kv-transfer-config "$KV_CONFIG"
)
[[ -n "$KV_CACHE_BYTES" ]] && SERVE_ARGS+=(--kv-cache-memory-bytes "$KV_CACHE_BYTES")
# YaRN only when the window exceeds Qwen2.x's native 32768 (as run-serve-cputier.sh).
if [[ "${ROPE_YARN:-1}" != "0" && "$MAX_MODEL_LEN" -gt "$QWEN_NATIVE_CTX" && "${MODEL,,}" == *qwen2* ]]; then
  FACTOR=$(awk "BEGIN{printf \"%g\", ${MAX_MODEL_LEN}/${QWEN_NATIVE_CTX}}")
  SERVE_ARGS+=(--hf-overrides "{\"rope_scaling\":{\"rope_type\":\"yarn\",\"factor\":${FACTOR},\"original_max_position_embeddings\":${QWEN_NATIVE_CTX}}}")
  echo "[run] YaRN rope-scaling factor=${FACTOR}"
fi
if [[ -n "${EXTRA_SERVE_ARGS:-}" ]]; then
  set +B
  # shellcheck disable=SC2206  # intentional word-split of caller-provided flags
  SERVE_ARGS+=(${EXTRA_SERVE_ARGS})
  set -B
  echo "[run] extra serve args: ${EXTRA_SERVE_ARGS}"
fi

echo "[run] vllm: ${MODEL} on :${PORT}  GPU=${GPU} TP=${TENSOR_PARALLEL} window=${MAX_MODEL_LEN}${KV_CACHE_BYTES:+ gpu-kv=${KV_CACHE_BYTES}}"
VLM_RUN=(
  --name "$VLM" "${COMMON[@]}"
  -e VLLM_SERVER_DEV_MODE=1
  -v "${HF_CACHE}:/root/.cache/huggingface"
  "$IMAGE" "${SERVE_ARGS[@]}"
)

if [[ "${FOREGROUND:-0}" == "1" ]]; then
  trap 'podman rm -f "$SRV" >/dev/null 2>&1 || true' EXIT
  podman run --rm "${VLM_RUN[@]}"
  exit $?
fi

podman run -d "${VLM_RUN[@]}" >/dev/null
echo "[run] started ${VLM}; waiting for /health (logs: podman logs -f ${VLM})"
for _ in $(seq 1 900); do
  if curl -sf "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
    echo "[run] up: http://127.0.0.1:${PORT}/v1  model ${SERVED_MODEL_NAME}. Next: ./smoke.sh"
    exit 0
  fi
  if [[ "$(podman inspect -f '{{.State.Running}}' "$VLM" 2>/dev/null)" != "true" ]]; then
    echo "[run] vllm exited during startup:" >&2
    podman logs --tail 60 "$VLM" >&2
    exit 1
  fi
  sleep 1
done
echo "[run] timed out waiting for /health; see: podman logs ${VLM}" >&2
exit 1
