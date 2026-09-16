#!/bin/bash
# run-docker-common-otel.sh — shared config + helpers for the OTel-replay
# run-docker-otel-*.sh runners. This file is SOURCED, not executed.
#
# The OTel-corpus analogue of ../kv-offload-replay/run-docker-common.sh. Every
# value is overridable from the environment, e.g.:
#   NUM_CONVS=100 TIME_SCALE=0 ./run-docker-otel-offload.sh
#
# The runners launch a prebuilt image against the OTel corpus. They do NOT build
# — build the images first with build-otel.sh.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ── Workload (OTel corpus; Qwen tokenizer / 131072 window / 120k context) ─────
MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
# NUM_CONVS empty = replay ALL corpus files; set N for a subset (a smoke test).
NUM_CONVS="${NUM_CONVS:-}"
# TIME_SCALE=1.0 honors the recorded inter-turn timing (real, can run very long);
# 0 disables the waits (saturating); 0.1 replays 10x faster.
TIME_SCALE="${TIME_SCALE:-1.0}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-131072}"
CONTEXT_CAP="${CONTEXT_CAP:-120000}"
MAX_NUM_SEQS="${MAX_NUM_SEQS:-64}"
ACTIVE_SESSIONS="${ACTIVE_SESSIONS:-0}"      # 0 = open loop; N = closed loop
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.90}"
GPU="${GPU:-all}"

# HF cache on the large filesystem — NOT $HOME/.cache (the /home partition is
# small and fills up mid-download).
HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"

# Host OTel corpus dir, bind-mounted read-only into the container. This is the
# ~22 GB corpus that is NOT baked into the image; it MUST exist on the host.
OTEL_HOST="${OTEL_HOST:-/mnt/certus1/inference-perf-syn-data/otel_1k}"
OTEL_MNT="/workspace/otel-corpus"
if [[ ! -d "$OTEL_HOST" ]]; then
  echo "error: OTEL_HOST corpus dir not found: $OTEL_HOST" >&2
  echo "       set OTEL_HOST=/path/to/otel_files (per-conversation *.json)." >&2
  exit 1
fi

# Shared podman-run flags, built from the resolved env above. Optional envs
# (NUM_CONVS) are only forwarded when set so the image default (ALL files) stands.
COMMON_RUN_ARGS=(
  --device "nvidia.com/gpu=${GPU}"
  -e "MODEL=${MODEL}"
  -e "TIME_SCALE=${TIME_SCALE}"
  -e "MAX_MODEL_LEN=${MAX_MODEL_LEN}"
  -e "CONTEXT_CAP=${CONTEXT_CAP}"
  -e "MAX_NUM_SEQS=${MAX_NUM_SEQS}"
  -e "ACTIVE_SESSIONS=${ACTIVE_SESSIONS}"
  -e "GPU_MEM_UTIL=${GPU_MEM_UTIL}"
  # TENSOR_PARALLEL_SIZE spans each vLLM engine across N GPUs (TP). Podman does
  # not inherit host env, so this -e is the only path that delivers it.
  -e "TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-1}"
  # enforce_eager MUST be identical across every backend or the comparison is a
  # confound (eager disables CUDA graphs + torch.compile).
  -e "ENFORCE_EAGER=${ENFORCE_EAGER:-0}"
  # Data-parallel shard identity for this replica (disjoint 1/N corpus slice).
  -e "DP_RANK=${DP_RANK:-0}"
  -e "DP_SIZE=${DP_SIZE:-1}"
  -e "HF_HUB_OFFLINE=0"
  -v "${HF_CACHE}:/root/.cache/huggingface:z"
  # The corpus bind mount (read-only) + OTEL_DIR pointing at it.
  -v "${OTEL_HOST}:${OTEL_MNT}:ro"
  -e "OTEL_DIR=${OTEL_MNT}"
)
if [[ -n "$NUM_CONVS" ]]; then
  COMMON_RUN_ARGS+=(-e "NUM_CONVS=${NUM_CONVS}")
fi

# stamp — a HHMMSS suffix for default log names.
stamp() { date +%H%M%S; }

# require_image <image> [podman-store-flags...] — the runners do not build, so
# fail early with the build command if the prebuilt image is absent.
require_image() {
  local img="$1"; shift
  if ! command podman "$@" image exists "$img"; then
    echo "error: image '$img' not found${*:+ (store flags: $*)}." >&2
    echo "       These run scripts do not build. Build the images first:" >&2
    echo "         bash ${SCRIPT_DIR}/build-otel.sh" >&2
    exit 1
  fi
}

# run_container <logfile> <image> [extra podman-run args...] — run the image with
# the common flags, tee to a log, and return the container's exit code (not tee's).
run_container() {
  local log="$1" img="$2"; shift 2
  echo "[run] ${img}  ->  ${log}"
  command podman run --rm --pull=never "${COMMON_RUN_ARGS[@]}" "$@" "$img" 2>&1 | tee "$log"
  return "${PIPESTATUS[0]}"
}
