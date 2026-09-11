#!/bin/bash
# kv-offload-otel-replay on the OffloadingConnector family (image
# certus-otel-offload-bench). ONE script, three backends selected by env — the
# OTel-corpus counterpart of run-docker-none.sh / run-docker-cpu.sh /
# run-docker-cputier.sh:
#
#   # CPUOffload (default): OffloadingConnector -> pinned host RAM
#   ./run-docker-otel-offload.sh
#   CPU_BYTES=$((32*(1<<30))) ./run-docker-otel-offload.sh
#
#   # NoOffload baseline: no kv_transfer_config
#   OFFLOAD_MODE=none ./run-docker-otel-offload.sh
#
#   # Tiered CPU+FS: CPU primary (host RAM) + fs disk secondary
#   SECONDARY_TIER=fs CPU_BYTES=$((8*(1<<30))) DISK_DIR_HOST=/mnt/certus1/kv-fs-tier \
#       ./run-docker-otel-offload.sh
#
#   # smoke test: 8 conversations, saturating (skip the recorded waits)
#   NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-offload.sh
#
# OTEL_HOST (host corpus dir), NUM_CONVS, TIME_SCALE, MODEL, etc. are handled by
# run-docker-common-otel.sh. The corpus is bind-mounted read-only there.
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common-otel.sh"

IMAGE="${IMAGE:-certus-otel-offload-bench}"
OFFLOAD_MODE="$(printf '%s' "${OFFLOAD_MODE:-}" | tr '[:upper:]' '[:lower:]')"
SECONDARY_TIER="$(printf '%s' "${SECONDARY_TIER:-}" | tr '[:upper:]' '[:lower:]')"
LOG="${LOG:-${SCRIPT_DIR}/otel_offload_$(stamp).log}"

require_image "$IMAGE"

# ── Optional Prometheus exporter (gated on PROM_PORT) ─────────────────────────
# The OTel driver already opens the exporter itself (run_otel_async calls
# common.start_prom_exporter() after building the engine — it is BAKED into the
# image), so exposing metrics needs no rebuild: just set PROM_PORT + LOG_STATS
# and publish the port. run-docker-otel-offload-prom.sh is a thin wrapper that
# sets these. Appended to COMMON_RUN_ARGS so it applies to all three modes below.
#   Scrape from the host at:  http://127.0.0.1:${PROM_PORT}/metrics
#   (podman publishes IPv4 only — use 127.0.0.1, not localhost/::1.)
# Optional DRIVER_SRC=<dir of run_otel_replay.py> bind-mounts the repo driver set
# over the baked copies so driver edits take effect without a rebuild.
PROM_ARGS=()
if [[ -n "${PROM_PORT:-}" ]]; then
  LOG_STATS="${LOG_STATS:-1}"     # 1 = register vLLM metrics (empty /metrics otherwise)
  PROM_ARGS=(-p "${PROM_PORT}:${PROM_PORT}" -e "PROM_PORT=${PROM_PORT}" -e "LOG_STATS=${LOG_STATS}")
  if [[ -n "${DRIVER_SRC:-}" ]]; then
    for f in run_otel_replay.py run_otel_async.py otel_corpus.py \
             run_multiturn_common.py run_multiturn_async.py; do
      [[ -f "${DRIVER_SRC}/${f}" ]] && PROM_ARGS+=(-v "${DRIVER_SRC}/${f}:/workspace/bench/${f}:z")
    done
  fi
  COMMON_RUN_ARGS+=("${PROM_ARGS[@]}")
  echo "[otel-offload] prometheus exporter -> http://127.0.0.1:${PROM_PORT}/metrics"
fi

# ── NoOffload: GPU-only baseline, nothing extra to wire ───────────────────────
if [ "$OFFLOAD_MODE" = "none" ]; then
  run_container "$LOG" "$IMAGE" -e "OFFLOAD_MODE=none"
  exit $?
fi

# ── Tiered CPU+FS: needs --shm-size (CPU tier is a /dev/shm mmap) + fs bind ────
if [ "$SECONDARY_TIER" = "fs" ] || [ -n "${DISK_DIR:-}" ]; then
  CPU_BYTES="${CPU_BYTES:-$((8 * (1 << 30)))}"          # CPU primary tier (bytes)
  # TieringOffloadingSpec allocates the CPU tier as a /dev/shm mmap and force-
  # populates it (MADV_POPULATE_WRITE). Podman's default /dev/shm is 64M -> EFAULT.
  # Size it to the CPU tier plus headroom (the region is padded past cpu_bytes).
  SHM_BYTES="${SHM_BYTES:-$((CPU_BYTES + 4 * (1 << 30)))}"
  DISK_DIR_HOST="${DISK_DIR_HOST:-/mnt/certus1/kv-fs-tier}"   # host-side fs tier
  DISK_DIR_CTR="/workspace/kv-fs-tier"                        # container mount
  DISK_READ_THREADS="${DISK_READ_THREADS:-16}"
  DISK_WRITE_THREADS="${DISK_WRITE_THREADS:-16}"
  mkdir -p "$DISK_DIR_HOST"
  run_container "$LOG" "$IMAGE" \
    --shm-size "${SHM_BYTES}" \
    -e "SECONDARY_TIER=fs" \
    -e "CPU_BYTES=${CPU_BYTES}" \
    -e "DISK_DIR=${DISK_DIR_CTR}" \
    -e "DISK_READ_THREADS=${DISK_READ_THREADS}" \
    -e "DISK_WRITE_THREADS=${DISK_WRITE_THREADS}" \
    -v "${DISK_DIR_HOST}:${DISK_DIR_CTR}:z"
  exit $?
fi

# ── CPUOffload (default): pinned host RAM tier ────────────────────────────────
CPU_BYTES="${CPU_BYTES:-$((16 * (1 << 30)))}"
run_container "$LOG" "$IMAGE" -e "CPU_BYTES=${CPU_BYTES}"
