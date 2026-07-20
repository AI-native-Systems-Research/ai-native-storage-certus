#!/bin/bash
# Entrypoint for the SharedStorage backend image.
#
# The KV tier is a host RAID0/XFS bind-mounted at /mnt/fs-backend-bench. This
# script sanity-checks that mount, then execs the driver. The driver's own
# preflight() re-checks the mount + RAM cap and pins NUMA; set SKIP_PREFLIGHT=1
# to bypass it (e.g. when the bind mount doesn't register as a mountpoint inside
# the container, or the host isn't RAM-capped for a non-faithful smoke run).
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/bench/run_fs_bench_450.py}"
KV_MOUNT="/mnt/fs-backend-bench"
KV_PATH="${KV_MOUNT}/shared-kv"

if [ ! -d "${KV_PATH}" ]; then
    echo "[entrypoint] WARNING: ${KV_PATH} not present in the container." >&2
    echo "[entrypoint]          Bind-mount the host RAID: -v ${KV_MOUNT}:${KV_MOUNT}" >&2
    echo "[entrypoint]          (set it up on the host first: sudo tools/configure-bench.sh sharedstorage)" >&2
elif [ ! -w "${KV_PATH}" ]; then
    echo "[entrypoint] WARNING: ${KV_PATH} is not writable by this container user." >&2
    echo "[entrypoint]          On the host: sudo chown -R \$USER:\$(id -gn) ${KV_PATH}" >&2
fi

echo "[entrypoint] SharedStorage run: NUM_CONVS=${NUM_CONVS:-?} MODEL=${MODEL:-?} DRAM=${DRAM:-?} KV_PATH=${KV_PATH} SKIP_PREFLIGHT=${SKIP_PREFLIGHT:-0}"
exec python3 "${WORKLOAD}"
