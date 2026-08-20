#!/bin/bash
# run-cputier-docker.sh — run the native Tiering CPU+FS KV backend from docker.
#
# vLLM 0.26 replacement for the (0.26-broken) SharedStorage backend. Uses the
# native multi-tier framework: OffloadingConnector -> TieringOffloadingSpec with
# a CPU (host-RAM) primary tier + an "fs" disk secondary tier.
#
# Self-contained: this script has NO dependency on run-docker-common.sh and does
# NOT bind-mount the host driver. The tiering config is baked into a dedicated
# image (certus-cputier-bench, from Dockerfile.cputier); the script builds that
# image if it is missing, then runs it. The only host mounts are the HF cache
# and the disk-tier data dir.
#
#   ./run-cputier-docker.sh
#   CPU_BYTES=$((8*(1<<30))) DISK_DIR_HOST=/mnt/kv-fs ./run-cputier-docker.sh
#   NO_BUILD=1 ./run-cputier-docker.sh          # fail instead of building if absent
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ── Image / build ─────────────────────────────────────────────────────────
IMAGE="${IMAGE:-certus-cputier-bench}"
VLLM_VERSION="${VLLM_VERSION:-0.26.0}"        # tiering.fs requires vLLM 0.26+
DOCKERFILE="${DOCKERFILE:-${SCRIPT_DIR}/Dockerfile.cputier}"

# ── Workload (mirrors the other kv-offload-replay backends) ─────────────────
MODEL="${MODEL:-NousResearch/Meta-Llama-3-8B}"
NUM_CONVS="${NUM_CONVS:-450}"
MAX_ROUNDS="${MAX_ROUNDS:-0}"                 # 0 = replay all 12 turns
OUTPUT_TOKENS="${OUTPUT_TOKENS:-150}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-8192}"
MAX_NUM_SEQS="${MAX_NUM_SEQS:-64}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.90}"
GPU="${GPU:-all}"

# ── Tiering config ──────────────────────────────────────────────────────────
CPU_BYTES="${CPU_BYTES:-$((32 * (1 << 30)))}"        # CPU (host-RAM) primary tier
DISK_DIR_HOST="${DISK_DIR_HOST:-/mnt/certus1/kv-fs-tier}"  # host-side fs disk tier
DISK_DIR_CTR="/workspace/kv-fs-tier"                 # baked mount point (see Dockerfile)
DISK_READ_THREADS="${DISK_READ_THREADS:-16}"
DISK_WRITE_THREADS="${DISK_WRITE_THREADS:-16}"
# TieringOffloadingSpec mmaps its CPU tier in /dev/shm and force-populates it,
# so /dev/shm must be >= CPU_BYTES (podman default is 64M). Size it with headroom.
SHM_BYTES="${SHM_BYTES:-$((CPU_BYTES + 4 * (1 << 30)))}"

# HF cache on the large filesystem — NOT $HOME/.cache (small /home partition).
HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"
LOG="${LOG:-${SCRIPT_DIR}/cputier_$(date +%H%M%S).log}"

# ── Ensure the dedicated image exists (build if missing) ────────────────────
if ! podman image exists "$IMAGE"; then
  if [[ "${NO_BUILD:-0}" == "1" ]]; then
    echo "error: image '$IMAGE' not found and NO_BUILD=1." >&2
    echo "       build it: podman build -f ${DOCKERFILE} --build-arg VLLM_VERSION=${VLLM_VERSION} -t ${IMAGE} ${REPO_ROOT}" >&2
    exit 1
  fi
  echo "[cputier] image '${IMAGE}' missing — building from ${DOCKERFILE} (VLLM_VERSION=${VLLM_VERSION})"
  podman build -f "${DOCKERFILE}" --build-arg "VLLM_VERSION=${VLLM_VERSION}" -t "${IMAGE}" "${REPO_ROOT}"
fi

mkdir -p "$DISK_DIR_HOST"

echo "[cputier] run ${IMAGE}: CPU_BYTES=${CPU_BYTES} DISK_DIR_HOST=${DISK_DIR_HOST} shm=${SHM_BYTES}  -> ${LOG}"
podman run --rm --pull=never \
  --device "nvidia.com/gpu=${GPU}" \
  --shm-size "${SHM_BYTES}" \
  -e "MODEL=${MODEL}" \
  -e "NUM_CONVS=${NUM_CONVS}" \
  -e "MAX_ROUNDS=${MAX_ROUNDS}" \
  -e "OUTPUT_TOKENS=${OUTPUT_TOKENS}" \
  -e "MAX_MODEL_LEN=${MAX_MODEL_LEN}" \
  -e "MAX_NUM_SEQS=${MAX_NUM_SEQS}" \
  -e "GPU_MEM_UTIL=${GPU_MEM_UTIL}" \
  -e "ENFORCE_EAGER=${ENFORCE_EAGER:-0}" \
  -e "CPU_BYTES=${CPU_BYTES}" \
  -e "DISK_DIR=${DISK_DIR_CTR}" \
  -e "DISK_READ_THREADS=${DISK_READ_THREADS}" \
  -e "DISK_WRITE_THREADS=${DISK_WRITE_THREADS}" \
  -e "HF_HUB_OFFLINE=0" \
  -v "${HF_CACHE}:/root/.cache/huggingface:z" \
  -v "${DISK_DIR_HOST}:${DISK_DIR_CTR}:z" \
  "$IMAGE" 2>&1 | tee "$LOG"
exit "${PIPESTATUS[0]}"
