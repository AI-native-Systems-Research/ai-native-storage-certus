#!/usr/bin/env bash
# run-in-container.sh — build the bench image and run profile-hardware-ceiling.sh
# inside it, on an immutable bootc host where host packages cannot be installed.
#
# Two modes:
#   (default)  deps-only image (Containerfile): detection, CPU features
#              (python3.12 + py-cpuinfo), NVMe inspect + fio, host-RAM memcpy.
#              GPU paths skip (no nvcc). Non-destructive (--no-nvme).
#   --gpu      CUDA image (Containerfile.cuda) + GPU passthrough via CDI, so the
#              GPU H2D/D2H and D2D benches also run. Requires the host to expose
#              GPUs to podman (nvidia-ctk + /etc/cdi/nvidia.yaml). Still --no-nvme,
#              so the destructive SPDK/vfio paths stay a host-side job.
#
# The container gets /dev (nvme-cli + fio against /dev/nvme*) and the repo mounted
# read-write at /work so the report lands back on the host.
#
# Usage:
#   <dir>/run-in-container.sh [outdir]            # deps-only, default outdir profiling/
#   <dir>/run-in-container.sh --gpu [outdir]      # + GPU benches
#
# Requires podman (present on bootc/ostree hosts). Behind a proxy, export
# HTTPS_PROXY first so the image build (and GPU-image pull) can reach the registry.

set -euo pipefail

GPU=0
ARGS=()
for a in "$@"; do
  case "$a" in
    --gpu) GPU=1 ;;
    *)     ARGS+=("$a") ;;
  esac
done
OUTDIR="${ARGS[0]:-profiling}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Resolve repo root. The `|| ... && pwd` precedence trap: `(A || B) && C` would run
# pwd even when git succeeds, capturing TWO lines. Keep the fallback self-contained.
REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null)"
if [[ -z "$REPO_ROOT" ]]; then
  REPO_ROOT="$(cd "$HERE/../.." && pwd)"
fi
# Collapse the bootc /home -> /var/home symlink so the bind-mount source is a real
# path podman can statfs (git may report /var/home while $PWD shows /home).
REPO_ROOT="$(cd "$REPO_ROOT" && pwd -P)"

# Script path relative to the repo root — works whether this dir is tools/hw-ceiling
# (old layout) or benchmarks/hw-ceiling (tidied layout), no hardcoding.
SCRIPT_REL="${HERE#"$REPO_ROOT"/}/profile-hardware-ceiling.sh"

if [[ "$GPU" == 1 ]]; then
  IMAGE="certus-hwceiling:cuda"
  DOCKERFILE="$HERE/Containerfile.cuda"
else
  IMAGE="certus-hwceiling:deps"
  DOCKERFILE="$HERE/Containerfile"
fi

echo "[run-in-container] repo root:   $REPO_ROOT"
echo "[run-in-container] script:      $SCRIPT_REL"
echo "[run-in-container] image:       $IMAGE ($([[ $GPU == 1 ]] && echo 'GPU/CUDA' || echo 'deps-only'))"

# Pass proxy through to the build if the caller has one set.
BUILD_ARGS=()
if [[ -n "${HTTPS_PROXY:-}" ]]; then
  BUILD_ARGS+=(--build-arg "https_proxy=$HTTPS_PROXY" --build-arg "http_proxy=${HTTP_PROXY:-$HTTPS_PROXY}")
  echo "[run-in-container] proxy:       $HTTPS_PROXY (for build)"
fi

podman build "${BUILD_ARGS[@]}" -t "$IMAGE" -f "$DOCKERFILE" "$HERE"

# GPU passthrough via CDI when --gpu.
GPU_ARGS=()
if [[ "$GPU" == 1 ]]; then
  GPU_ARGS+=(--device nvidia.com/gpu=all)
  echo "[run-in-container] GPU:         --device nvidia.com/gpu=all (CDI)"
fi

echo "[run-in-container] running profiler (--no-nvme) ..."

# --privileged + /dev so nvme-cli/fio can read the drives; no vfio rebind happens
# because we pass --no-nvme. Repo mounted rw so profiling/ output persists on host.
# :Z relabels for SELinux (enforced on RHEL/bootc).
# --hostname: carry the host's short name into the container so the report/YAML
# is labelled with the real node, not the random container id.
HOST_SHORT="$(hostname -s 2>/dev/null || cat /etc/hostname 2>/dev/null)"; HOST_SHORT="${HOST_SHORT%%.*}"
podman run --rm -it \
  --privileged \
  --hostname "${HOST_SHORT:-profiled-host}" \
  "${GPU_ARGS[@]}" \
  -v /dev:/dev \
  -v "$REPO_ROOT":/work:Z \
  -w /work \
  "$IMAGE" \
  -c "./${SCRIPT_REL} --no-nvme '$OUTDIR'"

echo "[run-in-container] done. Report + YAML under: $REPO_ROOT/$OUTDIR/"
