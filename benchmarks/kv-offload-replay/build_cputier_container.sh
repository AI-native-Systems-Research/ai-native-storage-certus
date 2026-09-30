#!/bin/bash
# build_cputier_container.sh — build the cputier / offload-family image on a given
# vLLM base version. This is the CPUTIER counterpart of
# certus-shmq-connector/build_connector_container.sh (which builds the shmq image).
#
# The image is Dockerfile.offload: a stock vLLM plus the KV-offload replay driver.
# ONE image serves NoOffload, CPUOffload and Tiered (cputier) — the backend is
# picked at RUN time by the run scripts' OFFLOAD_MODE / SECONDARY_TIER env, so
# there is nothing cputier-specific baked in beyond the driver. run-docker-cputier.sh
# / run-cputier-docker.sh / profile_all.sh all drive this image.
#
# STORE: built into podman's DEFAULT store (NOT the /mnt/certus1 store the shmq
# image uses). That is where run-docker-cputier.sh and friends look for it, and a
# plain `podman images` will list it. Contrast build_connector_container.sh, which
# builds the shmq image into /mnt/certus1 and needs --root/--runroot to be seen.
#
# The vLLM version is a BUILD ARG selecting the vllm/vllm-openai:v<X> base image.
# VLLM_FIX_TIERING=1 (the Dockerfile default) applies the pure-Python tiering
# deferred-finalize overlay (fix#2), but Dockerfile.offload only applies it on a
# 0.26.x base — on any other version it warns and builds STOCK vLLM. (The
# _build_store_jobs assert, fix#3, is a separate bug that vLLM fixed upstream:
# fully from 0.30, partially from 0.27 — see apps/vllm-serve/patches/PROVENANCE.md.)
#
# Usage:
#   ./build_cputier_container.sh <vllm-version>            # e.g. 0.29.0
#   VLLM_VERSION=0.30.0 ./build_cputier_container.sh        # version via env
#   FIX_TIERING=0 ./build_cputier_container.sh 0.26.0       # stock/crashing baseline (0.26 A/B)
#   TAG=vllm0.29-test ./build_cputier_container.sh 0.29.0   # override the :tag
#   IMAGE=certus-offload:mine ./build_cputier_container.sh 0.29.0  # override full ref
#   NO_CACHE=1 ./build_cputier_container.sh 0.29.0          # --no-cache
#   ./build_cputier_container.sh --print-image 0.29.0       # print the ref, don't build
#
# Then point the run script at it:
#   IMAGE=certus-offload:vllm0.29.0 ./run-docker-cputier.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"          # build context = repo root
DOCKERFILE="${DOCKERFILE:-${SCRIPT_DIR}/Dockerfile.offload}"

usage() {
  cat <<'EOF'
build_cputier_container.sh — build the cputier / offload-family image on a vLLM base.

usage:
  build_cputier_container.sh <vllm-version>        e.g. 0.29.0
  VLLM_VERSION=0.30.0 build_cputier_container.sh    version via env

ONE Dockerfile.offload image serves NoOffload / CPUOffload / Tiered (cputier);
the backend is selected at run time by the run scripts' env, not at build time.
The version selects the vllm/vllm-openai:v<X> BASE image (build arg only).

Built into podman's DEFAULT store (a bare `podman images` will show it) — this is
where run-docker-cputier.sh looks. (The shmq image, by contrast, goes to the
/mnt/certus1 store via certus-shmq-connector/build_connector_container.sh.)

IMPORTANT: pass a FULL patch version (0.29.0), not a bare minor (0.29). vLLM
publishes only full patch tags (vX.Y.Z); podman reports a missing tag as a
misleading "unauthorized: incorrect username or password", not "not found".

options:
  --help, -h              show this help and exit
  --print-image <ver>     print the resolved IMAGE ref and exit (no build)

env overrides:
  TAG=vllm0.29-test       override the :tag (default vllm<version>)
  IMAGE=repo:tag          override the full image ref
  IMAGE_REPO=<repo>       override the repo (default certus-offload)
  FIX_TIERING=0|1         VLLM_FIX_TIERING build arg (default 1; the tiering fix#2
                          overlay is applied by Dockerfile.offload on a 0.26.x base
                          only, else it warns and builds stock). Use 0 for the
                          deliberate stock/crashing baseline in a 0.26 A/B.
  NO_CACHE=1              pass --no-cache to podman build

examples:
  build_cputier_container.sh 0.29.0
  IMAGE=certus-offload:vllm0.29.0 ./run-docker-cputier.sh
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then usage; exit 0; fi

# --print-image: emit the resolved IMAGE ref and exit (no build).
PRINT_ONLY=0
if [[ "${1:-}" == "--print-image" ]]; then PRINT_ONLY=1; shift; fi

# vLLM version: first positional arg, else $VLLM_VERSION. No silent default —
# this script exists to build a *specific* version.
VLLM_VERSION="${1:-${VLLM_VERSION:-}}"
if [[ -z "$VLLM_VERSION" ]]; then
  echo "error: no vLLM version given" >&2
  echo "usage: $0 <vllm-version>   (e.g. $0 0.29.0), or set VLLM_VERSION=" >&2
  echo "run '$0 --help' for details" >&2
  exit 2
fi
# Accept a leading 'v' (v0.29.0) — the base tag adds its own 'v'.
VLLM_VERSION="${VLLM_VERSION#v}"

# Image ref: IMAGE overrides everything; otherwise <repo>:<tag>, tag = vllm<ver>.
IMAGE_REPO="${IMAGE_REPO:-certus-offload}"
TAG="${TAG:-vllm${VLLM_VERSION}}"
IMAGE="${IMAGE:-${IMAGE_REPO}:${TAG}}"

if [[ "$PRINT_ONLY" == "1" ]]; then echo "$IMAGE"; exit 0; fi

# Tiering fix#2 overlay build arg (Dockerfile default is 1; only takes effect on a
# 0.26.x base — Dockerfile.offload warns + builds stock otherwise).
FIX_TIERING="${FIX_TIERING:-1}"

command -v podman >/dev/null 2>&1 || { echo "error: podman not on PATH" >&2; exit 1; }
[[ -f "$DOCKERFILE" ]] || { echo "error: Dockerfile not found at ${DOCKERFILE}" >&2; exit 1; }

build_flags=()
[[ -n "${NO_CACHE:-}" ]] && build_flags+=(--no-cache)

echo "[build] image        : ${IMAGE}"
echo "[build] vLLM base    : docker.io/vllm/vllm-openai:v${VLLM_VERSION}"
echo "[build] fix-tiering  : VLLM_FIX_TIERING=${FIX_TIERING} (0.26.x-only overlay)"
echo "[build] store        : default (bare 'podman images' will show it)"
echo "[build] context      : ${REPO_ROOT}"
echo "[build] dockerfile   : ${DOCKERFILE}"

# Default podman store on purpose — no --root/--runroot (see header).
podman build \
  --build-arg "VLLM_VERSION=${VLLM_VERSION}" \
  --build-arg "VLLM_FIX_TIERING=${FIX_TIERING}" \
  "${build_flags[@]}" \
  -f "$DOCKERFILE" \
  -t "$IMAGE" \
  "$REPO_ROOT"

echo
if img_id="$(podman image inspect "$IMAGE" --format '{{.Id}}' 2>/dev/null)"; then
  echo "[build] done: ${IMAGE}  (${img_id:0:12})"
else
  echo "[build] WARNING: build reported success but ${IMAGE} is not in the default store" >&2
  echo "[build]          (check the build output above for an error)" >&2
fi
echo "[build] run it with, e.g.:"
echo "    IMAGE=${IMAGE} ${SCRIPT_DIR}/run-docker-cputier.sh"
