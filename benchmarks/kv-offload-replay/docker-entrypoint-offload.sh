#!/bin/bash
# Unified entrypoint for the KV-offload workload image (certus-offload-bench).
#
# One image, one driver (run_multiturn_offloading.py), three backends selected by
# environment — this merges the former nooffload / cpu-offload / cputier
# entrypoints:
#
#   OFFLOAD_MODE=none               GPU-only baseline (no kv_transfer_config).
#                                   Nothing to size, nothing to check.
#   OFFLOAD_MODE unset/other,       CPU-offload (host-RAM tier). Sanity-check that
#     SECONDARY_TIER empty          the pinned CPU_BYTES tier fits in RAM.
#   SECONDARY_TIER=fs (or DISK_DIR) Tiered CPU primary + fs secondary. Additionally
#                                   check /dev/shm (the CPU tier is an shm mmap) and
#                                   warn if the fs tier dir is not a bind mount.
#
# All three are self-contained: no external server, no gRPC, no CUDA IPC. There is
# nothing to wait for — we validate the environment for the selected mode, then
# exec the driver.
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/bench/run_multiturn_offloading.py}"
OFFLOAD_MODE="$(printf '%s' "${OFFLOAD_MODE:-}" | tr '[:upper:]' '[:lower:]')"
cpu_bytes="${CPU_BYTES:-0}"

# ── NoOffload: nothing to size or check ──────────────────────────────────────
if [ "${OFFLOAD_MODE}" = "none" ]; then
    echo "[entrypoint] NO-OFFLOAD run: NUM_CONVS=${NUM_CONVS:-?} MODEL=${MODEL:-?} DATASET_PATH=${DATASET_PATH:-?}"
    exec python3 "${WORKLOAD}"
fi

# ── Tiered (CPU primary + fs secondary): the CPU tier is a /dev/shm mmap ──────
# It is force-populated with MADV_POPULATE_WRITE; if /dev/shm is smaller than the
# CPU tier the populate fails with "OSError: [Errno 14] Bad address". Podman's
# default /dev/shm is 64M, so --shm-size >= CPU_BYTES is required. Fail early with
# an actionable message rather than deep in vLLM init.
is_tiered=0
if [ -n "${SECONDARY_TIER:-}" ] || [ -n "${DISK_DIR:-}" ]; then
    is_tiered=1
fi

if [ "${is_tiered}" -eq 1 ]; then
    shm_bytes="$(df -B1 --output=size /dev/shm 2>/dev/null | tail -n1 | tr -d ' ')"
    if [ -n "${shm_bytes:-}" ] && [ "${cpu_bytes}" -gt 0 ] && [ "${shm_bytes}" -lt "${cpu_bytes}" ]; then
        echo "[entrypoint] ERROR: /dev/shm is ${shm_bytes} bytes but CPU_BYTES=${cpu_bytes}." >&2
        echo "[entrypoint]        The tiering CPU tier mmaps in /dev/shm and will fail with" >&2
        echo "[entrypoint]        'OSError: [Errno 14] Bad address'. Re-run with" >&2
        echo "[entrypoint]        --shm-size >= ${cpu_bytes} (e.g. --shm-size $(( cpu_bytes / (1<<30) + 4 ))g)." >&2
        exit 1
    fi
    # The fs disk tier writes block files under DISK_DIR / FS_ROOT_DIR. Bind a host
    # dir onto it to hit real disk; without a bind it lands on the ephemeral layer.
    fs_dir="${DISK_DIR:-${FS_ROOT_DIR:-}}"
    if [ -n "${fs_dir}" ]; then
        mkdir -p "${fs_dir}"
        if ! mountpoint -q "${fs_dir}" 2>/dev/null; then
            echo "[entrypoint] WARNING: ${fs_dir} is not a bind mount — the fs disk tier will" >&2
            echo "[entrypoint]          write to the container's ephemeral layer (lost on --rm)." >&2
        fi
    fi
fi

# ── CPU / Tiered shared RAM sanity: the CPU tier is pinned host RAM ───────────
# (a CUDA pinned buffer for CPU-only, /dev/shm for tiered). 1G hugepages reserved
# on the host (Certus mode) come out of RAM and shrink that budget.
if [ -r /proc/meminfo ]; then
    avail_bytes=$(( $(awk '/^MemAvailable:/{print $2}' /proc/meminfo) * 1024 ))
    if [ "${cpu_bytes}" -gt 0 ] && [ "${cpu_bytes}" -gt "${avail_bytes}" ]; then
        echo "[entrypoint] WARNING: CPU_BYTES=${cpu_bytes} exceeds MemAvailable=${avail_bytes} — the pinned CPU tier may OOM at init." >&2
    fi
    hp="$(awk '/^HugePages_Total:/{print $2}' /proc/meminfo)"
    if [ -n "${hp:-}" ] && [ "${hp}" -gt 0 ]; then
        echo "[entrypoint] WARNING: ${hp} hugepages reserved on the host — they reduce RAM available to the CPU tier (free them if this host was in Certus mode)." >&2
    fi
fi

if [ "${is_tiered}" -eq 1 ]; then
    echo "[entrypoint] Tiering CPU+FS run: NUM_CONVS=${NUM_CONVS:-?} MODEL=${MODEL:-?} CPU_BYTES=${cpu_bytes} fs_dir=${DISK_DIR:-${FS_ROOT_DIR:-?}}"
else
    echo "[entrypoint] CPU-offload run: NUM_CONVS=${NUM_CONVS:-?} MODEL=${MODEL:-?} CPU_BYTES=${cpu_bytes} DATASET_PATH=${DATASET_PATH:-?}"
fi
exec python3 "${WORKLOAD}"
