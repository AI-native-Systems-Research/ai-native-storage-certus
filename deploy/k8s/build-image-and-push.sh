#!/usr/bin/env bash
#
# Build the certus container image and push to a registry.
#
# Usage:
#   deploy/k8s/build-image-and-push.sh [--no-push] [--tag <tag>]
#
# Required environment variables:
#   CERTUS_REGISTRY  — Container registry hostname (e.g. registry.example.com)
#   CERTUS_REPO      — Repository path within the registry (e.g. team-docker-local)
#
# Optional environment variables:
#   CERTUS_IMAGE     — Image name (default: certus)
#   CERTUS_TAG       — Image tag (default: latest); overridden by --tag flag
#
# Must be run from the top-level directory of the source repository.
#
# Requires: docker CLI (builds OCI images; containerd pulls them at runtime).
# Auth: uses ~/.docker/config.json credentials for the registry.
#
set -euo pipefail

# --- Verify CWD is repo root ---
if [ ! -d components ]; then
    echo "ERROR: Must be run from the top-level source directory." >&2
    echo "       (Expected to find 'components/' in current directory.)" >&2
    exit 1
fi

# --- Required environment variables ---
if [ -z "${CERTUS_REGISTRY:-}" ]; then
    echo "ERROR: CERTUS_REGISTRY is not set." >&2
    echo "       Set it to the container registry hostname." >&2
    exit 1
fi

if [ -z "${CERTUS_REPO:-}" ]; then
    echo "ERROR: CERTUS_REPO is not set." >&2
    echo "       Set it to the repository path within the registry." >&2
    exit 1
fi

# --- Optional environment variables with defaults ---
CERTUS_IMAGE="${CERTUS_IMAGE:-certus}"
CERTUS_TAG="${CERTUS_TAG:-latest}"

# --- Parse command-line arguments ---
NO_PUSH=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-push) NO_PUSH=true; shift ;;
        --tag)     CERTUS_TAG="$2"; shift 2 ;;
        *)         echo "Unknown arg: $1"; exit 1 ;;
    esac
done

FULL_IMAGE="${CERTUS_REGISTRY}/${CERTUS_REPO}/${CERTUS_IMAGE}:${CERTUS_TAG}"

# --- Build the container image ---
echo "Building image: ${FULL_IMAGE}"
docker build -f deploy/k8s/Dockerfile -t "${FULL_IMAGE}" .

# --- Generate k8s manifests from templates ---
echo "Generating k8s manifests..."
for tpl in deploy/k8s/*.yaml.tpl; do
    out="${tpl%.tpl}"
    sed -e "s|%%CERTUS_REGISTRY%%|${CERTUS_REGISTRY}|g" \
        -e "s|%%CERTUS_REPO%%|${CERTUS_REPO}|g" \
        -e "s|%%CERTUS_IMAGE%%|${CERTUS_IMAGE}|g" \
        -e "s|%%CERTUS_TAG%%|${CERTUS_TAG}|g" \
        "$tpl" > "$out"
    echo "  Generated: ${out}"
done

# --- Push ---
if [ "${NO_PUSH}" = false ]; then
    echo "Pushing image: ${FULL_IMAGE}"
    docker push "${FULL_IMAGE}"
    echo "Pushed successfully."
else
    echo "Skipping push (--no-push specified)."
fi

echo ""
echo "To pull with containerd/crictl:"
echo "  crictl pull ${FULL_IMAGE}"
echo ""
echo "To run as a k8s pod, use image: ${FULL_IMAGE}"
