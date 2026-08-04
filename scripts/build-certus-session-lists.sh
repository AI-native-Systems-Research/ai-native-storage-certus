#!/usr/bin/env bash
# Build certus-server-yaml with the session-lineage eviction policy.
#
# Selects the `full-session-lists` profile (identical to `full`, but wires in
# eviction-policy-session-lists instead of eviction-policy-lru) and compiles the
# SPDK NVMe + GPU stack. The profile is baked in at compile time by build.rs
# code generation, so CERTUS_PROFILE must be set for the build, not just at run
# time.
#
# Usage:
#   scripts/build-certus-session-lists.sh                # release build, spdk feature
#   PROFILE_BUILD=debug scripts/build-certus-session-lists.sh   # debug build
#   scripts/build-certus-session-lists.sh --features otel       # extra cargo args pass through
set -euo pipefail

# Resolve repo root from this script's location (scripts/).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

CERTUS_PROFILE="${CERTUS_PROFILE:-full-session-lists}"
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
