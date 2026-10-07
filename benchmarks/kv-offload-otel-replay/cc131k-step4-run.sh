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
#   cputier   -> CPU + fs disk tier
#
# Override via env:
#   ARM=shmq PROM=1 \
#   OTEL_HOST=/mnt/certus1/inference-perf-syn-data/otel_cc131k \
#   MODEL=Qwen/Qwen2.5-14B-Instruct NUM_CONVS=200 TIME_SCALE=0 \
#   TENSOR_PARALLEL_SIZE=2 \
#   SHM_PATH=/dev/shm/certus-shmq SLAB_SIZE_BYTES=2097152 \
#   CPU_BYTES=$((13*(1<<30))) DISK_DIR_HOST=/mnt/certus1/kv-fs-tier \
#   ./cc131k-step4-run.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ARM="${ARM:-shmq}"
PROM="${PROM:-1}"                        # 1 = -prom variant (Prometheus metrics)

# cc131k corpus + model defaults, exported so the arm scripts + common file see
# them. These must match what step 2 produced / step 4 serves.
export OTEL_HOST="${OTEL_HOST:-/mnt/certus1/inference-perf-syn-data/otel_cc131k}"
export MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct}"
export NUM_CONVS="${NUM_CONVS:-200}"
export TIME_SCALE="${TIME_SCALE:-0}"     # saturating by default (stress the tier)
export TENSOR_PARALLEL_SIZE="${TENSOR_PARALLEL_SIZE:-2}"  # cc131k = 14B on 2 GPUs

suffix=""; [[ "$PROM" == "1" ]] && suffix="-prom"

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
    export DISK_DIR_HOST="${DISK_DIR_HOST:-/mnt/certus1/kv-fs-tier}"
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
echo

exec "${SCRIPT_DIR}/${arm_script}"
