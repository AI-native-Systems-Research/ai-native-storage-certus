#!/bin/bash
# run-server-h3fs0-cc131k.sh — SERVER side of the 131K cc-traces KV-offload replay
# test, using vLLM's in-tree HF3FS KV connector (DeepSeek 3FS as the offload tier).
#
# Serves Qwen2.5-14B-Instruct behind vLLM's HF3FSKVConnector with a context window
# large enough to replay the 128K-context cc/mooncake traces
# (e.g. cc-traces-weka-*.mooncake-131k.jsonl). This is the "hf3fs" sibling of
# run-serve-cc131k.sh (Certus-SHMQ) and run-serve-cputier.sh (CPU+fs tiering):
# same model, endpoint, and engine flags, but the KV cache spills to a 3FS mount
# instead of a /dev/shm mailbox or a local fs tier. Pair with run-guidellm-cc131k.sh.
#
#   connector:  HF3FSKVConnector  (kv_role=kv_both — store AND load)
#   docs:       https://docs.vllm.ai/en/latest/api/vllm/distributed/kv_transfer/kv_connector/v1/hf3fs/hf3fs_connector/
#   3FS:        https://github.com/deepseek-ai/3fs
#
# ── PREREQUISITES (neither exists on this host by default — you must provide them) ──
#   1. A 3FS mount. The connector reads/writes KV cache as files under
#      HF3FS_STORAGE_PATH, which must be a live 3FS FUSE mount (a full 3FS cluster:
#      mgmtd + meta + storage services + a local FUSE client). Point HF3FS_MOUNT_HOST
#      at that host mountpoint; it is bind-mounted into the container at
#      HF3FS_STORAGE_PATH. A plain local directory "works" but is just local disk,
#      not 3FS — use run-serve-cputier.sh's fs tier for that.
#   2. A container image whose vLLM carries HF3FSKVConnector *and* the 3FS USRBIO
#      Python client. The stock vllm/vllm-openai image does NOT bundle the 3FS
#      client, and the v0.23 image predates the connector — set IMAGE to an image
#      you have built with both. There is no default that runs out of the box here.
#
# Why these model/engine values (inherited from run-serve-cc131k.sh):
#   MAX_MODEL_LEN=131072  the trace's inputs reach ~131K tokens (median ~88K); a
#                         smaller window rejects most rows with HTTP 400. Auto-enables
#                         Qwen YaRN rope-scaling (factor 131072/32768=4).
#   TENSOR_PARALLEL=2     a single 131K sequence's KV (~24G) plus the 14B weights
#                         (~29G) does NOT fit one 40G A100; sharding across both puts
#                         it at ~27G/GPU. Requires GPU=all so both devices are visible.
#   GPU_MEM_UTIL=0.9      leave headroom for activations/overhead per GPU.
#
#   ./run-server-h3fs0-cc131k.sh                                      # defaults (needs IMAGE + 3FS mount)
#   PORT=9000 ./run-server-h3fs0-cc131k.sh                            # publish on another port
#   IMAGE=localhost/my-vllm-hf3fs ./run-server-h3fs0-cc131k.sh        # image carrying the connector + 3FS client
#   HF3FS_MOUNT_HOST=/mnt/3fs ./run-server-h3fs0-cc131k.sh            # host 3FS mountpoint to offload into
#   MAX_MODEL_LEN=32768 ./run-server-h3fs0-cc131k.sh                  # native 32K window (no YaRN)
#   KV_CACHE_BYTES=4G ./run-server-h3fs0-cc131k.sh                    # cap GPU KV cache; spill reuse to 3FS
#
# Clients then use:
#   Base URL:  http://127.0.0.1:${PORT}/v1     (127.0.0.1 — podman publishes IPv4 only)
#   Model:     ${SERVED_MODEL_NAME}
#   /metrics:  http://127.0.0.1:${PORT}/metrics  (vLLM + HF3FS connector counters)
set -euo pipefail

# ── Endpoint ──────────────────────────────────────────────────────────────────
HOST="${HOST:-0.0.0.0}"
PORT="${PORT:-8000}"

# ── Model / engine (kept identical to run-serve-cc131k.sh / run-serve-cputier.sh
#    so the three offload backends are directly comparable over the same HTTP surface) ─
MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-14b}"   # the name clients pass as "model"
DTYPE="${DTYPE:-float16}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-131072}"
QWEN_NATIVE_CTX="${QWEN_NATIVE_CTX:-32768}"
GPU="${GPU:-all}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.90}"
# Shard the model across this many GPUs. TP=2 is REQUIRED for the 131K window on
# 40G A100s (see header). GPU=all only makes both devices visible — it does NOT
# enable sharding; that needs this flag.
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}"
# Directly cap the GPU-resident KV cache (per GPU). When set, vLLM IGNORES
# gpu-memory-utilization and pins the KV pool to this size — accepts human-readable
# sizes (4G, 512M). Shrinking it forces reused prefixes to spill from GPU HBM into
# the 3FS tier, which is what raises the external prefix cache hit rate. Unset
# (default) = derive KV size from GPU_MEM_UTIL (stock behavior).
KV_CACHE_BYTES="${KV_CACHE_BYTES:-}"

# ── HF3FS connector (vLLM HF3FSKVConnector) ──────────────────────────────────────
# All keys below map to kv_connector_extra_config entries the connector reads via
# get_from_extra_config; the values here are the connector's own documented defaults
# unless noted. See the docs link in the header.
HF3FS_MOUNT_HOST="${HF3FS_MOUNT_HOST:-/mnt/3fs}"                       # host 3FS FUSE mountpoint (bind-mounted in)
HF3FS_MOUNT_CTR="${HF3FS_MOUNT_CTR:-/vllm-workspace/mnt/hf3fs}"      # in-container path of the 3FS mount ROOT
# KV files go in a subdir: the mount root is root-owned, and USRBIO needs the ROOT
# bound in (it finds the mount via <root>/3fs-virt). deploy/3fs creates <root>/vllm.
HF3FS_STORAGE_PATH="${HF3FS_STORAGE_PATH:-${HF3FS_MOUNT_CTR}/vllm}"   # in-container path the connector uses
HF3FS_METADATA_SERVER_URL="${HF3FS_METADATA_SERVER_URL:-http://localhost:18000}"  # connector's mini metadata server
HF3FS_FILE_SIZE="${HF3FS_FILE_SIZE:-$((1024 * 1024 * 1024))}"         # per-file size (bytes), default 1 GiB
HF3FS_CLIENT_NUMJOBS="${HF3FS_CLIENT_NUMJOBS:-16}"                    # USRBIO client jobs (I/O parallelism)
HF3FS_MAX_DEVICE_BUFFER_COUNT="${HF3FS_MAX_DEVICE_BUFFER_COUNT:-128}" # device buffer pool (raised internally if too small)

# ── Container / store ────────────────────────────────────────────────────────────
# There is NO working default here: the image must carry vLLM's HF3FSKVConnector and
# the 3FS USRBIO Python client. Set IMAGE to an image you built with both.
IMAGE="${IMAGE:-localhost/vllm-hf3fs-bench:vllm0.30.0}"

# HF cache on the large filesystem — NOT $HOME/.cache (the /home partition is small
# and fills up mid-download).
HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"

# ── Preflight ──────────────────────────────────────────────────────────────────
if ! command podman image exists "$IMAGE"; then
  echo "error: image '$IMAGE' not found in the default podman store." >&2
  echo "       Set IMAGE to a vLLM image that bundles HF3FSKVConnector + the 3FS USRBIO client." >&2
  exit 1
fi
if [[ ! -d "$HF3FS_MOUNT_HOST" ]]; then
  echo "error: HF3FS_MOUNT_HOST='${HF3FS_MOUNT_HOST}' does not exist — is the 3FS FUSE client mounted?" >&2
  echo "       Bring up a 3FS cluster + FUSE mount first (see https://github.com/deepseek-ai/3fs)," >&2
  echo "       or point HF3FS_MOUNT_HOST at the mountpoint." >&2
  exit 1
fi
if ! mountpoint -q "$HF3FS_MOUNT_HOST" 2>/dev/null; then
  echo "warning: '${HF3FS_MOUNT_HOST}' is not a mountpoint — offloading to a plain local dir, NOT 3FS." >&2
fi

# ── KV-transfer config: HF3FSKVConnector ─────────────────────────────────────────
# JSON passed to `vllm serve --kv-transfer-config`. Keep it compact (no spaces) so
# the EXTRA_SERVE_ARGS word-split path below never has to reason about it.
KV_CONFIG=$(cat <<JSON
{"kv_connector":"HF3FSKVConnector","kv_role":"kv_both","kv_connector_extra_config":{"hf3fs_storage_path":"${HF3FS_STORAGE_PATH}","hf3fs_metadata_server_url":"${HF3FS_METADATA_SERVER_URL}","hf3fs_file_size":${HF3FS_FILE_SIZE},"hf3fs_client_numjobs":${HF3FS_CLIENT_NUMJOBS},"hf3fs_max_device_buffer_count":${HF3FS_MAX_DEVICE_BUFFER_COUNT}}}
JSON
)

# ── Assemble `vllm serve` args ──────────────────────────────────────────────────
# --no-async-scheduling: KV-transfer connectors serialize transfers per request;
#   async scheduling asserts/crashes with them (verified with the sibling connectors).
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

# Extra pass-through `vllm serve` flags for wrappers that need model-specific options
# this script doesn't model. Word-split, so keep any JSON value compact (no spaces).
# Appended last so it can also override earlier flags. Unset (default) = no extra args.
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

# YaRN rope-scaling: only when the requested window exceeds Qwen2.x's native 32768
# (Qwen ships no rope_scaling, so vLLM rejects a larger window otherwise). Qwen's
# official recipe is static YaRN with factor = target / native. Opt out with
# ROPE_YARN=0 (which caps you at the native window).
if [[ "${ROPE_YARN:-1}" != "0" && "$MAX_MODEL_LEN" -gt "$QWEN_NATIVE_CTX" && "${MODEL,,}" == *qwen2* ]]; then
  FACTOR=$(awk "BEGIN{printf \"%g\", ${MAX_MODEL_LEN}/${QWEN_NATIVE_CTX}}")
  HF_OVERRIDES=$(cat <<JSON
{"rope_scaling":{"rope_type":"yarn","factor":${FACTOR},"original_max_position_embeddings":${QWEN_NATIVE_CTX}}}
JSON
)
  SERVE_ARGS+=(--hf-overrides "$HF_OVERRIDES")
  echo "[serve] YaRN rope-scaling factor=${FACTOR} (native ${QWEN_NATIVE_CTX} -> max_model_len ${MAX_MODEL_LEN})"
fi

echo "[serve] ${IMAGE}: vllm serve ${MODEL} on ${HOST}:${PORT}"
echo "[serve] hf3fs: ${HF3FS_MOUNT_HOST} -> ${HF3FS_MOUNT_CTR}, KV in ${HF3FS_STORAGE_PATH} (meta=${HF3FS_METADATA_SERVER_URL}, numjobs=${HF3FS_CLIENT_NUMJOBS})"
echo "[serve] clients -> base_url http://127.0.0.1:${PORT}/v1   model ${SERVED_MODEL_NAME}"

# Bind the host 3FS mount into the container; -p publishes the API port.
# --pull=never (image is built locally).
#
# Metadata server: the connector's page/key index is a separate HTTP service
# (hf3fs_metadata_server.py) that vLLM does NOT start, and in vLLM 0.30 the
# connector constructs Hf3fsMetadataClient() with its built-in default URL
# (http://localhost:18000) — hf3fs_metadata_server_url is read but not passed on.
# So start it in this container on :18000 before `vllm serve`; it lives and dies
# with the container (a restart drops the index, matching a fresh KV file set).
# --ipc host: USRBIO passes I/O buffers to hf3fs_fuse_main via /dev/shm, so this
# container must share the host IPC namespace with deploy/3fs's cluster container.
# No :z on the 3FS bind — relabelling a FUSE mount fails and is not needed.
exec command podman run --rm --pull=never \
  --device "nvidia.com/gpu=${GPU}" \
  -p "${PORT}:${PORT}" \
  -e "HF_HUB_OFFLINE=0" \
  -v "${HF_CACHE}:/root/.cache/huggingface:z" \
  --ipc host \
  -v "${HF3FS_MOUNT_HOST}:${HF3FS_MOUNT_CTR}" \
  --entrypoint bash \
  "$IMAGE" \
  -c 'python3 -m vllm.distributed.kv_transfer.kv_connector.v1.hf3fs.hf3fs_metadata_server \
        --host 127.0.0.1 --port 18000 &
      for _ in $(seq 60); do python3 -c "import socket; socket.create_connection((\"127.0.0.1\", 18000), 1)" 2>/dev/null && break; sleep 1; done
      exec vllm serve "$@"' _ "${SERVE_ARGS[@]}"
