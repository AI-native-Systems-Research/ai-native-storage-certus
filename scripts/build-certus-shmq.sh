#!/usr/bin/env bash
# Build certus-shmq-server — the shared-memory-queue (/dev/shm mailbox)
# alternative to the gRPC control plane (apps/certus-server).
#
# Unlike the certus-server-yaml build scripts, this crate has NO build.rs
# codegen and does NOT read CERTUS_PROFILE: its SPDK NVMe + GPU stack is wired
# in unconditionally via per-dependency features (interfaces/spdk,
# dispatcher/spdk-backend, memory-tier/spdk, gpu-services/gpu). So there is no
# profile to set and no --features spdk to pass — a plain release build gives
# the full stack. It IS a workspace member but NOT a default member, so it must
# be named with -p.
#
# Requires SPDK prebuilt at deps/spdk-build/ (deps/build_spdk.sh) and a GPU.
#
# Usage:
#   scripts/build-certus-shmq.sh                          # release build
#   PROFILE_BUILD=debug scripts/build-certus-shmq.sh      # debug build
#   scripts/build-certus-shmq.sh --features integrity-check   # extra cargo args pass through
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PROFILE_BUILD="${PROFILE_BUILD:-release}"
RELEASE_FLAG=()
[[ "${PROFILE_BUILD}" == "release" ]] && RELEASE_FLAG=(--release)

echo "Building certus-shmq-server (${PROFILE_BUILD}) ${*:-}"

cargo build \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    -p certus-shmq-server \
    "${RELEASE_FLAG[@]}" "$@"

echo
echo "Built ./target/${PROFILE_BUILD}/certus-shmq-server"
echo
echo "Usage:"
echo "  certus-shmq-server --device-pci <DDDD:BB:DD.F> [--device-pci ...] \\"
echo "      --memory-tier-size <=32G> --shm-path /dev/shm/certus-shmq --format"
echo "  certus-shmq-server --drive-count <N> --memory-tier-size <=32G> --format"
