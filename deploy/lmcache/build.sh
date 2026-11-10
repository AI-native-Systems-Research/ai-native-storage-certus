#!/bin/bash
# build.sh — build the vLLM + LMCache image (rootless; no sudo needed).
#
# A thin layer over docker.io/vllm/vllm-openai:v<VLLM_VERSION>, which already
# bundles a matching LMCache (0.5.5 in v0.30.0) — so this takes seconds, not the
# 3FS build's half hour. See Containerfile for what the layer adds.
#
#   ./build.sh                                   # vLLM 0.30.0, bundled LMCache
#   VLLM_VERSION=0.31.0 ./build.sh               # another vLLM base (full x.y.z tag)
#   LMCACHE_VERSION=0.5.6 ./build.sh             # swap LMCache (must match the base's torch/CUDA)
#   NO_CACHE=1 ./build.sh
#
# Built into podman's DEFAULT rootless store (a bare `podman images` lists it),
# which is where run.sh and benchmarks/vllm-serve/run-serve-lmcache*.sh look.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

VLLM_VERSION="${VLLM_VERSION:-0.30.0}"
VLLM_VERSION="${VLLM_VERSION#v}"
LMCACHE_VERSION="${LMCACHE_VERSION:-}"
IMAGE_REPO="${IMAGE_REPO:-localhost/vllm-lmcache-bench}"
TAG="${TAG:-vllm${VLLM_VERSION}${LMCACHE_VERSION:+-lmcache${LMCACHE_VERSION}}}"
IMAGE="${IMAGE:-${IMAGE_REPO}:${TAG}}"

command -v podman >/dev/null 2>&1 || { echo "error: podman not on PATH" >&2; exit 1; }

build_flags=()
[[ -n "${NO_CACHE:-}" ]] && build_flags+=(--no-cache)

echo "[build] image     : ${IMAGE}"
echo "[build] vLLM base : docker.io/vllm/vllm-openai:v${VLLM_VERSION}"
echo "[build] LMCache   : ${LMCACHE_VERSION:-<bundled with the base>}"

podman build --format docker -f Containerfile \
  --build-arg "VLLM_VERSION=${VLLM_VERSION}" \
  --build-arg "LMCACHE_VERSION=${LMCACHE_VERSION}" \
  "${build_flags[@]}" \
  -t "$IMAGE" .

lmc=$(podman run --rm --entrypoint cat "$IMAGE" /opt/lmcache/LMCACHE_VERSION)
echo "[build] done: ${IMAGE} (lmcache ${lmc}). Next: ./run.sh && ./smoke.sh"
