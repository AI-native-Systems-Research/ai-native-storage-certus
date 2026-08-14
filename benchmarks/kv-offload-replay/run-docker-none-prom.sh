#!/bin/bash
# NoOffload + Prometheus — GPU-only baseline (image certus-nooffload-bench) with
# the bench's vLLM engine exposing Prometheus metrics on port 8000.
#
# The bench drives vLLM through the offline LLM(...) engine (no OpenAI server),
# so there is no /metrics endpoint unless the driver opens one. This variant
# sets LOG_STATS=1 + PROM_PORT=8000, publishes -p 8000:8000, and bind-mounts the
# repo driver over the baked one so the exporter works WITHOUT rebuilding.
#
# Scrape from the host at:  http://127.0.0.1:8000/metrics  (IPv4, not localhost)
#
#   ./run-docker-none-prom.sh
#   PROM_PORT=9100 NUM_CONVS=100 ./run-docker-none-prom.sh
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-nooffload-bench}"
LOG="${LOG:-${SCRIPT_DIR}/nooffload_prom_$(stamp).log}"
DRIVER="${DRIVER:-${SCRIPT_DIR}/run_multiturn_nooffload.py}"

# ── Prometheus wiring (the only substantive difference from the base script) ──
PROM_PORT="${PROM_PORT:-8000}"        # host port the exporter is published on
LOG_STATS="${LOG_STATS:-1}"           # 1 = register vLLM metrics (empty /metrics otherwise)

require_image "$IMAGE"
[[ -f "$DRIVER" ]] || { echo "error: driver not found at ${DRIVER}" >&2; exit 1; }

echo "[nooffload] prometheus exporter -> http://127.0.0.1:${PROM_PORT}/metrics"
run_container "$LOG" "$IMAGE" \
  -p "${PROM_PORT}:${PROM_PORT}" \
  -e "PROM_PORT=${PROM_PORT}" \
  -e "LOG_STATS=${LOG_STATS}" \
  -v "${DRIVER}:/workspace/bench/run_multiturn_nooffload.py:z"
