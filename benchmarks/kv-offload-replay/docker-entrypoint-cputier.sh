#!/bin/bash
# Entrypoint for the native Tiering CPU+FS backend image (certus-cputier-bench).
#
# Self-contained: no external server, no shmq mailbox. It runs the multi-turn workload
# through vLLM 0.26's native multi-tier offload — a CPU (host-RAM) primary tier
# plus an "fs" disk secondary tier rooted at DISK_DIR. So there is nothing to
# wait for; we sanity-check the environment, then exec the driver.
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/bench/run_multiturn_offloading.py}"
DISK_DIR="${DISK_DIR:-/workspace/kv-fs-tier}"
cpu_bytes="${CPU_BYTES:-0}"

# The CPU primary tier of TieringOffloadingSpec is a /dev/shm mmap that is
# force-populated with MADV_POPULATE_WRITE. If /dev/shm is smaller than the CPU
# tier the populate fails with "OSError: [Errno 14] Bad address". Podman's
# default /dev/shm is 64M, so --shm-size >= CPU_BYTES is required. Fail early
# with an actionable message rather than deep in vLLM init.
shm_bytes="$(df -B1 --output=size /dev/shm 2>/dev/null | tail -n1 | tr -d ' ')"
if [ -n "${shm_bytes:-}" ] && [ "${cpu_bytes}" -gt 0 ] && [ "${shm_bytes}" -lt "${cpu_bytes}" ]; then
    echo "[entrypoint] ERROR: /dev/shm is ${shm_bytes} bytes but CPU_BYTES=${cpu_bytes}." >&2
    echo "[entrypoint]        TieringOffloadingSpec mmaps its CPU tier in /dev/shm and will" >&2
    echo "[entrypoint]        fail with 'OSError: [Errno 14] Bad address'. Re-run with" >&2
    echo "[entrypoint]        --shm-size >= ${cpu_bytes} (e.g. --shm-size $(( cpu_bytes / (1<<30) + 4 ))g)." >&2
    exit 1
fi

# The fs disk tier writes block files under DISK_DIR. Bind a host dir onto it to
# hit real disk; without a bind it lands on the container's writable layer.
mkdir -p "${DISK_DIR}"
if ! mountpoint -q "${DISK_DIR}" 2>/dev/null; then
    echo "[entrypoint] WARNING: ${DISK_DIR} is not a bind mount — the fs disk tier will" >&2
    echo "[entrypoint]          write to the container's ephemeral layer (lost on --rm)." >&2
fi

# The CPU tier (CPU_BYTES) is pinned host RAM via /dev/shm; hugepages reserved
# on the host (Certus mode) reduce that budget. Warn if it looks tight.
if [ -r /proc/meminfo ]; then
    avail_bytes=$(( $(awk '/^MemAvailable:/{print $2}' /proc/meminfo) * 1024 ))
    if [ "${cpu_bytes}" -gt 0 ] && [ "${cpu_bytes}" -gt "${avail_bytes}" ]; then
        echo "[entrypoint] WARNING: CPU_BYTES=${cpu_bytes} exceeds MemAvailable=${avail_bytes} — the CPU tier may OOM at init." >&2
    fi
    hp="$(awk '/^HugePages_Total:/{print $2}' /proc/meminfo)"
    if [ -n "${hp:-}" ] && [ "${hp}" -gt 0 ]; then
        echo "[entrypoint] WARNING: ${hp} hugepages reserved on the host — they reduce RAM available to the CPU tier." >&2
    fi
fi

echo "[entrypoint] Tiering CPU+FS run: NUM_CONVS=${NUM_CONVS:-?} MODEL=${MODEL:-?} CPU_BYTES=${cpu_bytes} DISK_DIR=${DISK_DIR}"
exec python3 "${WORKLOAD}"
