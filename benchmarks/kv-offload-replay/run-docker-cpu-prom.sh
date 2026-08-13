#!/bin/bash
# CPUOffload + Prometheus — vLLM OffloadingConnector -> host RAM (image
# certus-cpu-offload-bench) with the bench's vLLM engine exposing Prometheus
# metrics on port 8000.
#
# The bench drives vLLM through the offline LLM(...) engine (no OpenAI server),
# so there is no /metrics endpoint unless the driver opens one. This variant
# sets LOG_STATS=1 + PROM_PORT=8000, publishes -p 8000:8000, and bind-mounts the
# repo driver over the baked one so the exporter works WITHOUT rebuilding.
#
# Scrape from the host at:  http://127.0.0.1:8000/metrics  (IPv4, not localhost)
#
#   ./run-docker-cpu-prom.sh
#   PROM_PORT=9100 CPU_BYTES=$((32*(1<<30))) ./run-docker-cpu-prom.sh
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-cpu-offload-bench}"
CPU_BYTES="${CPU_BYTES:-$((16 * (1 << 30)))}"   # host-RAM KV pool (bytes)
LOG="${LOG:-${SCRIPT_DIR}/cpu_offload_prom_$(stamp).log}"
DRIVER="${DRIVER:-${SCRIPT_DIR}/run_multiturn_offloading.py}"

# ── Prometheus wiring (the only substantive difference from the base script) ──
PROM_PORT="${PROM_PORT:-8000}"        # host port the exporter is published on
LOG_STATS="${LOG_STATS:-1}"           # 1 = register vLLM metrics (empty /metrics otherwise)

require_image "$IMAGE"
[[ -f "$DRIVER" ]] || { echo "error: driver not found at ${DRIVER}" >&2; exit 1; }

echo "[cpu-offload] prometheus exporter -> http://127.0.0.1:${PROM_PORT}/metrics"
run_container "$LOG" "$IMAGE" \
  -p "${PROM_PORT}:${PROM_PORT}" \
  -e "PROM_PORT=${PROM_PORT}" \
  -e "LOG_STATS=${LOG_STATS}" \
  -e "CPU_BYTES=${CPU_BYTES}" \
  -v "${DRIVER}:/workspace/bench/run_multiturn_offloading.py:z"
