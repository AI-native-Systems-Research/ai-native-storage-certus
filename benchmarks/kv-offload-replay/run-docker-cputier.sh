#!/bin/bash
# CPU+Disk offload — vLLM 0.26 native multi-tier (OffloadingConnector ->
# TieringOffloadingSpec: CPU primary tier + "fs" disk secondary tier).
#
# This is the in-tree replacement for the SharedStorage (llmd_fs_backend)
# solution, which no longer builds on vLLM 0.26.0 (upstream removed
# vllm.v1.kv_offload.abstract). It reuses the unified certus-offload-bench image
# (which already ships vLLM 0.26 + the tiering framework) and bind-mounts the
# repo copy of the driver over the baked one, so no rebuild is needed. Setting
# DISK_DIR makes the driver take the tiering (CPU primary + fs secondary) path.
#
#   ./run-docker-cputier.sh
#   DISK_DIR_HOST=/mnt/kv-fs-tier CPU_BYTES=$((8*(1<<30))) ./run-docker-cputier.sh
#
# CPU_BYTES     = CPU primary tier (pinned host RAM); must be < free RAM.
# DISK_DIR_HOST = host directory backing the fs disk tier (must be writable by
#                 the rootless-mapped container uid, else stores silently fail).
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-offload-bench}"
CPU_BYTES="${CPU_BYTES:-$((8 * (1 << 30)))}"      # CPU primary tier (bytes)
# TieringOffloadingSpec allocates its CPU primary tier as a /dev/shm mmap and
# force-populates it with MADV_POPULATE_WRITE. Podman's default /dev/shm is 64M,
# so the populate fails with EFAULT ("Bad address"). Size /dev/shm to the CPU
# tier plus headroom (the region is padded past cpu_bytes_to_use).
SHM_BYTES="${SHM_BYTES:-$((CPU_BYTES + 4 * (1 << 30)))}"
DISK_DIR_HOST="${DISK_DIR_HOST:-/mnt/certus1/kv-fs-tier}"   # host-side fs tier
DISK_DIR_CTR="/workspace/kv-fs-tier"              # container mount point
DISK_READ_THREADS="${DISK_READ_THREADS:-16}"
DISK_WRITE_THREADS="${DISK_WRITE_THREADS:-16}"
DRIVER="${DRIVER:-${SCRIPT_DIR}/run_multiturn_offloading.py}"
LOG="${LOG:-${SCRIPT_DIR}/cputier_offload_$(stamp).log}"

require_image "$IMAGE"
[[ -f "$DRIVER" ]] || { echo "error: driver not found at ${DRIVER}" >&2; exit 1; }
mkdir -p "$DISK_DIR_HOST"

# The driver was refactored to import sibling modules (run_multiturn_common,
# run_multiturn_sync_batched, and run_multiturn_async for the async path). The
# baked images ship only run_multiturn_offloading.py under /workspace/bench, so
# bind-mount every helper that exists next to the driver — otherwise the mounted
# (newer) driver dies with ModuleNotFoundError.
HELPER_MOUNTS=()
for _h in run_multiturn_common.py run_multiturn_sync_batched.py run_multiturn_async.py; do
  [[ -f "${SCRIPT_DIR}/${_h}" ]] && HELPER_MOUNTS+=( -v "${SCRIPT_DIR}/${_h}:/workspace/bench/${_h}:z" )
done

run_container "$LOG" "$IMAGE" \
  --shm-size "${SHM_BYTES}" \
  -e "CPU_BYTES=${CPU_BYTES}" \
  -e "DISK_DIR=${DISK_DIR_CTR}" \
  -e "DISK_READ_THREADS=${DISK_READ_THREADS}" \
  -e "DISK_WRITE_THREADS=${DISK_WRITE_THREADS}" \
  -v "${DRIVER}:/workspace/bench/run_multiturn_offloading.py:z" \
  "${HELPER_MOUNTS[@]}" \
  -v "${DISK_DIR_HOST}:${DISK_DIR_CTR}:z"
