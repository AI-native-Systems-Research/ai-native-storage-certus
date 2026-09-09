#!/bin/bash
# Entrypoint for the kv-offload-otel-replay OffloadingConnector image
# (certus-otel-offload-bench).
#
# One image, one driver (run_otel_replay.py), three backends selected by
# environment — the OTel-corpus counterpart of docker-entrypoint-offload.sh:
#
#   OFFLOAD_MODE=none               GPU-only baseline (no kv_transfer_config).
#                                   Nothing to size, nothing to check.
#   OFFLOAD_MODE unset/other,       CPU-offload (host-RAM tier). Sanity-check that
#     SECONDARY_TIER empty          the pinned CPU_BYTES tier fits in RAM.
#   SECONDARY_TIER=fs (or DISK_DIR) Tiered CPU primary + fs secondary. Additionally
#                                   check /dev/shm (the CPU tier is an shm mmap) and
#                                   warn if the fs tier dir is not a bind mount.
#
# Unlike the ShareGPT image, the workload corpus is NOT baked in: it is a ~22 GB
# bind mount on OTEL_DIR. We validate that mount first (fail fast with the -v
# hint), then run the same per-mode sanity checks, then exec the driver.
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/bench/run_otel_replay.py}"
OFFLOAD_MODE="$(printf '%s' "${OFFLOAD_MODE:-}" | tr '[:upper:]' '[:lower:]')"
cpu_bytes="${CPU_BYTES:-0}"
OTEL_DIR="${OTEL_DIR:-/workspace/otel-corpus}"

# ── Corpus bind-mount check (OTel-specific; corpus is not baked in) ───────────
# The driver globs OTEL_DIR/*.json; an empty/absent dir means the bind mount is
# missing. Fail here with an actionable message rather than exiting 1 deep in the
# loader after model load.
if [ ! -d "${OTEL_DIR}" ] || [ -z "$(ls -A "${OTEL_DIR}" 2>/dev/null)" ]; then
    echo "[entrypoint] ERROR: OTEL_DIR=${OTEL_DIR} is empty or missing." >&2
    echo "[entrypoint]        The OTel corpus is NOT baked into this image (~22 GB)." >&2
    echo "[entrypoint]        Bind-mount it read-only, e.g.:" >&2
    echo "[entrypoint]          -v /mnt/certus1/inference-perf-syn-data/otel_1k:${OTEL_DIR}:ro" >&2
    exit 1
fi
n_files="$(ls -1 "${OTEL_DIR}"/*.json 2>/dev/null | wc -l | tr -d ' ')"
echo "[entrypoint] corpus ${OTEL_DIR}: ${n_files} json files  (NUM_CONVS=${NUM_CONVS:-ALL} TIME_SCALE=${TIME_SCALE:-1.0})"

# ── NoOffload: nothing to size or check ──────────────────────────────────────
if [ "${OFFLOAD_MODE}" = "none" ]; then
    echo "[entrypoint] NO-OFFLOAD run: MODEL=${MODEL:-?} OTEL_DIR=${OTEL_DIR}"
    exec python3 "${WORKLOAD}"
fi

# ── Tiered (CPU primary + fs secondary): the CPU tier is a /dev/shm mmap ──────
# It is force-populated with MADV_POPULATE_WRITE; if /dev/shm is smaller than the
# CPU tier the populate fails with "OSError: [Errno 14] Bad address". Podman's
# default /dev/shm is 64M, so --shm-size >= CPU_BYTES is required. Fail early.
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
    echo "[entrypoint] Tiering CPU+FS run: MODEL=${MODEL:-?} CPU_BYTES=${cpu_bytes} fs_dir=${DISK_DIR:-${FS_ROOT_DIR:-?}} OTEL_DIR=${OTEL_DIR}"
else
    echo "[entrypoint] CPU-offload run: MODEL=${MODEL:-?} CPU_BYTES=${cpu_bytes} OTEL_DIR=${OTEL_DIR}"
fi
exec python3 "${WORKLOAD}"
