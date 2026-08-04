#!/bin/bash
# build-sharedstorage.sh — two-step build of the SharedStorage backend image.
#
# The llmd_fs_backend connector is a COMPILED torch C++ extension living in a
# separate repo. Rather than reimplement its build, this script reuses that
# package's OWN Dockerfile.wheel to produce the wheel — but overrides its torch/
# CUDA/arch build args to MATCH this repo's runtime base (vllm/vllm-openai:
# v0.20.0 → torch 2.11.0/cu130) and the target GPU, so the extension ABI matches
# at run time. Then it builds Dockerfile.sharedstorage, which installs the wheel.
#
# Defaults target this host (A30 = sm_80, torch 2.11.0/cu130). Override via env.
set -euo pipefail

_here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${_here}/../.." && pwd)"

ENGINE="${ENGINE:-podman}"
FS_BACKEND_DIR="${FS_BACKEND_DIR:-$HOME/llm-d-kv-cache/kv_connectors/llmd_fs_backend}"

# vLLM base-image version for the runtime image (step 2). Default 0.20.0. If you
# bump this, also update the torch args below to match that base's torch/CUDA, or
# the compiled wheel's ABI will not match at run time.
VLLM_VERSION="${VLLM_VERSION:-0.20.0}"

# Match the runtime base image's torch. Check with:
#   <venv>/bin/python -c "import torch;print(torch.__version__, torch.version.cuda)"
TORCH_VERSION="${TORCH_VERSION:-2.11.0}"
TORCH_CUDA_INDEX="${TORCH_CUDA_INDEX:-cu130}"
CUDA_BASE_TAG="${CUDA_BASE_TAG:-13.0.0-devel-ubuntu22.04}"
# Target GPU compute capability (A30 = 8.0). Check: nvidia-smi --query-gpu=compute_cap --format=csv,noheader
TORCH_CUDA_ARCH_LIST="${TORCH_CUDA_ARCH_LIST:-8.0}"

WHEEL_IMG="${WHEEL_IMG:-llmd-fs-backend-wheel:local}"
RUNTIME_IMG="${RUNTIME_IMG:-certus-sharedstorage-bench}"
WHEELS_DIR="${_here}/wheels"

echo "[build] fs-backend wheel: torch=${TORCH_VERSION} ${TORCH_CUDA_INDEX} cuda-base=${CUDA_BASE_TAG} arch=${TORCH_CUDA_ARCH_LIST}"
[ -f "${FS_BACKEND_DIR}/Dockerfile.wheel" ] || { echo "[build] ERROR: ${FS_BACKEND_DIR}/Dockerfile.wheel not found (set FS_BACKEND_DIR)"; exit 1; }

# ── Step 1: build the wheel with its own Dockerfile.wheel, matched args ───────
"${ENGINE}" build -f "${FS_BACKEND_DIR}/Dockerfile.wheel" \
    --build-arg "TORCH_VERSION=${TORCH_VERSION}" \
    --build-arg "TORCH_CUDA_INDEX=${TORCH_CUDA_INDEX}" \
    --build-arg "CUDA_BASE_TAG=${CUDA_BASE_TAG}" \
    --build-arg "torch_cuda_arch_list=${TORCH_CUDA_ARCH_LIST}" \
    -t "${WHEEL_IMG}" "${FS_BACKEND_DIR}"

# ── Extract the built wheel into ./wheels/ (build context for step 2) ─────────
mkdir -p "${WHEELS_DIR}"
rm -f "${WHEELS_DIR}"/*.whl
cid="$("${ENGINE}" create "${WHEEL_IMG}")"
# The wheel lands in /workspace/dist (auditwheel-repaired) in Dockerfile.wheel.
"${ENGINE}" cp "${cid}:/workspace/dist/." "${WHEELS_DIR}/"
"${ENGINE}" rm "${cid}" >/dev/null
echo "[build] wheel(s) extracted:"; ls -1 "${WHEELS_DIR}"/*.whl

# ── Step 2: build the runtime image (context = repo root) ─────────────────────
"${ENGINE}" build -f "${_here}/Dockerfile.sharedstorage" \
    --build-arg "VLLM_VERSION=${VLLM_VERSION}" \
    -t "${RUNTIME_IMG}" "${repo_root}"
echo "[build] done: ${RUNTIME_IMG}"
