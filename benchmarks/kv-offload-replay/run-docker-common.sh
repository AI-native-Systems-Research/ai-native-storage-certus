#!/bin/bash
# run-docker-common.sh — shared config + helpers for the per-solution
# run-docker-*.sh runners. This file is SOURCED, not executed.
#
# Every value is overridable from the environment, e.g.:
#   NUM_CONVS=100 MODEL=meta-llama/Llama-3.1-8B ./run-docker-none.sh
#
# The four runners each launch ONE KV-offload solution against the same replay
# workload, using a prebuilt image. They do NOT build — build the images first
# with build_026.sh (which pins --build-arg VLLM_VERSION).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ── Workload (identical across all four solutions; mirrors profile_all.sh) ──
MODEL="${MODEL:-NousResearch/Meta-Llama-3-8B}"
NUM_CONVS="${NUM_CONVS:-450}"
MAX_ROUNDS="${MAX_ROUNDS:-0}"          # 0 = replay all 12 turns; N caps rounds
OUTPUT_TOKENS="${OUTPUT_TOKENS:-150}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-8192}"
MAX_NUM_SEQS="${MAX_NUM_SEQS:-64}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.90}"
GPU="${GPU:-all}"

# HF cache on the large filesystem — NOT $HOME/.cache (the /home partition is
# small and fills up mid-download).
HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"

# Shared podman-run flags for the self-contained images (the unified
# certus-offload-bench, which covers nooffload/cpu/tiered via OFFLOAD_MODE, plus
# sharedstorage). Built from the resolved env above.
COMMON_RUN_ARGS=(
  --device "nvidia.com/gpu=${GPU}"
  -e "MODEL=${MODEL}"
  -e "NUM_CONVS=${NUM_CONVS}"
  -e "MAX_ROUNDS=${MAX_ROUNDS}"
  -e "OUTPUT_TOKENS=${OUTPUT_TOKENS}"
  -e "MAX_MODEL_LEN=${MAX_MODEL_LEN}"
  -e "MAX_NUM_SEQS=${MAX_NUM_SEQS}"
  -e "GPU_MEM_UTIL=${GPU_MEM_UTIL}"
  # enforce_eager MUST be identical across every backend or the comparison is a
  # confound (eager disables CUDA graphs + torch.compile). All drivers read this
  # env with the same "0" default; forward it here so it applies uniformly to
  # every containerized backend (podman does NOT inherit host env — only -e vars
  # reach the container).
  -e "ENFORCE_EAGER=${ENFORCE_EAGER:-0}"
  # WORKLOAD_MODE selects the execution model (batched rounds vs async
  # per-conversation); ACTIVE_SESSIONS>0 makes the async model a closed loop
  # holding that many conversations active (admit-on-finish). Defaults preserve
  # the historical batched behavior; podman does not inherit host env, so both
  # must be forwarded explicitly here to reach the containerized driver.
  -e "WORKLOAD_MODE=${WORKLOAD_MODE:-batched}"
  -e "ACTIVE_SESSIONS=${ACTIVE_SESSIONS:-0}"
  -e "HF_HUB_OFFLINE=0"
  -v "${HF_CACHE}:/root/.cache/huggingface:z"
)

# Optional dataset override: mount a host dataset json read-only and point
# DATASET_PATH at it, instead of the image's baked sharegpt_12turn_450.json.
# Lets a run use a different/larger conversation set (e.g. 10000 convs from
# sharegpt_v3.json) without rebuilding the image. Default (unset) = baked file.
DATASET_HOST="${DATASET_HOST:-}"
if [[ -n "$DATASET_HOST" ]]; then
  [[ -f "$DATASET_HOST" ]] || { echo "error: DATASET_HOST not found: $DATASET_HOST" >&2; exit 1; }
  COMMON_RUN_ARGS+=(
    -v "${DATASET_HOST}:/workspace/data/replay-dataset.json:ro"
    -e "DATASET_PATH=/workspace/data/replay-dataset.json"
  )
fi

# stamp — a HHMMSS suffix for default log names (date is fine in a real shell).
stamp() { date +%H%M%S; }

# require_image <image> [podman-store-flags...] — the runners do not build, so
# fail early with the build command if the prebuilt image is absent.
require_image() {
  local img="$1"; shift
  if ! command podman "$@" image exists "$img"; then
    echo "error: image '$img' not found${*:+ (store flags: $*)}." >&2
    echo "       These run scripts do not build. Build the images first:" >&2
    echo "         bash ${SCRIPT_DIR}/build_026.sh" >&2
    exit 1
  fi
}

# run_container <logfile> <image> [extra podman-run args...] — run one of the
# three self-contained images with the common flags, tee to a log, and return
# the container's exit code (not tee's).
run_container() {
  local log="$1" img="$2"; shift 2
  echo "[run] ${img}  ->  ${log}"
  command podman run --rm --pull=never "${COMMON_RUN_ARGS[@]}" "$@" "$img" 2>&1 | tee "$log"
  return "${PIPESTATUS[0]}"
}
