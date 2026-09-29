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
#   IMAGE=localhost/certus-shmq-connector:vllm0.28.0 ./run-serve-certus-shmq.sh   # pin a vLLM version (see below)
#
# IMAGE selects the vLLM/connector build; VLLM_FIX3=1 (default) applies the
# scheduler crash fix in-place, self-adapting to whatever vLLM the image ships
# (VLLM_FIX3=0 runs the stock, crash-prone scheduler). See the Container / store
# and fix#3 sections below for building versioned images and the patch details.
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
# Any shmq/otel-shmq client image works — this script only needs vLLM + the
# certus_shmq_connector inside it, and drives it via `vllm serve`. The image lives
# in the /mnt/certus1 podman store, not the default store (mirrors
# run-docker-otel-shmq.sh / run-docker-certus-shmq.sh). Build one with either:
#   certus-shmq-connector/build_connector_container.sh 0.28.0   # -> localhost/certus-shmq-connector:vllm0.28.0
#   benchmarks/kv-offload-otel-replay/build-otel.sh             # -> localhost/certus-otel-shmq-connector:latest
# then point IMAGE at it. build_connector_container.sh <ver> selects the vLLM base
# version (FULL patch tag, e.g. 0.28.0 — not a bare 0.28); `--help` lists versions.
# Default is the otel image (unversioned :latest); set IMAGE for a versioned one:
#   IMAGE=localhost/certus-shmq-connector:vllm0.28.0 ./run-serve-certus-shmq.sh
IMAGE="${IMAGE:-localhost/certus-otel-shmq-connector}"
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
# a finishing request can cross an unbacked chunk boundary). The fix clamps
# num_chunks to the chunks that have both a key and backing GPU blocks.
#
# We apply it with an IN-PLACE patcher (patches/apply_fix3.py) rather than the old
# whole-file scheduler.py bind-mount: the mounted file was pinned to vLLM 0.26.0
# and would drop a stale scheduler onto a drifted API on any other base image
# (0.27+). The patcher instead locates the installed scheduler via importlib (no
# hardcoded pythonX.Y site-packages path) and edits only the clamp in, anchored on
# a block that is byte-stable across 0.26/0.27/0.28+, so it self-adapts to
# whatever vLLM the image ships. It is non-fatal by contract (WARNING + stock
# scheduler on any mismatch) and idempotent. This is the shmq-only fix — it does
# NOT carry fix#2's mark_stores_submitted handshake (CertusShmqOffloadingSpec's
# manager lacks it). Set VLLM_FIX3=0 to run the stock (crash-prone) scheduler.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VLLM_FIX3="${VLLM_FIX3:-1}"
FIX3_PATCHER="${FIX3_PATCHER:-${SCRIPT_DIR}/patches/apply_fix3.py}"
FIX3_PATCHER_IN_CONTAINER=/opt/certus/apply_fix3.py
FIX3_MOUNT=()
FIX3_ENABLED=0
if [[ "$VLLM_FIX3" == "1" ]]; then
  if [[ -f "$FIX3_PATCHER" ]]; then
    FIX3_MOUNT=(-v "${FIX3_PATCHER}:${FIX3_PATCHER_IN_CONTAINER}:ro,z")
    FIX3_ENABLED=1
    echo "[serve] fix#3: patching the in-container vLLM scheduler via ${FIX3_PATCHER##*/} before serving"
  else
    echo "warning: VLLM_FIX3=1 but patcher not found at ${FIX3_PATCHER}; running STOCK scheduler (crash-prone under load)" >&2
  fi
fi

# ── Preflight ──────────────────────────────────────────────────────────────────
if ! command podman "${STORE_FLAGS[@]}" image exists "$IMAGE"; then
  echo "error: image '$IMAGE' not found in store ${PODMAN_STORE}." >&2
  echo "       build one first (then set IMAGE= if it is not the default), e.g.:" >&2
  echo "         certus-shmq-connector/build_connector_container.sh 0.28.0   # versioned shmq image" >&2
  echo "         bash benchmarks/kv-offload-otel-replay/build-otel.sh        # default otel-shmq image" >&2
  echo "       (build_connector_container.sh --help lists supported vLLM versions)" >&2
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

# Entrypoint. With fix#3 enabled we override the image's replay-driver entrypoint
# with `bash -c`, run the in-place patcher, then exec the OpenAI API server. The
# patcher is non-fatal (see apply_fix3.py): on any failure it prints a WARNING and
# we still serve, just on the stock (crash-prone) scheduler. In `bash -c '…' A B…`
# $0 is A (the patcher path) and "$@" is B… (the serve args). With fix#3 off we
# override the entrypoint straight to `vllm` — no patch step.
if [[ "$FIX3_ENABLED" == "1" ]]; then
  RUN_ENTRY=(--entrypoint bash "$IMAGE" -c \
    'python3 "$0" || echo "[serve] WARNING: fix#3 patch failed; serving on STOCK scheduler (crash-prone under sustained store load)" >&2; exec vllm serve "$@"' \
    "$FIX3_PATCHER_IN_CONTAINER" "${SERVE_ARGS[@]}")
else
  RUN_ENTRY=(--entrypoint vllm "$IMAGE" serve "${SERVE_ARGS[@]}")
fi

# --ipc=host shares the mailbox + exposes CUDA IPC handles; -p publishes the API
# port. --pull=never against the alt store.
exec command podman "${STORE_FLAGS[@]}" run --rm --pull=never \
  --ipc=host \
  --device "nvidia.com/gpu=${GPU}" \
  -p "${PORT}:${PORT}" \
  -e "HF_HUB_OFFLINE=0" \
  -v "${HF_CACHE}:/root/.cache/huggingface:z" \
  "${FIX3_MOUNT[@]}" \
  "${RUN_ENTRY[@]}"
