#!/bin/bash
# Build the two kv-offload-otel-replay images (context = repo root). Same store
# split as the versioned builders: the offload-family image goes to the default
# podman store (cf. build_cputier_container.sh); the shmq client image goes to
# the /mnt/certus1 store (where run-docker-otel-shmq.sh looks for it). Does NOT
# abort on a single failure — records per-image rc.
#
# The shmq image is built via certus-shmq-connector/build_connector_container.sh,
# so it inherits the connector's version handling: the --no-deps offline-install
# fix (needed on CUDA-13 bases, vLLM >= 0.27) and the full-patch-version guidance.
# VLLM_VERSION must be a FULL patch version (0.26.0), not a bare minor (0.26) —
# vLLM publishes only vX.Y.Z base tags. See:
#   certus-shmq-connector/build_connector_container.sh --help
#
#   ./build-otel.sh                         # both, vLLM 0.26.0
#   VLLM_VERSION=0.29.0 ./build-otel.sh      # pin a different base (full patch ver)
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
      -t certus-otel-offload .
fi

# shmq client image -> the /mnt/certus1 podman store (run-docker-otel-shmq.sh
# reads it there via --root/--runroot). Delegated to the connector build helper
# so it shares the connector's version handling + --no-deps offline-install fix.
# We only override the Dockerfile, the image ref (kept unqualified so it lands as
# localhost/certus-otel-shmq-connector:latest, which run-docker-otel-shmq.sh expects)
# and the store. VLLM_VERSION is passed as the helper's positional version arg.
if [ "$ONLY" = "both" ] || [ "$ONLY" = "shmq" ]; then
  build otel-shmq \
    env DOCKERFILE="$REPO_ROOT/benchmarks/kv-offload-otel-replay/Dockerfile.otel-shmq" \
        IMAGE="certus-otel-shmq-connector" \
        PODMAN_STORE="$PODMAN_STORE" PODMAN_RUNROOT="$PODMAN_RUNROOT" \
        NO_CACHE="${NO_CACHE:-}" \
      certus-shmq-connector/build_connector_container.sh "$VLLM_VERSION"
fi

echo "[build] DONE $(date +%H:%M:%S)"
