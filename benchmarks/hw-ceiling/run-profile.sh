#!/usr/bin/env bash
# run-profile.sh — one command for the full NON-DESTRUCTIVE hardware-ceiling
# profile, entirely inside the CUDA container: GPU H2D/D2H + D2D, host-RAM memcpy,
# CPU features (python3.12 + py-cpuinfo in the image), NVMe inspect, and fio
# kernel-driver NVMe READ. One image, one report dir, no host python/fio needed.
#
# The container does everything (it has nvcc, python3.12, fio). For raw NVMe the
# container's root cannot open a root:disk device, so run-in-container.sh chmods
# the named scratch block node(s) o+rw on the host first (sudo) — only the
# devices you pass with --nvme, nothing else.
#
# NON-DESTRUCTIVE: fio runs READ-only; no vfio rebind, no writes. The chmod needs
# sudo. For destructive paths (fio --write, SPDK) use run-spdk-nvme.sh or
# profile-hardware-ceiling.sh --fio-write directly.
#
# Usage:
#   benchmarks/hw-ceiling/run-profile.sh [outdir] [--nvme BDF[,BDF...]]
#
#   benchmarks/hw-ceiling/run-profile.sh profiling/run1                      # GPU + memory
#   benchmarks/hw-ceiling/run-profile.sh profiling/run1 --nvme 0000:41:00.0  # + fio read
#
# Env: HTTPS_PROXY is passed to the container build if set (registry pull).

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

OUTDIR="profiling"
PASS_ARGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    --nvme|--tag|--fio-runtime|--total-bytes) PASS_ARGS+=("$1" "$2"); shift 2 ;;
    --detect-only|--spdk|--fio-write|--skip-build) PASS_ARGS+=("$1"); shift ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) OUTDIR="$1"; shift ;;
  esac
done

# One container pass does it all. All flags pass through to the profiler.
ARGS=(--gpu "$OUTDIR" "${PASS_ARGS[@]}")

exec "$HERE/run-in-container.sh" "${ARGS[@]}"
