#!/bin/bash
# build_h3fs_container.sh — build the vLLM + 3FS (HF3FS) image on a given vLLM base
# version. This is the HF3FS counterpart of build_cputier_container.sh (which builds
# the cputier / offload-family image) — same version-handling UX, different payload.
#
# The image is Dockerfile.hf3fs: a stock vLLM base plus the DeepSeek 3FS USRBIO
# python client (`hf3fs_fuse` / the compiled `hf3fs_py_usrbio` extension) that
# vLLM's in-tree HF3FSKVConnector dlopens. The stock vllm/vllm-openai image ships
# the connector code but NOT that client, so it cannot offload to 3FS on its own;
# this image adds it. run-serve-h3fs0-cc131k.sh drives the result.
#
#   connector:  HF3FSKVConnector
#   docs:       https://docs.vllm.ai/en/latest/api/vllm/distributed/kv_transfer/kv_connector/v1/hf3fs/hf3fs_connector/
#   3FS:        https://github.com/deepseek-ai/3fs
#
# STORE: the finished image lands in podman's DEFAULT store (a bare `podman images`
# will list it) — that is where run-serve-h3fs0-cc131k.sh and
# 3fs-connector-bench look (they run --pull=never, no --root/--runroot).
# STORE_DIR=<dir> builds in a separate store instead (images, layers and build
# temp under <dir>, e.g. on /raid0 when the default store's filesystem is full),
# seeds it with the vLLM base from the default store when that has it (a local
# copy, no download), and copies only the finished image back into the default
# store. KEEP_STORE=1 keeps the build store afterwards for faster rebuilds.
#
# The vLLM version is a BUILD ARG selecting the vllm/vllm-openai:v<X> base image;
# the 3FS source is pinned separately by HF3FS_REF. The connector ships in the
# modern (0.2x/0.3x) vLLM images — the stock v0.23.0 image on this host lacks the
# 3FS client, which is exactly what this build adds.
#
# HEADS-UP: the 3FS build is heavy and version-sensitive (FoundationDB, libfuse
# >=3.16.1, clang-14, Rust). The pinned FDB_VERSION / LIBFUSE_VERSION / RUST_VERSION
# / HF3FS_REF in Dockerfile.hf3fs are the knobs most likely to need adjustment for a
# given 3FS commit; they are overridable here (see env overrides below).
#
# Usage:
#   ./build_h3fs_container.sh <vllm-version>              # e.g. 0.30.0
#   VLLM_VERSION=0.30.0 ./build_h3fs_container.sh          # version via env
#   HF3FS_REF=<commit> ./build_h3fs_container.sh 0.30.0    # pin the 3FS source rev
#   TAG=vllm0.30-test ./build_h3fs_container.sh 0.30.0     # override the :tag
#   IMAGE=vllm-hf3fs:mine ./build_h3fs_container.sh 0.30.0 # override full ref
#   NO_CACHE=1 ./build_h3fs_container.sh 0.30.0            # --no-cache
#   STORE_DIR=/raid0/podman-hf3fs ./build_h3fs_container.sh 0.30.0  # build store on /raid0
#   ./build_h3fs_container.sh --print-image 0.30.0         # print the ref, don't build
#
# Then point the run script at it:
#   IMAGE=vllm-hf3fs-bench:vllm0.30.0 ./run-serve-h3fs0-cc131k.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# The Dockerfile clones 3FS and builds on the vLLM base — it needs no repo files,
# so the build context is just this dir (contrast build_cputier_container.sh, which
# uses the repo root to COPY in the benchmark driver + dataset).
DOCKERFILE="${DOCKERFILE:-${SCRIPT_DIR}/Dockerfile.hf3fs}"
CONTEXT="${CONTEXT:-${SCRIPT_DIR}}"

usage() {
  cat <<'EOF'
build_h3fs_container.sh — build the vLLM + 3FS (HF3FS) image on a vLLM base.

usage:
  build_h3fs_container.sh <vllm-version>        e.g. 0.30.0
  VLLM_VERSION=0.30.0 build_h3fs_container.sh    version via env

Dockerfile.hf3fs = a vLLM base + the DeepSeek 3FS USRBIO python client that vLLM's
HF3FSKVConnector dlopens (built from deepseek-ai/3fs in a builder stage). The
version selects the vllm/vllm-openai:v<X> BASE image (build arg); HF3FS_REF pins
the 3FS source revision.

Built into podman's DEFAULT store (a bare `podman images` will show it) — this is
where run-serve-h3fs0-cc131k.sh looks.

IMPORTANT: pass a FULL patch version (0.30.0), not a bare minor (0.30). vLLM
publishes only full patch tags (vX.Y.Z); podman reports a missing tag as a
misleading "unauthorized: incorrect username or password", not "not found".

options:
  --help, -h              show this help and exit
  --print-image <ver>     print the resolved IMAGE ref and exit (no build)

env overrides:
  TAG=vllm0.30-test       override the :tag (default vllm<version>)
  IMAGE=repo:tag          override the full image ref
  IMAGE_REPO=<repo>       override the repo (default vllm-hf3fs-bench)
  HF3FS_REF=<rev>         3FS source revision to build (default main; pin a commit
                          for reproducibility)
  FDB_VERSION=<x.y.z>     FoundationDB client version (Dockerfile default 7.3.63)
  LIBFUSE_VERSION=<x.y.z> libfuse version to build (Dockerfile default 3.16.2)
  RUST_VERSION=<x.y.z>    Rust toolchain (Dockerfile default 1.85.0)
  HF3FS_SHUFFLE_METHOD=<m> 3FS data-placement shuffle: g++10|g++11|stdshuffle
                          (Dockerfile default stdshuffle)
  NO_CACHE=1              pass --no-cache to podman build
  STORE_DIR=<dir>         build in a store under <dir>, then copy the image into the
                          default store (for when the default store's fs is full)
  KEEP_STORE=1            with STORE_DIR: keep the build store (layer cache)

examples:
  build_h3fs_container.sh 0.30.0
  IMAGE=vllm-hf3fs-bench:vllm0.30.0 ./run-serve-h3fs0-cc131k.sh
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then usage; exit 0; fi

# --print-image: emit the resolved IMAGE ref and exit (no build).
PRINT_ONLY=0
if [[ "${1:-}" == "--print-image" ]]; then PRINT_ONLY=1; shift; fi

# vLLM version: first positional arg, else $VLLM_VERSION. No silent default —
# this script exists to build a *specific* version (matches build_cputier_container.sh).
VLLM_VERSION="${1:-${VLLM_VERSION:-}}"
if [[ -z "$VLLM_VERSION" ]]; then
  echo "error: no vLLM version given" >&2
  echo "usage: $0 <vllm-version>   (e.g. $0 0.30.0), or set VLLM_VERSION=" >&2
  echo "run '$0 --help' for details" >&2
  exit 2
fi
# Accept a leading 'v' (v0.30.0) — the base tag adds its own 'v'.
VLLM_VERSION="${VLLM_VERSION#v}"

# Image ref: IMAGE overrides everything; otherwise <repo>:<tag>, tag = vllm<ver>.
IMAGE_REPO="${IMAGE_REPO:-vllm-hf3fs-bench}"
TAG="${TAG:-vllm${VLLM_VERSION}}"
IMAGE="${IMAGE:-${IMAGE_REPO}:${TAG}}"

if [[ "$PRINT_ONLY" == "1" ]]; then echo "$IMAGE"; exit 0; fi

# 3FS source revision + pinned external component versions. Empty (default) leaves
# the Dockerfile's own ARG defaults in place; a set value is forwarded as a
# --build-arg override.
HF3FS_REF="${HF3FS_REF:-}"

command -v podman >/dev/null 2>&1 || { echo "error: podman not on PATH" >&2; exit 1; }
[[ -f "$DOCKERFILE" ]] || { echo "error: Dockerfile not found at ${DOCKERFILE}" >&2; exit 1; }

build_flags=()
[[ -n "${NO_CACHE:-}" ]] && build_flags+=(--no-cache)
# Forward optional pinned-version overrides only when the caller set them, so the
# Dockerfile ARG defaults stay authoritative otherwise.
[[ -n "$HF3FS_REF" ]]               && build_flags+=(--build-arg "HF3FS_REF=${HF3FS_REF}")
[[ -n "${FDB_VERSION:-}" ]]         && build_flags+=(--build-arg "FDB_VERSION=${FDB_VERSION}")
[[ -n "${LIBFUSE_VERSION:-}" ]]     && build_flags+=(--build-arg "LIBFUSE_VERSION=${LIBFUSE_VERSION}")
[[ -n "${RUST_VERSION:-}" ]]        && build_flags+=(--build-arg "RUST_VERSION=${RUST_VERSION}")
[[ -n "${HF3FS_SHUFFLE_METHOD:-}" ]] && build_flags+=(--build-arg "HF3FS_SHUFFLE_METHOD=${HF3FS_SHUFFLE_METHOD}")

echo "[build] image        : ${IMAGE}"
echo "[build] vLLM base    : docker.io/vllm/vllm-openai:v${VLLM_VERSION}"
echo "[build] 3FS source   : ${HF3FS_REF:-<Dockerfile default: main>}"
echo "[build] store        : ${STORE_DIR:-default} (result in the default store)"
echo "[build] context      : ${CONTEXT}"
echo "[build] dockerfile   : ${DOCKERFILE}"

BUILD_PODMAN=(podman)
BASE="docker.io/vllm/vllm-openai:v${VLLM_VERSION}"
if [[ -n "${STORE_DIR:-}" ]]; then
  mkdir -p "$STORE_DIR"/{storage,run,tmp}
  # A rootless store outside the default location needs fuse-overlayfs on these
  # hosts: storage.conf's overlay mountopt "metacopy=on" is refused for an
  # unprivileged kernel-overlay mount on a fresh store (see deploy/3fs/common.sh).
  BUILD_PODMAN=(podman --root "$STORE_DIR/storage" --runroot "$STORE_DIR/run")
  command -v fuse-overlayfs >/dev/null \
    && BUILD_PODMAN+=(--storage-opt "overlay.mount_program=$(command -v fuse-overlayfs)")
  export TMPDIR="$STORE_DIR/tmp"
  # Seed the base from the default store (local copy) rather than downloading it.
  if ! "${BUILD_PODMAN[@]}" image exists "$BASE" && podman image exists "$BASE"; then
    echo "[build] copying base ${BASE} from the default store into ${STORE_DIR}"
    podman save "$BASE" | "${BUILD_PODMAN[@]}" load
  fi
fi

"${BUILD_PODMAN[@]}" build \
  --build-arg "VLLM_VERSION=${VLLM_VERSION}" \
  "${build_flags[@]}" \
  -f "$DOCKERFILE" \
  -t "$IMAGE" \
  "$CONTEXT"

if [[ -n "${STORE_DIR:-}" ]]; then
  # podman load skips layers the default store already has (the vLLM base), so
  # this writes only the 3FS client layers there.
  echo "[build] copying ${IMAGE} into the default store"
  "${BUILD_PODMAN[@]}" save "$IMAGE" | podman load   # staging TMPDIR stays under STORE_DIR
  if [[ "${KEEP_STORE:-0}" != 1 ]]; then
    "${BUILD_PODMAN[@]}" system reset --force >/dev/null 2>&1 || true
    rm -rf "$STORE_DIR" 2>/dev/null || podman unshare rm -rf "$STORE_DIR"
    echo "[build] removed build store ${STORE_DIR} (KEEP_STORE=1 keeps it)"
  fi
fi

echo
if img_id="$(podman image inspect "$IMAGE" --format '{{.Id}}' 2>/dev/null)"; then
  echo "[build] done: ${IMAGE}  (${img_id:0:12})"
else
  echo "[build] WARNING: build reported success but ${IMAGE} is not in the default store" >&2
  echo "[build]          (check the build output above for an error)" >&2
fi
echo "[build] run it with, e.g.:"
echo "    IMAGE=${IMAGE} ${SCRIPT_DIR}/run-serve-h3fs0-cc131k.sh"
