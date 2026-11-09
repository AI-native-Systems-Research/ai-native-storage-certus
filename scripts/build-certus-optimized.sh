#!/usr/bin/env bash
# Build certus-server-yaml with the optimized eviction policy.
#
# Selects the `full-optimized` profile (identical to `full`, but wires in
# eviction-policy-optimized instead of eviction-policy-lru) and compiles the
# SPDK NVMe + GPU stack. The optimized policy is an O(1) index-linked LRU with a
# TinyLFU (Count-Min Sketch) admission gate. The profile is baked in at compile
# time by build.rs code generation, so CERTUS_PROFILE must be set for the build,
# not just at run time.
#
# Usage:
#   scripts/build-certus-optimized.sh                # release build, spdk feature
#   PROFILE_BUILD=debug scripts/build-certus-optimized.sh   # debug build
#   scripts/build-certus-optimized.sh --features otel       # extra cargo args pass through
set -euo pipefail

# Resolve repo root from this script's location (scripts/).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

CERTUS_PROFILE="${CERTUS_PROFILE:-full-optimized}"
PROFILE_BUILD="${PROFILE_BUILD:-release}"

RELEASE_FLAG=()
if [[ "${PROFILE_BUILD}" == "release" ]]; then
    RELEASE_FLAG=(--release)
fi

echo "Building certus-server-yaml"
echo "  profile : ${CERTUS_PROFILE}"
echo "  build   : ${PROFILE_BUILD}"
echo "  features: spdk (default) ${*:-}"

cd "${REPO_ROOT}"
CERTUS_PROFILE="${CERTUS_PROFILE}" \
    cargo build -p certus-server-yaml "${RELEASE_FLAG[@]}" "$@"

echo
echo "Built ./target/${PROFILE_BUILD}/certus-server-yaml (profile: ${CERTUS_PROFILE})"
echo "Run e.g.: ./target/${PROFILE_BUILD}/certus-server-yaml --drive-count 4 --format --memory-tier-size 2G"
