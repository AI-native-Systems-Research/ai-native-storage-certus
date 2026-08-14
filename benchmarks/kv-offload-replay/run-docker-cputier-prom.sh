#!/bin/bash
# CPU+Disk offload + Prometheus — same as run-docker-cputier.sh (vLLM 0.26
# native multi-tier: CPU primary + "fs" disk secondary), but with the bench's
# vLLM engine exposing Prometheus metrics on port 8000.
#
# The bench drives vLLM through the offline LLM(...) engine (no OpenAI server),
# so there is no /metrics endpoint unless the driver opens one itself. This
# variant sets three extra knobs the base script does not:
#   * LOG_STATS=1   — so vLLM registers its PrometheusStatLogger metrics
#   * PROM_PORT=8000 — the driver calls start_http_server(8000)
#   * -p 8000:8000   — publish that port on the host
# The driver (run_multiturn_offloading.py) is already bind-mounted from the repo,
# so the exporter block takes effect WITHOUT rebuilding the image.
#
# Scrape from the host at:  http://localhost:8000/metrics
# (Note: podman publishes IPv4 only — use 127.0.0.1, not localhost/::1.)
#
#   ./run-docker-cputier-prom.sh
#   PROM_PORT=9100 CPU_BYTES=$((8*(1<<30))) ./run-docker-cputier-prom.sh
#
# CPU_BYTES     = CPU primary tier (pinned host RAM); must be < free RAM.
# DISK_DIR_HOST = host directory backing the fs disk tier (must be writable by
#                 the rootless-mapped container uid, else stores silently fail).
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-cpu-offload-bench}"
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
LOG="${LOG:-${SCRIPT_DIR}/cputier_offload_prom_$(stamp).log}"

# ── Prometheus wiring (the only substantive difference from the base script) ──
PROM_PORT="${PROM_PORT:-8000}"        # host port the exporter is published on
LOG_STATS="${LOG_STATS:-1}"           # 1 = register vLLM metrics (empty /metrics otherwise)

require_image "$IMAGE"
[[ -f "$DRIVER" ]] || { echo "error: driver not found at ${DRIVER}" >&2; exit 1; }
mkdir -p "$DISK_DIR_HOST"

echo "[cputier] prometheus exporter -> http://localhost:${PROM_PORT}/metrics (use 127.0.0.1, not localhost)"

# run_container forwards these extra args straight to `podman run`, so the port
# publish + PROM_PORT/LOG_STATS env need no change to run-docker-common.sh.
run_container "$LOG" "$IMAGE" \
  --shm-size "${SHM_BYTES}" \
  -p "${PROM_PORT}:${PROM_PORT}" \
  -e "PROM_PORT=${PROM_PORT}" \
  -e "LOG_STATS=${LOG_STATS}" \
  -e "CPU_BYTES=${CPU_BYTES}" \
  -e "DISK_DIR=${DISK_DIR_CTR}" \
  -e "DISK_READ_THREADS=${DISK_READ_THREADS}" \
  -e "DISK_WRITE_THREADS=${DISK_WRITE_THREADS}" \
  -v "${DRIVER}:/workspace/bench/run_multiturn_offloading.py:z" \
  -v "${DISK_DIR_HOST}:${DISK_DIR_CTR}:z"
