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
# Immutable by default. Two deployments testing different certus builds push to
# the same repo, so a shared mutable tag like `latest` lets one overwrite the
# other and lets a node pull the wrong build. Override for a one-off.
CERTUS_TAG="${CERTUS_TAG:-$(git rev-parse --short HEAD 2>/dev/null || echo latest)}"
# Cargo build selection, passed through to the Dockerfile. The defaults build the
# full-remote profile (remote-lookup over zyre + hardware RDMA), matching
# scripts/build-certus-full-remote-spdk.sh. Overriding CERTUS_FEATURES with just
# "spdk" reverts to a server that accepts --rl-group but cannot remote-look-up.
CERTUS_PROFILE="${CERTUS_PROFILE:-full-remote}"
CERTUS_FEATURES="${CERTUS_FEATURES:-spdk,rdma,remote-lookup-rdma-initiator/rdma,remote-lookup-rdma-responder/rdma}"

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
echo "  profile:  ${CERTUS_PROFILE}"
echo "  features: ${CERTUS_FEATURES}"
docker build -f deploy/k8s/Dockerfile \
    --build-arg "CERTUS_PROFILE=${CERTUS_PROFILE}" \
    --build-arg "CERTUS_FEATURES=${CERTUS_FEATURES}" \
    -t "${FULL_IMAGE}" .

# --- Generate k8s manifests from templates ---
# Delegated so the same substitution serves both callers, and so a deployment
# knob can be re-stamped without coming back through an image build.
# Deployment knobs (CERTUS_NAMESPACE, CERTUS_NODE_SELECTOR_*, CERTUS_RL_GROUP)
# are owned by render-manifests.sh and inherited from the environment -- they are
# deliberately not redefined here, so there is one place that defines each.
export CERTUS_REGISTRY CERTUS_REPO CERTUS_IMAGE CERTUS_TAG
deploy/k8s/render-manifests.sh

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
