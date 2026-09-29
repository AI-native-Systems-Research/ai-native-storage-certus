#!/bin/bash
# build_connector_container.sh — build the certus-shmq client image on a given
# vLLM base version.
#
# The image bundles vLLM + the pure-Python certus-shmq connector + the workload
# driver (client side only; the certus-server runs on the host). The vLLM
# version is a BUILD ARG — it selects the `vllm/vllm-openai:v<X>` base image.
# Nothing in the connector keys off it: certus_shmq_connector.compat detects the
# installed vLLM at run time and self-adapts (matrix: 0.20, 0.22, 0.23, 0.24,
# 0.26, 0.29, 0.30). So building a version = FROM a different base, no code change.
#
# The image is built into the /mnt/certus1 podman store by default, which is
# where run-docker-certus-shmq.sh / run-bench.sh look for it. Pass the resulting
# IMAGE to the run script:
#
#   ./build_connector_container.sh 0.29.0
#   IMAGE=$(./build_connector_container.sh --print-image 0.29.0) \
#     ../benchmarks/kv-offload-replay/run-docker-certus-shmq.sh   # (see run script for env)
#
# Usage:
#   ./build_connector_container.sh <vllm-version>            # e.g. 0.29.0
#   VLLM_VERSION=0.30.0 ./build_connector_container.sh        # version via env
#   TAG=vllm0.29-test ./build_connector_container.sh 0.29.0   # override the :tag
#   IMAGE=localhost/my-bench:x ./build_connector_container.sh 0.29.0  # override full ref
#   PODMAN_STORE=/var/lib/containers/storage PODMAN_RUNROOT=/run/containers \
#     ./build_connector_container.sh 0.29.0                   # use a different store
#   NO_CACHE=1 ./build_connector_container.sh 0.29.0          # --no-cache
#   ./build_connector_container.sh --print-image 0.29.0       # print the ref, don't build
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"           # build context = repo root
DOCKERFILE="${DOCKERFILE:-${SCRIPT_DIR}/Dockerfile}"

usage() {
  cat <<'EOF'
build_connector_container.sh — build the certus-shmq client image on a vLLM base.

usage:
  build_connector_container.sh <vllm-version>        e.g. 0.29.0
  VLLM_VERSION=0.30.0 build_connector_container.sh    version via env

The version selects the vllm/vllm-openai:v<X> BASE image (build arg only). The
connector self-adapts to the installed vLLM at run time, so no code keys off it.

IMPORTANT: pass a FULL patch version (0.29.0), not a bare minor (0.29). vLLM
publishes only full patch tags (vX.Y.Z) — there is no v0.29 tag, and podman
reports a missing tag as a misleading "unauthorized: incorrect username or
password", not "not found".

versions (connector compat matrix — certus_shmq_connector.compat, self-adapting):
  0.20.0  0.22.0  0.23.0  0.24.0  0.26.0  0.29.0  0.30.0
also built + smoke-tested from this script:
  0.27.0  0.28.0
(any published vllm/vllm-openai:vX.Y.Z tag works; the matrix versions are the
ones the connector has capability rows for. List real base tags with:
  skopeo list-tags docker://docker.io/vllm/vllm-openai )

options:
  --help, -h              show this help and exit
  --print-image <ver>     print the resolved IMAGE ref and exit (no build)

env overrides:
  TAG=vllm0.29-test       override the :tag (default vllm<version>)
  IMAGE=repo:tag          override the full image ref
  IMAGE_REPO=<repo>       override the repo (default localhost/certus-shmq-connector)
  PODMAN_STORE= PODMAN_RUNROOT=   use a different podman store
                          (default /mnt/certus1/podman/{storage,run})
  NO_CACHE=1              pass --no-cache to podman build

examples:
  build_connector_container.sh 0.29.0
  IMAGE=$(build_connector_container.sh --print-image 0.29.0) \
    ../benchmarks/kv-offload-replay/run-docker-certus-shmq.sh
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then usage; exit 0; fi

# --print-image: emit the resolved IMAGE ref and exit (no build). Handy for
# `IMAGE=$(... --print-image X) run-docker-certus-shmq.sh`.
PRINT_ONLY=0
if [[ "${1:-}" == "--print-image" ]]; then PRINT_ONLY=1; shift; fi

# vLLM version: first positional arg, else $VLLM_VERSION. No silent default —
# this script exists to build a *specific* version.
VLLM_VERSION="${1:-${VLLM_VERSION:-}}"
if [[ -z "$VLLM_VERSION" ]]; then
  echo "error: no vLLM version given" >&2
  echo "usage: $0 <vllm-version>   (e.g. $0 0.29.0), or set VLLM_VERSION=" >&2
  echo "run '$0 --help' for the supported versions" >&2
  exit 2
fi
# Accept a leading 'v' (v0.29.0) — the base tag adds its own 'v'.
VLLM_VERSION="${VLLM_VERSION#v}"

# Image ref: IMAGE overrides everything; otherwise <repo>:<tag>, tag = vllm<ver>.
IMAGE_REPO="${IMAGE_REPO:-localhost/certus-shmq-connector}"
TAG="${TAG:-vllm${VLLM_VERSION}}"
IMAGE="${IMAGE:-${IMAGE_REPO}:${TAG}}"

if [[ "$PRINT_ONLY" == "1" ]]; then echo "$IMAGE"; exit 0; fi

# Podman store (defaults to the /mnt/certus1 store the run scripts use).
PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"
STORE_FLAGS=(--root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT")

command -v podman >/dev/null 2>&1 || { echo "error: podman not on PATH" >&2; exit 1; }
[[ -f "$DOCKERFILE" ]] || { echo "error: Dockerfile not found at ${DOCKERFILE}" >&2; exit 1; }

build_flags=()
[[ -n "${NO_CACHE:-}" ]] && build_flags+=(--no-cache)

echo "[build] image      : ${IMAGE}"
echo "[build] vLLM base  : docker.io/vllm/vllm-openai:v${VLLM_VERSION}"
echo "[build] store      : ${PODMAN_STORE}"
echo "[build] context    : ${REPO_ROOT}"
echo "[build] dockerfile : ${DOCKERFILE}"

podman "${STORE_FLAGS[@]}" build \
  --build-arg "VLLM_VERSION=${VLLM_VERSION}" \
  "${build_flags[@]}" \
  -f "$DOCKERFILE" \
  -t "$IMAGE" \
  "$REPO_ROOT"

echo
# Verify the image really landed in THIS store, and print its id. The image is
# built into the /mnt/certus1 store (--root/--runroot above), NOT podman's
# default store — so a bare `podman images` looks empty and invites a needless
# rebuild. Confirm + show the store-qualified list command so that can't mislead.
if img_id="$(podman "${STORE_FLAGS[@]}" image inspect "$IMAGE" --format '{{.Id}}' 2>/dev/null)"; then
  echo "[build] done: ${IMAGE}  (${img_id:0:12})"
else
  echo "[build] WARNING: build reported success but ${IMAGE} is not in ${PODMAN_STORE}" >&2
  echo "[build]          (check the build output above for an error)" >&2
fi
echo "[build] NOTE: this image is in the ${PODMAN_STORE} store, not podman's default"
echo "[build]       store — a bare 'podman images' will NOT show it. List it with:"
echo "    podman --root ${PODMAN_STORE} --runroot ${PODMAN_RUNROOT} images"
echo "[build] run it with, e.g.:"
echo "    IMAGE=${IMAGE} ${REPO_ROOT}/benchmarks/kv-offload-replay/run-docker-certus-shmq.sh"
