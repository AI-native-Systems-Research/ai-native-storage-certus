#!/bin/bash
# cc131k-step0-build.sh — STEP 0: ensure the Certus server binary used by step 3
# is built with SPDK support.
#
# Builds certus-server-yaml (the YAML/CLI-configurable server) with the SPDK
# backend (--features spdk) under the full-optimized link profile — the same
# binary + profile the cc-traces evaluators use. The build is incremental, so
# re-running when nothing changed is cheap and safe before every session.
#
# Requires SPDK prebuilt at deps/spdk-build (one-time: deps/build_spdk.sh).
# NOTE: this builds the Rust server only. The podman replay images are a
# separate concern — build them with ./build-otel.sh (VLLM_VERSION=0.29.0 ...).
#
# Override via env:
#   CERTUS_PROFILE=full-optimized   # SPDK link profile (default; or full)
#   ./cc131k-step0-build.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

CERTUS_PROFILE="${CERTUS_PROFILE:-full-optimized}"
BIN="${REPO_ROOT}/target/release/certus-server-yaml"

if [[ ! -d "${REPO_ROOT}/deps/spdk-build" ]]; then
  echo "error: SPDK not prebuilt at ${REPO_ROOT}/deps/spdk-build" >&2
  echo "       build it once with: ${REPO_ROOT}/deps/build_spdk.sh" >&2
  exit 1
fi

echo "[step0] building certus-server-yaml (CERTUS_PROFILE=${CERTUS_PROFILE}, release, --features spdk)"
echo "[step0] repo: ${REPO_ROOT}"
echo

CERTUS_PROFILE="${CERTUS_PROFILE}" cargo build --release \
  --manifest-path "${REPO_ROOT}/Cargo.toml" \
  -p certus-server-yaml \
  --features spdk

echo
if [[ -x "$BIN" ]]; then
  echo "[step0] built: $BIN"
  ls -la "$BIN"
  echo "[step0] next: start it with  ./cc131k-step3-server.sh"
else
  echo "error: build reported success but binary missing: $BIN" >&2
  exit 1
fi
