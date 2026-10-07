#!/bin/bash
# cc131k-step4-run.sh — STEP 4: replay the cc131k OTel corpus (built in step 2)
# against one backend. Thin wrapper over the existing run-docker-otel-*.sh arms
# that pins OTEL_HOST / MODEL / NUM_CONVS to the cc131k corpus and forwards the
# arm-specific knobs. Build the images once first:  bash build-otel.sh
#
# Pick the arm with ARM=:
#   shmq      -> Certus-shmq  (REQUIRES step 3 server up on the same SHM_PATH)
#   offload   -> CPUOffload   (self-contained vLLM)
#   nooffload -> NoOffload baseline (CPUOffload image, OFFLOAD_MODE=none)
#   cputier   -> CPU + fs disk tier (fs dir = FS_TIER_HOST, default
#                /mnt/certus1/kv-fs-tier; CPU tier size = CPU_BYTES)
#
# Default = Option A: Qwen2.5-7B, TP=1, pinned to GPU 0 (single A100-40G).
# Override via env (this example switches to the 14B / two-GPU config):
#   ARM=shmq PROM=1 \
#   OTEL_HOST=/mnt/certus1/inference-perf-syn-data/otel_cc131k \
#   MODEL=Qwen/Qwen2.5-14B-Instruct NUM_CONVS=200 TIME_SCALE=0 \
#   GPU=all TENSOR_PARALLEL_SIZE=2 \
#   SHM_PATH=/dev/shm/certus-shmq SLAB_SIZE_BYTES=2097152 \
#   CPU_BYTES=$((13*(1<<30))) FS_TIER_HOST=/mnt/certus1/kv-fs-tier \
#   ./cc131k-step4-run.sh
#
# cputier on one GPU, custom fs-tier dir:
#   ARM=cputier FS_TIER_HOST=/mnt/certus1/kv-fs-tier ./cc131k-step4-run.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ARM="${ARM:-shmq}"
# Prometheus metrics are ON by default so Grafana can scrape the run: PROM=1
# selects the -prom arm script, which publishes PROM_PORT and makes vLLM +
# KV-offload metrics available at http://127.0.0.1:${PROM_PORT}/metrics.
# Set PROM=0 for the non-prom (no metrics) variant.
PROM="${PROM:-1}"

# cc131k corpus + model defaults, exported so the arm scripts + common file see
# them. These must match what step 2 produced / step 4 serves.
# Default = Option A: Qwen2.5-7B on a SINGLE A100-40G (TP=1, GPU 0). 7B weights
# (~14 GiB) leave ~22 GiB for KV, enough for a ~120k-token prompt on one card;
# the 14B (TP=2) config does NOT fit on one 40G GPU. 7B and 14B share the Qwen2.5
# tokenizer, so the corpus built in step 2 is valid for either.
export OTEL_HOST="${OTEL_HOST:-/mnt/certus1/inference-perf-syn-data/otel_cc131k}"
export MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
export NUM_CONVS="${NUM_CONVS:-200}"
export TIME_SCALE="${TIME_SCALE:-0}"     # saturating by default (stress the tier)
export TENSOR_PARALLEL_SIZE="${TENSOR_PARALLEL_SIZE:-1}"  # single GPU
export GPU="${GPU:-0}"                    # pin to one A100 (set GPU=all for both)

# EXTERNAL_HITS=1: shrink the GPU prefix cache so reused prefixes (especially a
# synthetic SHARED_PREFIX corpus, see step 1) spill off-GPU and get RELOADED from
# the offload tier -- which is what increments vllm_external_prefix_cache_hits_total.
# Pair with a SHARED_PREFIX corpus and, for an even stronger effect, TIME_SCALE=1.0.
if [[ "${EXTERNAL_HITS:-0}" == "1" ]]; then
  export GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.55}"
  echo "[step4] EXTERNAL_HITS=1 -> GPU_MEM_UTIL=${GPU_MEM_UTIL} (small GPU KV cache to force tier reloads)"
fi

suffix=""
if [[ "$PROM" == "1" ]]; then
  suffix="-prom"
  # Surface the scrape knobs here (the -prom arm scripts default them too) so the
  # Grafana target port is explicit and overridable from step 4.
  export PROM_PORT="${PROM_PORT:-8000}"
  export LOG_STATS="${LOG_STATS:-1}"
fi

case "$ARM" in
  shmq)
    export SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
    export SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"
    arm_script="run-docker-otel-shmq${suffix}.sh"
    ;;
  offload)
    arm_script="run-docker-otel-offload${suffix}.sh"
    ;;
  nooffload)
    export OFFLOAD_MODE="none"
    arm_script="run-docker-otel-offload${suffix}.sh"
    ;;
  cputier)
    export CPU_BYTES="${CPU_BYTES:-$((13*(1<<30)))}"
    # FS_TIER_HOST is the user-facing name for the host dir backing the fs disk
    # tier; falls back to DISK_DIR_HOST (what the arm script reads) then default.
    export DISK_DIR_HOST="${FS_TIER_HOST:-${DISK_DIR_HOST:-/mnt/certus1/kv-fs-tier}}"
    arm_script="run-docker-otel-cputier${suffix}.sh"
    ;;
  *)
    echo "error: unknown ARM='$ARM' (want: shmq | offload | nooffload | cputier)" >&2
    exit 2
    ;;
esac

if [[ ! -x "${SCRIPT_DIR}/${arm_script}" ]]; then
  echo "error: arm script not found: ${SCRIPT_DIR}/${arm_script}" >&2
  echo "       (PROM=${PROM}; set PROM=0 for the non-prom variant)" >&2
  exit 1
fi

echo "[step4] arm       : ${ARM}  ->  ${arm_script}"
echo "[step4] corpus    : ${OTEL_HOST}"
echo "[step4] model     : ${MODEL}  (TP=${TENSOR_PARALLEL_SIZE})"
echo "[step4] num convs : ${NUM_CONVS}  (TIME_SCALE=${TIME_SCALE})"
[[ "$ARM" == "shmq" ]] && echo "[step4] shm path  : ${SHM_PATH}  (step-3 server must be up here)"
if [[ "$PROM" == "1" ]]; then
  echo "[step4] metrics   : http://127.0.0.1:${PROM_PORT}/metrics  (Prometheus; point Grafana/Prometheus here)"
else
  echo "[step4] metrics   : OFF (PROM=0)"
fi
echo

exec "${SCRIPT_DIR}/${arm_script}"
