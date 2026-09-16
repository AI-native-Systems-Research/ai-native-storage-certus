#!/bin/bash
# run-serve-cputier.sh — vLLM OpenAI-compatible API server wired to vLLM's
# native multi-tier KV offload (OffloadingConnector -> TieringOffloadingSpec:
# a CPU primary tier + an "fs" disk secondary tier), for interactive /
# benchmarking clients (guidellm, aiperf, the OpenAI SDK, curl).
#
# This is the "cputier" counterpart of run-serve-shmq.sh: same model, endpoint,
# and engine flags, but instead of the Certus-SHMQ connector (external
# certus-server + CUDA IPC over a /dev/shm mailbox) it uses vLLM 0.26's in-tree
# tiering framework. Run the two side by side to compare backends over the same
# HTTP surface.
#
# Unlike the shmq server this is SELF-CONTAINED: there is no external server, no
# gRPC, and no CUDA IPC. Two things matter instead:
#
#   * The CPU primary tier is a /dev/shm mmap force-populated with
#     MADV_POPULATE_WRITE. Podman's default /dev/shm is 64M, so the populate
#     fails with "OSError: [Errno 14] Bad address" unless the container's
#     /dev/shm is sized to the tier. We pass --shm-size >= CPU_BYTES + headroom.
#   * The "fs" secondary tier writes block files under a directory that must be
#     bind-mounted from the host and writable by the rootless-mapped uid, else
#     the disk tier silently drops stores. We bind ${FS_TIER_HOST}.
#
#   ./run-serve-cputier.sh                                   # Qwen2.5-7B, :8000, 8G CPU tier + fs disk tier
#   PORT=9000 ./run-serve-cputier.sh                         # publish on another port
#   CPU_BYTES=$((16*(1<<30))) ./run-serve-cputier.sh         # larger CPU primary tier (shm auto-sized)
#   MAX_MODEL_LEN=131072 ./run-serve-cputier.sh              # long context (auto-enables YaRN)
#   FS_TIER=0 ./run-serve-cputier.sh                         # CPU-only offload (CPUOffloadingSpec, no disk tier)
#   MODEL=Qwen/Qwen2.5-14B-Instruct GPU_MEM_UTIL=0.92 ./run-serve-cputier.sh
#
# Clients then use:
#   Base URL:  http://127.0.0.1:${PORT}/v1     (127.0.0.1 — podman publishes IPv4 only)
#   Model:     ${SERVED_MODEL_NAME}
#   /metrics:  http://127.0.0.1:${PORT}/metrics  (vLLM + KV-offload counters)
set -euo pipefail

# ── Endpoint ──────────────────────────────────────────────────────────────────
HOST="${HOST:-0.0.0.0}"
PORT="${PORT:-8000}"

# ── Model / engine (kept identical to run-serve-shmq.sh so the two backends are
#    directly comparable over the same HTTP surface) ───────────────────────────
MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b}"   # the name clients pass as "model"
DTYPE="${DTYPE:-float16}"
# Qwen2.5-7B native window is 32768. Default to it so no YaRN rope-scaling is
# needed (a larger MAX_MODEL_LEN triggers a ModelConfig validation error unless
# rope_scaling is supplied — handled below when it exceeds the native window).
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"
QWEN_NATIVE_CTX="${QWEN_NATIVE_CTX:-32768}"
GPU="${GPU:-all}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.90}"

# ── cputier offload (vLLM native OffloadingConnector) ────────────────────────────
CPU_BYTES="${CPU_BYTES:-$((8 * (1 << 30)))}"   # CPU primary tier, pinned host RAM (bytes)
# TieringOffloadingSpec allocates the CPU tier as a /dev/shm mmap and force-
# populates it; size the container's /dev/shm to the tier plus headroom (the
# region is padded past cpu_bytes_to_use), mirroring run-docker-cputier.sh.
SHM_BYTES="${SHM_BYTES:-$((CPU_BYTES + 4 * (1 << 30)))}"
# fs disk secondary tier. FS_TIER=0 drops it -> plain CPU offload (CPUOffloadingSpec).
FS_TIER="${FS_TIER:-1}"
FS_TIER_HOST="${FS_TIER_HOST:-/mnt/certus1/kv-fs-tier}"   # host dir backing the disk tier
FS_TIER_CTR="/workspace/kv-fs-tier"                       # container mount point
FS_READ_THREADS="${FS_READ_THREADS:-16}"
FS_WRITE_THREADS="${FS_WRITE_THREADS:-16}"

# ── Container / store ────────────────────────────────────────────────────────────
# The unified offload image (vLLM 0.26 + the native tiering framework) lives in
# the DEFAULT podman store (unlike the shmq image), so no --root/--runroot flags.
# Default to the -fix026 image: it bakes the vllm-fix2 overlay (the deferred
# finished-request finalize handshake) that stops the TieringOffloadingManager
# _req_state KeyError under load. The bare `certus-offload-bench` is the
# deliberate STOCK/crashing baseline (built --build-arg VLLM_FIX_TIERING=0) and
# is only useful for reproducing that upstream crash; override IMAGE= to get it.
# NB: fix026 does NOT touch the separate `len(offload_keys) == len(offload_block_ids)`
# assertion in _build_store_jobs — that path is identical in both images.
IMAGE="${IMAGE:-certus-offload-bench-fix026}"

# HF cache on the large filesystem — NOT $HOME/.cache (the /home partition is
# small and fills up mid-download).
HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"

# ── Preflight ──────────────────────────────────────────────────────────────────
if ! command podman image exists "$IMAGE"; then
  echo "error: image '$IMAGE' not found in the default podman store." >&2
  echo "       build it first: bash benchmarks/kv-offload-replay/build_026.sh" >&2
  exit 1
fi
if [[ -r /proc/meminfo ]]; then
  avail_bytes=$(( $(awk '/^MemAvailable:/{print $2}' /proc/meminfo) * 1024 ))
  if (( CPU_BYTES > avail_bytes )); then
    echo "warning: CPU_BYTES=${CPU_BYTES} exceeds MemAvailable=${avail_bytes} — the pinned CPU tier may OOM at init." >&2
  fi
  hp="$(awk '/^HugePages_Total:/{print $2}' /proc/meminfo)"
  if [[ -n "${hp:-}" && "${hp}" -gt 0 ]]; then
    echo "warning: ${hp} hugepages reserved on the host — they reduce RAM available to the CPU tier (free them if this host was in Certus mode)." >&2
  fi
fi

# ── KV-transfer config: native OffloadingConnector ──────────────────────────────
# TieringOffloadingSpec (CPU primary + fs secondary) — the same dict the offline
# cputier driver builds (run_otel_replay.py / run_multiturn_offloading.py). With
# FS_TIER=0 we fall back to a single-tier CPUOffloadingSpec (host RAM only).
FS_MOUNT_ARGS=()
if [[ "$FS_TIER" != "0" ]]; then
  mkdir -p "$FS_TIER_HOST"
  FS_MOUNT_ARGS=( -v "${FS_TIER_HOST}:${FS_TIER_CTR}:z" )
  KV_CONFIG=$(cat <<JSON
{"kv_connector":"OffloadingConnector","kv_role":"kv_both","kv_connector_extra_config":{"cpu_bytes_to_use":${CPU_BYTES},"spec_name":"TieringOffloadingSpec","eviction_policy":"lru","secondary_tiers":[{"type":"fs","root_dir":"${FS_TIER_CTR}","n_read_threads":${FS_READ_THREADS},"n_write_threads":${FS_WRITE_THREADS}}]}}
JSON
)
  echo "[serve] cputier: CPU primary ${CPU_BYTES}B + fs disk tier ${FS_TIER_HOST} -> ${FS_TIER_CTR} (r=${FS_READ_THREADS} w=${FS_WRITE_THREADS})"
else
  KV_CONFIG=$(cat <<JSON
{"kv_connector":"OffloadingConnector","kv_role":"kv_both","kv_connector_extra_config":{"cpu_bytes_to_use":${CPU_BYTES},"spec_name":"CPUOffloadingSpec","eviction_policy":"lru"}}
JSON
)
  echo "[serve] cpu-only offload: CPU tier ${CPU_BYTES}B (no disk tier)"
fi

# ── Assemble `vllm serve` args ──────────────────────────────────────────────────
# --no-async-scheduling: the OffloadingConnector serializes KV transfers per
#   request; async scheduling asserts/crashes with it. The offline drivers set
#   async_scheduling=False for the same reason.
SERVE_ARGS=(
  "$MODEL"
  --host "$HOST" --port "$PORT"
  --served-model-name "$SERVED_MODEL_NAME"
  --dtype "$DTYPE"
  --max-model-len "$MAX_MODEL_LEN"
  --gpu-memory-utilization "$GPU_MEM_UTIL"
  --enable-prefix-caching
  --no-async-scheduling
  --kv-transfer-config "$KV_CONFIG"
)

# YaRN rope-scaling: only when the requested window exceeds Qwen2.x's native
# 32768 (Qwen ships no rope_scaling, so vLLM rejects a larger window otherwise).
# Qwen's official recipe is static YaRN with factor = target / native. Opt out
# with ROPE_YARN=0 (which caps you at the native window).
if [[ "${ROPE_YARN:-1}" != "0" && "$MAX_MODEL_LEN" -gt "$QWEN_NATIVE_CTX" && "${MODEL,,}" == *qwen2* ]]; then
  FACTOR=$(awk "BEGIN{printf \"%g\", ${MAX_MODEL_LEN}/${QWEN_NATIVE_CTX}}")
  HF_OVERRIDES=$(cat <<JSON
{"rope_scaling":{"rope_type":"yarn","factor":${FACTOR},"original_max_position_embeddings":${QWEN_NATIVE_CTX}}}
JSON
)
  SERVE_ARGS+=(--hf-overrides "$HF_OVERRIDES")
  echo "[serve] YaRN rope-scaling factor=${FACTOR} (native ${QWEN_NATIVE_CTX} -> max_model_len ${MAX_MODEL_LEN})"
fi

echo "[serve] ${IMAGE}: vllm serve ${MODEL} on ${HOST}:${PORT} (shm-size=${SHM_BYTES})"
echo "[serve] clients -> base_url http://127.0.0.1:${PORT}/v1   model ${SERVED_MODEL_NAME}"

# --shm-size sizes the container's /dev/shm for the CPU tier mmap; -p publishes
# the API port. --pull=never (image is built locally). --entrypoint vllm
# overrides the image's replay-driver entrypoint with the OpenAI API server.
# No --ipc=host and no external server: cputier is self-contained.
exec command podman run --rm --pull=never \
  --shm-size "${SHM_BYTES}" \
  --device "nvidia.com/gpu=${GPU}" \
  -p "${PORT}:${PORT}" \
  -e "HF_HUB_OFFLINE=0" \
  -v "${HF_CACHE}:/root/.cache/huggingface:z" \
  "${FS_MOUNT_ARGS[@]}" \
  --entrypoint vllm \
  "$IMAGE" \
  serve "${SERVE_ARGS[@]}"
