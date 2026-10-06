#!/usr/bin/env bash
#
# Bring llm-d up in the llm-d-quickstart namespace. The a30 overlay is sized for
# 3 NVIDIA A30s spread over two nodes (2 + 1); adjust `replicas` for yours.
#
# This repo holds the SOURCES. The llm-d checkout is treated as a disposable
# build area: the A30 overlay is regenerated from llm-d's own `base` on every
# run, so a stale hand-edit there can never survive. Re-running is safe.
#
# Previously this file lived at ~/llm-d-install.sh and had to be sourced. It is
# now a normal script -- `cd` no longer leaks into your shell, and it can use
# `set -euo pipefail` so a failed step stops the run instead of carrying on.
#
# Usage:
#   deploy/llm-d/install.sh                 # plain A30 deployment
#   WITH_CERTUS=1 deploy/llm-d/install.sh   # + certus KV-cache offload
#
# Env:
#   LLMD_DIR     llm-d checkout (default: ~/llm-d; cloned if absent)
#   NAMESPACE    target namespace (default: llm-d-quickstart)
#   WITH_CERTUS  1 to apply the certus offload overlay (default: 0)
#
# Deliberately absent: the llm-d-hf-token secret. Qwen/Qwen3-8B is ungated and
# a30/edit-patch.ex comments the HF_TOKEN env block out of the modelserver, so
# no HuggingFace token is needed. vLLM warns about "unauthenticated requests to
# the HF Hub"; that costs download rate limit, not function.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

LLMD_DIR="${LLMD_DIR:-${HOME}/llm-d}"
NAMESPACE="${NAMESPACE:-llm-d-quickstart}"
WITH_CERTUS="${WITH_CERTUS:-0}"
GUIDE_NAME="${GUIDE_NAME:-quickstart}"

[ -d "${LLMD_DIR}/.git" ] || git clone https://github.com/llm-d/llm-d.git "${LLMD_DIR}"
cd "${LLMD_DIR}"

export REPO_ROOT="$(realpath "$(git rev-parse --show-toplevel)")"
# shellcheck disable=SC1091
source "${REPO_ROOT}/guides/env.sh"
export NAMESPACE GUIDE_NAME

VLLM_DIR="${REPO_ROOT}/guides/optimized-baseline/modelserver/gpu/vllm"
BASE_OVERLAY="${VLLM_DIR}/base"
A30_OVERLAY="${VLLM_DIR}/a30"
A30_CERTUS_OVERLAY="${VLLM_DIR}/a30-certus"
DECODE_DEPLOY="deploy/optimized-baseline-nvidia-gpu-vllm-decode"

# --- Gateway API Inference Extension CRDs ---
kubectl apply -f \
  "https://github.com/kubernetes-sigs/gateway-api-inference-extension/${GAIE_URL}/v1-manifests.yaml"

kubectl create namespace "${NAMESPACE}" --dry-run=client -o yaml | kubectl apply -f -

# --- Router / EPP. `upgrade --install` so a re-run updates in place instead of
# failing with "cannot re-use a name that is still in use". ---
helm upgrade --install "${GUIDE_NAME}" \
  "${ROUTER_STANDALONE_CHART}" \
  -f "${REPO_ROOT}/guides/recipes/router/base.values.yaml" \
  -f "${REPO_ROOT}/guides/optimized-baseline/router/optimized-baseline.values.yaml" \
  -n "${NAMESPACE}" \
  --version "${ROUTER_CHART_VERSION}"

# --- Regenerate the A30 overlay from llm-d's base, then replay our retuning. ---
# The cp overwrites wholesale: never hand-edit ${A30_OVERLAY}, change
# deploy/llm-d/a30/edit-*.ex in THIS repo instead. Copy-then-edit also keeps the
# ex scripts idempotent -- their append commands would otherwise duplicate the
# --gpu-memory-utilization / --max-model-len args on a second pass.
mkdir -p "${A30_OVERLAY}"
cp "${BASE_OVERLAY}"/* "${A30_OVERLAY}/"
ex - "${A30_OVERLAY}/patch-vllm.yaml"    < "${SCRIPT_DIR}/a30/edit-patch.ex"
ex - "${A30_OVERLAY}/kustomization.yaml" < "${SCRIPT_DIR}/a30/edit-kustomization.ex"

OVERLAY="${A30_OVERLAY}"

if [ "${WITH_CERTUS}" != "0" ]; then
    # The certus overlay consumes ../a30 as a kustomize resource, so it has to
    # sit beside the generated overlay inside the llm-d checkout. Only the
    # apply-time files are copied; Dockerfile and build-image-and-push.sh stay in
    # this repo because they are build-time, not apply-time.
    mkdir -p "${A30_CERTUS_OVERLAY}"
    cp "${SCRIPT_DIR}/a30-certus/kustomization.yaml" \
       "${SCRIPT_DIR}/a30-certus/patch-certus.yaml" \
       "${SCRIPT_DIR}/a30-certus/entrypoint-certus.sh" \
       "${A30_CERTUS_OVERLAY}/"
    OVERLAY="${A30_CERTUS_OVERLAY}"
    echo "[install] certus KV-cache offload ENABLED -> ${OVERLAY}"
else
    echo "[install] plain A30 deployment (set WITH_CERTUS=1 for certus offload)"
fi

# Fail here rather than letting a malformed overlay reach the cluster.
kubectl kustomize "${OVERLAY}" >/dev/null

kubectl apply -k "${OVERLAY}" -n "${NAMESPACE}"

# First start pulls ~16Gi of weights into the shared /home/llm-d-cache hostPath
# and compiles CUDA graphs; the startup probe allows 60 min (120 x 30s). Match
# it, or rollout status reports a false failure.
kubectl rollout status "${DECODE_DEPLOY}" -n "${NAMESPACE}" --timeout=60m
