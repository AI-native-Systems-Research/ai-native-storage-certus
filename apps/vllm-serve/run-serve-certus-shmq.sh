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
#   MAX_MODEL_LEN=32768 ./run-serve-certus-shmq.sh             # native 32K window (no YaRN); default is 128K
#   MODEL=Qwen/Qwen2.5-14B-Instruct GPU_MEM_UTIL=0.92 ./run-serve-certus-shmq.sh
#   KV_CACHE_BYTES=4G ./run-serve-certus-shmq.sh              # cap GPU KV cache; spill reuse to the offload tier
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
# Qwen2.5-7B's native window is 32768; default to a 131072 (128K) window so the
# long-context cc/mooncake replay traces fit without 400s. Because this exceeds the
# native window it auto-enables Qwen's static YaRN rope-scaling below (factor =
# 131072/32768 = 4). YaRN degrades quality at long context — fine for a KV-offload
# throughput/latency benchmark. Override MAX_MODEL_LEN=32768 (or ROPE_YARN=0) to
# serve the native 32K window with no rope-scaling.
MAX_MODEL_LEN="${MAX_MODEL_LEN:-131072}"
QWEN_NATIVE_CTX="${QWEN_NATIVE_CTX:-32768}"
GPU="${GPU:-all}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.90}"
# Shard the model across this many GPUs. TP>1 is REQUIRED for large windows on
# 40G A100s: a 131K sequence's KV (~24G) plus the 14B weights (~29G) does not fit
# one GPU and vLLM aborts at the startup KV-cache sizing check. GPU=all only makes
# both devices visible — it does NOT enable sharding; that needs this flag.
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"
# Directly cap the GPU-resident KV cache (per GPU). When set, vLLM IGNORES
# gpu-memory-utilization and pins the KV pool to this size — accepts human-
# readable sizes (4G, 512M). Shrinking it forces reused prefixes to spill from
# GPU HBM into the offload tier, which is what raises the external prefix cache
# hit rate. Unset (default) = derive KV size from GPU_MEM_UTIL (stock behavior).
KV_CACHE_BYTES="${KV_CACHE_BYTES:-}"

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

# ── fix#3: _build_store_jobs length-clamp patch ─────────────────────────────────
# The stock image's OffloadingConnector scheduler crashes the engine under load
# on `assert len(offload_keys) == len(offload_block_ids)` in _build_store_jobs
# (offload_keys advances every step; block_ids only grows on new allocations, so
# a finishing request can cross an unbacked chunk boundary). This bind-mounts a
# scheduler.py that clamps num_chunks to the chunks that have both a key and
# backing GPU blocks. shmq-only variant — it deliberately does NOT carry fix#2's
# mark_stores_submitted handshake (CertusShmqOffloadingSpec's manager lacks it).
# Set VLLM_FIX3=0 to run the stock (crash-prone) scheduler.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VLLM_FIX3="${VLLM_FIX3:-1}"
FIX3_SCHEDULER="${FIX3_SCHEDULER:-${SCRIPT_DIR}/patches/scheduler.fix3.py}"
FIX3_TARGET=/usr/local/lib/python3.12/dist-packages/vllm/distributed/kv_transfer/kv_connector/v1/offloading/scheduler.py
FIX3_MOUNT=()
if [[ "$VLLM_FIX3" == "1" ]]; then
  if [[ -f "$FIX3_SCHEDULER" ]]; then
    FIX3_MOUNT=(-v "${FIX3_SCHEDULER}:${FIX3_TARGET}:ro,z")
    echo "[serve] fix#3 scheduler patch: ${FIX3_SCHEDULER} -> in-container scheduler.py"
  else
    echo "warning: VLLM_FIX3=1 but patch not found at ${FIX3_SCHEDULER}; running STOCK scheduler (crash-prone under load)" >&2
  fi
fi

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
  --tensor-parallel-size "$TENSOR_PARALLEL"
  --gpu-memory-utilization "$GPU_MEM_UTIL"
  --enable-prefix-caching
  --no-async-scheduling
  --kv-transfer-config "$KV_CONFIG"
)

# Optional hard cap on the GPU KV cache (overrides gpu-memory-utilization).
if [[ -n "$KV_CACHE_BYTES" ]]; then
  SERVE_ARGS+=(--kv-cache-memory-bytes "$KV_CACHE_BYTES")
  echo "[serve] GPU KV cache capped at ${KV_CACHE_BYTES} (ignores GPU_MEM_UTIL)"
fi

# Extra pass-through `vllm serve` flags for wrappers that need model-specific
# options this generic script doesn't model (e.g. multimodal --limit-mm-per-prompt
# / --mm-processor-kwargs for a VL model). Word-split, so quote per-shell rules;
# for a JSON value with spaces set the flag and value as one element via an array
# is not possible through env, so keep JSON compact (no spaces). Appended last so
# it can also override earlier flags. Unset (default) = no extra args.
if [[ -n "${EXTRA_SERVE_ARGS:-}" ]]; then
  # Split on whitespace only. Disable brace expansion first so a JSON value like
  # {"image":2,"video":0} (comma inside braces) is NOT expanded into two words.
  set +B
  # shellcheck disable=SC2206  # intentional word-split of caller-provided flags
  EXTRA_ARR=(${EXTRA_SERVE_ARGS})
  set -B
  SERVE_ARGS+=("${EXTRA_ARR[@]}")
  echo "[serve] extra serve args: ${EXTRA_SERVE_ARGS}"
fi

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
  "${FIX3_MOUNT[@]}" \
  --entrypoint vllm \
  "$IMAGE" \
  serve "${SERVE_ARGS[@]}"
