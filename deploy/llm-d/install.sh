#!/usr/bin/env bash
#
# Bring llm-d up in the llm-d-quickstart namespace. The a30 overlay is sized for
# 3 NVIDIA A30s spread over two nodes (2 + 1); adjust `replicas` for yours.
#
# This repo holds the SOURCES; the llm-d checkout is a disposable build area
# pinned to llm-d.ref. The overlays under a30/ and a30-certus/ are copied into it
# because they consume llm-d's `base` (and each other) by relative path, which
# kustomize will not resolve outside its own root.
#
# Previously this file lived at ~/llm-d-install.sh and had to be sourced. It is
# now a normal script -- `cd` no longer leaks into your shell, and `set -euo
# pipefail` stops the run on a failed step instead of carrying on.
#
# Usage:
#   deploy/llm-d/install.sh                 # plain A30 deployment
#   WITH_CERTUS=1 deploy/llm-d/install.sh   # + certus KV-cache offload
#
# Env:
#   LLMD_DIR     llm-d checkout (default: ~/llm-d; cloned if absent)
#   LLMD_REF     override the pin in llm-d.ref
#   NAMESPACE    target namespace (default: llm-d-quickstart)
#   WITH_CERTUS  1 to apply the certus offload overlay (default: 0)
#
# Deliberately absent: the llm-d-hf-token secret. Qwen/Qwen3-8B is ungated and
# a30/patch-a30.yaml deletes the HF_TOKEN env entry, so no HuggingFace token is
# needed. vLLM warns about "unauthenticated requests to the HF Hub"; that costs
# download rate limit, not function.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

LLMD_DIR="${LLMD_DIR:-${HOME}/llm-d}"
NAMESPACE="${NAMESPACE:-llm-d-quickstart}"
WITH_CERTUS="${WITH_CERTUS:-0}"
GUIDE_NAME="${GUIDE_NAME:-quickstart}"

if [ -z "${LLMD_REF:-}" ]; then
    LLMD_REF="$(grep -vE '^[[:space:]]*(#|$)' "${SCRIPT_DIR}/llm-d.ref" \
                | head -1 | tr -d '[:space:]')"
fi
[ -n "${LLMD_REF}" ] || { echo "ERROR: no llm-d revision in ${SCRIPT_DIR}/llm-d.ref" >&2; exit 1; }

# --- llm-d checkout, pinned ---
[ -d "${LLMD_DIR}/.git" ] || git clone https://github.com/llm-d/llm-d.git "${LLMD_DIR}"
git -C "${LLMD_DIR}" fetch --quiet origin
# Detached on purpose: this checkout is build output, not somewhere to commit.
# Fetch all refs rather than the bare sha -- servers need not allow sha fetches.
if ! git -C "${LLMD_DIR}" checkout --quiet --detach "${LLMD_REF}"; then
    echo "ERROR: cannot check out llm-d ${LLMD_REF} in ${LLMD_DIR}." >&2
    echo "       Local modifications to tracked files will block this; stash them." >&2
    exit 1
fi
echo "[install] llm-d pinned at $(git -C "${LLMD_DIR}" rev-parse --short HEAD)"

cd "${LLMD_DIR}"
export REPO_ROOT="$(realpath "$(git rev-parse --show-toplevel)")"
# shellcheck disable=SC1091
source "${REPO_ROOT}/guides/env.sh"
export NAMESPACE GUIDE_NAME

VLLM_DIR="${REPO_ROOT}/guides/optimized-baseline/modelserver/gpu/vllm"
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

# --- Materialize the overlays into the pinned checkout. ---
# rm -rf first so nothing from an earlier layout survives (these directories once
# held copies of base/* transformed by ex scripts). Never hand-edit them: change
# the sources in this repo instead.
install_overlay() {           # install_overlay <name> <files...>
    local name="$1"; shift
    rm -rf "${VLLM_DIR}/${name}"
    mkdir -p "${VLLM_DIR}/${name}"
    local f
    for f in "$@"; do cp "${SCRIPT_DIR}/${name}/${f}" "${VLLM_DIR}/${name}/"; done
}

install_overlay a30 kustomization.yaml patch-a30.yaml
OVERLAY="${VLLM_DIR}/a30"

if [ "${WITH_CERTUS}" != "0" ]; then
    # Consumes ../a30, so it has to sit beside it. Only apply-time files are
    # copied; Dockerfile and build-image-and-push.sh stay in this repo.
    install_overlay a30-certus kustomization.yaml patch-certus.yaml entrypoint-certus.sh
    OVERLAY="${VLLM_DIR}/a30-certus"

    # The committed overlay pins a REGISTRY/REPO placeholder rather than a real
    # registry, so resolve it here against the variables publish-image.sh pushed
    # with. Done on the materialized copy, so the sources stay site-neutral.
    certus_km="${OVERLAY}/kustomization.yaml"
    if grep -q 'REGISTRY/REPO/' "${certus_km}"; then
        if [ -z "${CERTUS_REGISTRY:-}" ] || [ -z "${CERTUS_REPO:-}" ]; then
            echo "ERROR: a30-certus/kustomization.yaml pins the REGISTRY/REPO placeholder," >&2
            echo "       so there is no image to pull. Export CERTUS_REGISTRY and" >&2
            echo "       CERTUS_REPO (the values publish-image.sh pushed with), e.g." >&2
            echo "         export CERTUS_REGISTRY=registry.example.com" >&2
            echo "         export CERTUS_REPO=my-team-docker-local" >&2
            exit 1
        fi
        sed -i "s|REGISTRY/REPO/|${CERTUS_REGISTRY}/${CERTUS_REPO}/|" "${certus_km}"
        echo "[install] image registry -> ${CERTUS_REGISTRY}/${CERTUS_REPO}"
    fi
    echo "[install] certus KV-cache offload ENABLED"
else
    echo "[install] plain A30 deployment (set WITH_CERTUS=1 for certus offload)"
fi

# Fail here rather than letting a malformed overlay reach the cluster. This is
# also where an upstream base restructuring shows up after a llm-d.ref bump.
kubectl kustomize "${OVERLAY}" >/dev/null

kubectl apply -k "${OVERLAY}" -n "${NAMESPACE}"

# First start pulls ~16Gi of weights into the shared /home/llm-d-cache hostPath
# and compiles CUDA graphs; the startup probe allows 60 min (120 x 30s). Match
# it, or rollout status reports a false failure.
kubectl rollout status "${DECODE_DEPLOY}" -n "${NAMESPACE}" --timeout=60m
