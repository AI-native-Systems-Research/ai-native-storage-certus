#!/bin/bash
# run-serve-shmq.sh — vLLM OpenAI-compatible API server wired to the Certus-SHMQ
# KV-offload connector, for interactive / benchmarking clients (aiperf, ollama,
# the OpenAI SDK, curl). This is the SERVING counterpart of the offline replay
# drivers: instead of driving vLLM through the in-process LLM(...) engine, it
# runs `vllm serve` inside the shmq client image so there is a live
# /v1/chat/completions endpoint on a published port.
#
# Like run-docker-otel-shmq.sh this is CLIENT-ONLY: a certus-server must ALREADY
# be running and publishing the mailbox at SHM_PATH. It does NOT start or manage
# the server (that needs sudo / vfio-pci / hugepages). Bring it up exactly as for
# the replay tool:
#
#   * bind the NVMe device(s) to vfio-pci + reserve 1G hugepages
#     (certus-shmq-connector/setup-host.sh or tools/configure-bench.sh), then
#   * target/release/certus-server --shm-path /dev/shm/certus-shmq ...
#     (benchmarks/kv-offload-otel-replay/run-docker-certus-shmq.sh shows the full
#     server invocation).
#
# The client shares the mailbox via --ipc=host (which also lets the host server
# open this container's CUDA IPC handles). There is no server address — SHM_PATH
# IS the endpoint.
#
#   ./run-serve-certus-shmq.sh                                  # Qwen2.5-7B, native 32k window, :8000
#   PORT=9000 ./run-serve-certus-shmq.sh                        # publish on another port
#   MAX_MODEL_LEN=131072 ./run-serve-certus-shmq.sh             # long context (auto-enables YaRN)
#   MODEL=Qwen/Qwen2.5-14B-Instruct GPU_MEM_UTIL=0.92 ./run-serve-certus-shmq.sh
#
# Clients then use:
#   Base URL:  http://127.0.0.1:${PORT}/v1     (127.0.0.1 — podman publishes IPv4 only)
#   Model:     ${SERVED_MODEL_NAME}
#   /metrics:  http://127.0.0.1:${PORT}/metrics  (vLLM + KV-offload counters)
set -euo pipefail

# ── Endpoint ──────────────────────────────────────────────────────────────────
HOST="${HOST:-0.0.0.0}"
PORT="${PORT:-8000}"

# ── Model / engine ─────────────────────────────────────────────────────────────
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

# ── Certus-SHMQ connector ───────────────────────────────────────────────────────
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"   # mailbox file (shared into container)
SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"  # offload block size — MUST match certus-server

# ── Container / store ────────────────────────────────────────────────────────────
# The shmq image lives in the /mnt/certus1 podman store, not the default store
# (mirrors run-docker-otel-shmq.sh / run-docker-certus-shmq.sh).
IMAGE="${IMAGE:-localhost/certus-otel-shmq-bench}"
PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"
STORE_FLAGS=(--root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT")

# HF cache on the large filesystem — NOT $HOME/.cache (the /home partition is
# small and fills up mid-download).
HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"

# ── Preflight ──────────────────────────────────────────────────────────────────
if ! command podman "${STORE_FLAGS[@]}" image exists "$IMAGE"; then
  echo "error: image '$IMAGE' not found in store ${PODMAN_STORE}." >&2
  echo "       build it first: bash benchmarks/kv-offload-otel-replay/build-otel.sh" >&2
  exit 1
fi
if [[ ! -e "$SHM_PATH" ]]; then
  echo "error: mailbox ${SHM_PATH} does not exist — is certus-server running?" >&2
  echo "       start it first (see the header of this script)." >&2
  exit 1
fi

# ── KV-transfer config: stock OffloadingConnector + CertusShmqOffloadingSpec ────
# Same dict the offline shmq driver builds (run_otel_shmq_certus.py), passed here
# as the `vllm serve --kv-transfer-config` JSON.
KV_CONFIG=$(cat <<JSON
{"kv_connector":"OffloadingConnector","kv_role":"kv_both","kv_connector_extra_config":{"spec_name":"CertusShmqOffloadingSpec","spec_module_path":"certus_shmq_connector.spec","shm_path":"${SHM_PATH}","slab_size_bytes":${SLAB_SIZE_BYTES}}}
JSON
)

# ── Assemble `vllm serve` args ──────────────────────────────────────────────────
# --no-async-scheduling: the OffloadingConnector serializes KV transfers per
#   request; async scheduling asserts/crashes with it (verified on this image's
#   vLLM). The offline driver sets async_scheduling=False for the same reason.
#   (No hybrid-KV flag: this image's vLLM auto-disables the hybrid KV-cache
#   manager when a KV connector needs it — that explicit flag is a >=0.26 concern.)
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

echo "[serve] ${IMAGE}: vllm serve ${MODEL} on ${HOST}:${PORT} (shm=${SHM_PATH}, slab=${SLAB_SIZE_BYTES})"
echo "[serve] clients -> base_url http://127.0.0.1:${PORT}/v1   model ${SERVED_MODEL_NAME}"

# --ipc=host shares the mailbox + exposes CUDA IPC handles; -p publishes the API
# port. --pull=never against the alt store. --entrypoint vllm overrides the
# image's replay-driver entrypoint with the OpenAI API server.
exec command podman "${STORE_FLAGS[@]}" run --rm --pull=never \
  --ipc=host \
  --device "nvidia.com/gpu=${GPU}" \
  -p "${PORT}:${PORT}" \
  -e "HF_HUB_OFFLINE=0" \
  -v "${HF_CACHE}:/root/.cache/huggingface:z" \
  --entrypoint vllm \
  "$IMAGE" \
  serve "${SERVE_ARGS[@]}"
