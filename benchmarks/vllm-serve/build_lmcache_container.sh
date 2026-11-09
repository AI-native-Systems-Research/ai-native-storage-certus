#!/bin/bash
# build_lmcache_container.sh — build the vLLM + LMCache image on a given vLLM base
# version. The LMCache counterpart of build_h3fs_container.sh / build_cputier_container.sh
# (same version-handling UX); the image itself is defined in deploy/lmcache/ and
# this just forwards to deploy/lmcache/build.sh.
#
# The stock vllm/vllm-openai image already bundles a matching LMCache (0.5.5 in
# v0.30.0), so the build is a thin layer — no compile. run-serve-lmcache*.sh run it.
#
#   ./build_lmcache_container.sh <vllm-version>            # e.g. 0.30.0
#   LMCACHE_VERSION=0.5.6 ./build_lmcache_container.sh 0.30.0
#   ./build_lmcache_container.sh --print-image 0.30.0      # print the ref, don't build
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY_DIR="$(cd "${SCRIPT_DIR}/../../deploy/lmcache" && pwd)"

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
fi
PRINT_ONLY=0
if [[ "${1:-}" == "--print-image" ]]; then PRINT_ONLY=1; shift; fi

VLLM_VERSION="${1:-${VLLM_VERSION:-}}"
if [[ -z "$VLLM_VERSION" ]]; then
  echo "error: no vLLM version given" >&2
  echo "usage: $0 <vllm-version>   (e.g. $0 0.30.0), or set VLLM_VERSION=" >&2
  exit 2
fi
VLLM_VERSION="${VLLM_VERSION#v}"

if [[ "$PRINT_ONLY" == "1" ]]; then
  echo "${IMAGE:-localhost/vllm-lmcache-bench:vllm${VLLM_VERSION}${LMCACHE_VERSION:+-lmcache${LMCACHE_VERSION}}}"
  exit 0
fi
exec env VLLM_VERSION="$VLLM_VERSION" "${DEPLOY_DIR}/build.sh"
