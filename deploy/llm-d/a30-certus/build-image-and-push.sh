#!/usr/bin/env bash
#
# Build the vLLM + certus-connector image used by llm-d's modelserver pods, and
# push it to the registry.
#
# Mirrors deploy/k8s/build-image-and-push.sh and reuses its environment
# variables, so one set of exports drives both. That script builds certus-server;
# this one builds the vLLM client image.
#
# Usage:
#   deploy/llm-d/a30-certus/build-image-and-push.sh [--no-push] [--tag <tag>]
#
# Required:
#   CERTUS_REGISTRY  — registry hostname
#   CERTUS_REPO      — repository path within the registry
# Optional:
#   CERTUS_IMAGE     — image name (default: llm-d-vllm-certus)
#   CERTUS_TAG       — tag (default: <vllm version>-<repo short sha>, immutable)
#   VLLM_VERSION     — base vLLM image tag (default: v0.30.0)
#
# Auth: ~/.docker/config.json, same as the certus-server build.
set -euo pipefail

: "${CERTUS_REGISTRY:?set CERTUS_REGISTRY (registry hostname)}"
: "${CERTUS_REPO:?set CERTUS_REPO (repository path within the registry)}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

CERTUS_IMAGE="${CERTUS_IMAGE:-llm-d-vllm-certus}"
VLLM_VERSION="${VLLM_VERSION:-v0.30.0}"

if [ ! -d "${REPO_ROOT}/certus-shmq-connector" ]; then
    echo "ERROR: ${REPO_ROOT}/certus-shmq-connector not found." >&2
    echo "       Expected to be running from inside the certus repo." >&2
    exit 1
fi

# Default to an immutable tag so the overlay's imagePullPolicy: IfNotPresent
# stays correct -- a moving tag would leave nodes serving a stale cached layer.
if [ -z "${CERTUS_TAG:-}" ]; then
    _sha="$(cd "${REPO_ROOT}" && git rev-parse --short HEAD 2>/dev/null || echo nogit)"
    CERTUS_TAG="${VLLM_VERSION}-${_sha}"
fi

NO_PUSH=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-push) NO_PUSH=true; shift ;;
        --tag)     CERTUS_TAG="$2"; shift 2 ;;
        *)         echo "Unknown arg: $1" >&2; exit 1 ;;
    esac
done

FULL_IMAGE="${CERTUS_REGISTRY}/${CERTUS_REPO}/${CERTUS_IMAGE}:${CERTUS_TAG}"

echo "Building image: ${FULL_IMAGE}"
echo "  context:      ${REPO_ROOT}"
echo "  vllm base:    ${VLLM_VERSION}"
docker build \
    -f "${SCRIPT_DIR}/Dockerfile" \
    --build-arg "VLLM_VERSION=${VLLM_VERSION}" \
    -t "${FULL_IMAGE}" \
    "${REPO_ROOT}"

if [ "${NO_PUSH}" = false ]; then
    echo "Pushing image: ${FULL_IMAGE}"
    docker push "${FULL_IMAGE}"
    echo "Pushed successfully."
else
    echo "Skipping push (--no-push specified)."
fi

echo ""
echo "Pin the overlay to this build:"
echo "  (cd ${SCRIPT_DIR} && kustomize edit set image \\"
echo "     docker.io/vllm/vllm-openai=${FULL_IMAGE})"
