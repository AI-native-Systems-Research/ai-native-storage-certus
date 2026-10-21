#!/bin/bash
# build.sh — build the single-node 3FS cluster image (rootless; no sudo needed).
#
# Pins the 3FS source to the SAME commit as the client wheel inside the vLLM image,
# so server and USRBIO client are wire-compatible:
#   localhost/vllm-hf3fs-bench:vllm0.30.0  ->  hf3fs_py_usrbio 1.2.9+22fca04  ->  HF3FS_REF=22fca04
#
#   ./build.sh                              # ref taken from CLIENT_IMAGE
#   HF3FS_REF=<sha> ./build.sh              # explicit ref (must match your client!)
#   CLIENT_IMAGE=localhost/vllm-hf3fs-bench:vllm0.31.0 ./build.sh
#   STORE_DIR=/home/dwaddington/podman-3fs ./build.sh   # image store + build temp in this folder
#
# The image lands in YOUR rootless store; run.sh copies it into root's store on first
# use (the cluster container must be rootful to mount FUSE onto the host).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
source ./common.sh   # STORE_DIR → PODMAN=(podman --root … --runroot …), TMPDIR

CLIENT_IMAGE="${CLIENT_IMAGE:-localhost/vllm-hf3fs-bench:vllm0.30.0}"
IMAGE="${IMAGE:-localhost/certus-3fs-cluster}"
SHUFFLE_METHOD="${SHUFFLE_METHOD:-stdshuffle}"   # must match Dockerfile.hf3fs HF3FS_SHUFFLE_METHOD
BUILD_JOBS="${BUILD_JOBS:-32}"

if [[ -z "${HF3FS_REF:-}" ]]; then
  ver=$(podman run --rm --entrypoint python3 "$CLIENT_IMAGE" -c \
        'import importlib.metadata as m; print(m.version("hf3fs_py_usrbio"))') \
    || { echo "error: cannot read hf3fs_py_usrbio version from $CLIENT_IMAGE; set HF3FS_REF" >&2; exit 1; }
  HF3FS_REF="${ver##*+}"
  [[ "$ver" == *+* && -n "$HF3FS_REF" ]] || { echo "error: version '$ver' carries no +<sha>; set HF3FS_REF" >&2; exit 1; }
  echo "[build] client $CLIENT_IMAGE has hf3fs_py_usrbio $ver -> HF3FS_REF=$HF3FS_REF"
fi
TAG="${TAG:-$HF3FS_REF}"

# buildah stages each layer as a tar under $TMPDIR (default /var/tmp, on the 70G root
# fs). The 3FS build tree is tens of GB: with no STORE_DIR, stage on /home instead.
if [[ -z "${STORE_DIR:-}" ]]; then
  export TMPDIR="${TMPDIR:-$HOME/.cache/certus-3fs-buildtmp}"
  mkdir -p "$TMPDIR"
fi
need_space "$TMPDIR" 60 "the 3FS build (layer staging)"

"${PODMAN[@]}" build --format docker -f Containerfile \
  --build-arg HF3FS_REF="$HF3FS_REF" \
  --build-arg SHUFFLE_METHOD="$SHUFFLE_METHOD" \
  --build-arg BUILD_JOBS="$BUILD_JOBS" \
  -t "$IMAGE:$TAG" -t "$IMAGE:latest" .

echo "[build] built $IMAGE:$TAG (also :latest)${STORE_DIR:+ in store $STORE_DIR}."
echo "[build] Next: sudo ${STORE_DIR:+BUILD_STORE_DIR=$STORE_DIR }STORE_DIR=<root store dir> ./run.sh"
