#!/usr/bin/env bash
# run-in-container.sh — build the bench image and run profile-hardware-ceiling.sh
# inside it, on an immutable bootc host where host packages cannot be installed.
#
# Two modes:
#   (default)  deps-only image (Containerfile): detection, CPU features
#              (python3.12 + py-cpuinfo), host-RAM memcpy. GPU paths skip (no nvcc).
#   --gpu      CUDA image (Containerfile.cuda) + GPU passthrough via CDI, so the
#              GPU H2D/D2H and D2D benches also run. Requires the host to expose
#              GPUs to podman (nvidia-ctk + /etc/cdi/nvidia.yaml).
#
# NVMe is OFF unless you name scratch devices with --nvme (passed through to the
# profiler). fio reads them (add --fio-write for destructive writes); --spdk adds
# the SPDK paths (destructive rebind). With no --nvme, nothing touches any drive.
#
# The container gets /dev and the repo mounted read-write at /work so the report
# lands back on the host. All args except --gpu pass straight to the profiler.
#
# Usage:
#   <dir>/run-in-container.sh [outdir]                              # no NVMe
#   <dir>/run-in-container.sh --gpu [outdir]                        # + GPU benches
#   <dir>/run-in-container.sh --gpu --nvme /dev/nvme5n1 [outdir]    # + fio read on a scratch drive
#   <dir>/run-in-container.sh --nvme /dev/nvme5n1 --fio-write [outdir]
#
# Requires podman (present on bootc/ostree hosts). Behind a proxy, export
# HTTPS_PROXY first so the image build (and GPU-image pull) can reach the registry.

set -euo pipefail

# --gpu is wrapper-only (selects the CUDA image + GPU passthrough). Every other
# argument — the outdir, --nvme <devs>, --spdk, --fio-write, etc. — passes
# straight through to the profiler. NVMe is off unless the caller passes --nvme.
GPU=0
PASS_ARGS=()
for a in "$@"; do
  case "$a" in
    --gpu) GPU=1 ;;
    *)     PASS_ARGS+=("$a") ;;
  esac
done

# Use -P everywhere so $HERE and $REPO_ROOT are in the SAME (symlink-resolved)
# namespace — otherwise the bootc /home -> /var/home symlink makes $HERE start
# /home/... while $REPO_ROOT is /var/home/..., the prefix-strip below fails, and
# SCRIPT_REL keeps an absolute path (yielding ".//home/core/..." in the command).
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# Resolve repo root. The `|| ... && pwd` precedence trap: `(A || B) && C` would run
# pwd even when git succeeds, capturing TWO lines. Keep the fallback self-contained.
REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null)"
if [[ -z "$REPO_ROOT" ]]; then
  REPO_ROOT="$(cd "$HERE/../.." && pwd -P)"
fi
# Collapse the bootc /home -> /var/home symlink so the bind-mount source is a real
# path podman can statfs (git may report /var/home while $PWD shows /home).
REPO_ROOT="$(cd "$REPO_ROOT" && pwd -P)"

# Script path relative to the repo root — works whether this dir is tools/hw-ceiling
# (old layout) or benchmarks/hw-ceiling (tidied layout), no hardcoding.
SCRIPT_REL="${HERE#"$REPO_ROOT"/}/profile-hardware-ceiling.sh"
# Guard: if the strip failed (still absolute), the mount prefix is wrong — stop
# rather than emit ".//abs/path" that won't exist inside the container.
if [[ "$SCRIPT_REL" = /* ]]; then
  echo "[run-in-container] ERROR: script dir ($HERE) is not under repo root ($REPO_ROOT)" >&2
  exit 1
fi

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

# If --nvme <BDF[,BDF]> is being passed through, the container's root cannot open
# a root:disk block device (rootless userns maps it to nobody). Grant access by
# making ONLY those named devices' block nodes group/other-readable on the host
# first. Needs sudo; we chmod exactly the named scratch devices, nothing else.
NVME_ARG=""
_next_is_nvme=0
for a in "${PASS_ARGS[@]:-}"; do
  [[ "$_next_is_nvme" == 1 ]] && { NVME_ARG="$a"; _next_is_nvme=0; continue; }
  [[ "$a" == "--nvme" ]] && _next_is_nvme=1
done
if [[ -n "$NVME_ARG" ]]; then
  IFS=',' read -r -a _bdfs <<< "$NVME_ARG"
  for bdf in "${_bdfs[@]}"; do
    bdf="${bdf,,}"; [[ "$bdf" =~ ^[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]] && bdf="0000:${bdf}"
    cdir="$(ls -d "/sys/bus/pci/devices/${bdf}/nvme/"nvme* 2>/dev/null | head -1)"
    [[ -z "$cdir" ]] && { echo "[run-in-container] WARN: $bdf has no kernel block node (on vfio-pci?)"; continue; }
    blk="$(ls -d "$cdir"/nvme* 2>/dev/null | grep -E '/nvme[0-9]+n[0-9]+$' | head -1)"
    [[ -z "$blk" ]] && continue
    node="/dev/$(basename "$blk")"
    echo "[run-in-container] granting container access to $node (chmod o+rw; scratch device)"
    sudo chmod o+rw "$node" || echo "[run-in-container] WARN: chmod $node failed — fio in container may get EACCES"
  done
fi

# Build the in-container command with each pass-through arg safely quoted.
CMD="./${SCRIPT_REL}"
for a in "${PASS_ARGS[@]:-}"; do [[ -n "$a" ]] && CMD+=" $(printf '%q' "$a")"; done
echo "[run-in-container] running: $CMD"

# --privileged + /dev so nvme-cli/fio can read the named devices. Repo mounted rw
# so profiling/ output persists on host. :Z relabels for SELinux (enforced on bootc).
# --hostname carries the host's short name in so the report is labelled with the
# real node, not the random container id.
HOST_SHORT="$(hostname -s 2>/dev/null || cat /etc/hostname 2>/dev/null)"; HOST_SHORT="${HOST_SHORT%%.*}"
# NVMe caveat: raw block I/O (fio --nvme, SPDK) does NOT work from a rootless
# container — the device maps to nobody:nobody in the user namespace and even the
# container's root cannot open it (verified: dd and fio both get EACCES; it is the
# userns mapping, not SELinux). GPU/CPU/memory paths work fine here. For the NVMe
# paths, run profile-hardware-ceiling.sh directly on the host against scratch
# devices, e.g.  sudo ./.../profile-hardware-ceiling.sh --nvme /dev/nvmeXn1 ...
podman run --rm -it \
  --privileged \
  --hostname "${HOST_SHORT:-profiled-host}" \
  "${GPU_ARGS[@]}" \
  -v /dev:/dev \
  -v "$REPO_ROOT":/work:Z \
  -w /work \
  "$IMAGE" \
  -c "$CMD"

echo "[run-in-container] done. Report + YAML under: $REPO_ROOT/"
