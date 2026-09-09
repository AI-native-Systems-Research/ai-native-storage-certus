#!/bin/bash
# Build the two kv-offload-otel-replay images (context = repo root). Mirrors
# build_026.sh: the offload-family image goes to the default podman store; the
# shmq client image goes to the /mnt/certus1 store (where run-docker-otel-shmq.sh
# looks for it). Does NOT abort on a single failure — records per-image rc.
#
#   ./build-otel.sh                         # both, vLLM 0.26.0
#   VLLM_VERSION=0.23.0 ./build-otel.sh      # pin a different base
#   ONLY=offload ./build-otel.sh             # just the offload image
#   ONLY=shmq ./build-otel.sh                # just the shmq image
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT" || exit 2

VLLM_VERSION="${VLLM_VERSION:-0.26.0}"
BA=(--build-arg "VLLM_VERSION=${VLLM_VERSION}")
ONLY="${ONLY:-both}"
PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"

build() {
  local name="$1"; shift
  echo "[build] $name starting $(date +%H:%M:%S)"
  echo "        $*"
  "$@"
  echo "[build] $name rc=$?"
}

# Offload family (NoOffload / CPUOffload / Tiered via run-time env). Bakes the
# tiering fix by default (VLLM_FIX_TIERING=1) so the Tiered arm survives at scale.
if [ "$ONLY" = "both" ] || [ "$ONLY" = "offload" ]; then
  build otel-offload \
    podman build "${BA[@]}" \
      -f benchmarks/kv-offload-otel-replay/Dockerfile.otel-offload \
      -t certus-otel-offload-bench .
fi

# shmq client image -> the /mnt/certus1 podman store (run-docker-otel-shmq.sh
# reads it there via --root/--runroot).
if [ "$ONLY" = "both" ] || [ "$ONLY" = "shmq" ]; then
  build otel-shmq \
    podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" \
      build "${BA[@]}" \
      -f benchmarks/kv-offload-otel-replay/Dockerfile.otel-shmq \
      -t certus-otel-shmq-bench .
fi

echo "[build] DONE $(date +%H:%M:%S)"
