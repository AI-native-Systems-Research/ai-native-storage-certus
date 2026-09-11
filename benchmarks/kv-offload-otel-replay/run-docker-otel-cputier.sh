#!/bin/bash
# kv-offload-otel-replay — CPU+Disk tiered offload ("cputier"): vLLM 0.26 native
# OffloadingConnector -> TieringOffloadingSpec, a CPU primary tier (pinned host
# RAM) backed by an "fs" disk secondary tier. This is the OTel-corpus counterpart
# of ../kv-offload-replay/run-docker-cputier.sh and the dedicated "cputier" arm
# of the OTel head-to-head (vs. NoOffload / CPUOffload / Certus-shmq).
#
# Thin wrapper over run-docker-otel-offload.sh: it only pins SECONDARY_TIER=fs
# (the tiered path) and a cputier-flavored log name. All the tiering mechanics —
# --shm-size for the /dev/shm CPU-tier mmap, the fs-tier bind mount, the disk IO
# threads — live in the base script's SECONDARY_TIER=fs branch, so there is a
# single implementation to maintain (same reason run-docker-*-prom.sh are thin).
#
#   ./run-docker-otel-cputier.sh
#   CPU_BYTES=$((32*(1<<30))) DISK_DIR_HOST=/mnt/certus1/kv-fs-tier ./run-docker-otel-cputier.sh
#   NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-cputier.sh          # quick smoke
#
# cputier knobs (defaults applied in the base script's tiered branch):
#   CPU_BYTES      CPU primary tier, pinned host RAM (default 8 GiB); must be < free RAM.
#   DISK_DIR_HOST  host dir backing the fs disk tier (default /mnt/certus1/kv-fs-tier);
#                  must be writable by the rootless-mapped container uid, else
#                  stores silently fail.
#   SHM_BYTES      /dev/shm size for the CPU-tier mmap (default CPU_BYTES + 4 GiB) —
#                  the tier is a /dev/shm mmap force-populated with MADV_POPULATE_WRITE,
#                  so podman's default 64M /dev/shm would EFAULT.
#   DISK_READ_THREADS / DISK_WRITE_THREADS   fs-tier IO threads (default 16 each).
# OTEL_HOST / NUM_CONVS / TIME_SCALE / MODEL / MAX_NUM_SEQS / ... pass through
# unchanged (see run-docker-common-otel.sh). The corpus is bind-mounted read-only.
_here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export SECONDARY_TIER="${SECONDARY_TIER:-fs}"
export LOG="${LOG:-${_here}/otel_cputier_$(date +%H%M%S).log}"
exec "${_here}/run-docker-otel-offload.sh" "$@"
