#!/bin/bash
# Entrypoint for the CPU-offload backend image.
#
# This backend is self-contained: no external server, no shmq mailbox. It just runs the
# multi-turn workload through vLLM's in-process CPU-offload tier (pinned host
# RAM). So there is nothing to wait for — we only sanity-check the environment,
# then exec the driver.
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/bench/run_multiturn_offloading.py}"

# The offload tier (CPU_BYTES) is a PINNED, unswappable host-RAM buffer — it must
# fit in *available* RAM or vLLM OOMs at init. 1G hugepages (reserved when the
# host is in Certus mode) come out of RAM and shrink that budget. Warn if the
# CPU tier looks larger than free RAM, or if hugepages are reserved.
cpu_bytes="${CPU_BYTES:-0}"
if [ -r /proc/meminfo ]; then
    avail_kb="$(awk '/^MemAvailable:/{print $2}' /proc/meminfo)"
    avail_bytes=$(( avail_kb * 1024 ))
    if [ "${cpu_bytes}" -gt 0 ] && [ "${cpu_bytes}" -gt "${avail_bytes}" ]; then
        echo "[entrypoint] WARNING: CPU_BYTES=${cpu_bytes} exceeds MemAvailable=${avail_bytes} — the pinned offload tier may OOM at init." >&2
    fi
    hp="$(awk '/^HugePages_Total:/{print $2}' /proc/meminfo)"
    if [ -n "${hp:-}" ] && [ "${hp}" -gt 0 ]; then
        echo "[entrypoint] WARNING: ${hp} hugepages reserved on the host — they reduce RAM available to the pinned CPU tier (free them if this host was in Certus mode)." >&2
    fi
fi

echo "[entrypoint] CPU-offload run: NUM_CONVS=${NUM_CONVS:-?} MODEL=${MODEL:-?} CPU_BYTES=${cpu_bytes} DATASET_PATH=${DATASET_PATH:-?}"
exec python3 "${WORKLOAD}"
