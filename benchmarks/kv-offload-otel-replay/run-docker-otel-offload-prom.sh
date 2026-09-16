#!/bin/bash
# kv-offload-otel-replay (offload family) + Prometheus — run-docker-otel-offload.sh
# with the client vLLM engine exposing Prometheus metrics on port 8000.
#
# The bench drives vLLM through the offline LLM(...) engine (no OpenAI server),
# so there is no /metrics endpoint unless the driver opens one. The OTel driver
# already does (run_otel_async calls common.start_prom_exporter(), baked into the
# image), so this wrapper only sets the two knobs the base script gates on:
#   * PROM_PORT=8000 — the driver calls start_http_server(8000)
#   * LOG_STATS=1    — so vLLM registers its PrometheusStatLogger metrics
# and the base script publishes -p 8000:8000. No rebuild needed.
#
# Scrape from the host at:  http://127.0.0.1:8000/metrics
# (podman publishes IPv4 only — use 127.0.0.1, not localhost/::1.)
#
#   ./run-docker-otel-offload-prom.sh                       # CPUOffload + prom
#   OFFLOAD_MODE=none ./run-docker-otel-offload-prom.sh      # NoOffload + prom
#   SECONDARY_TIER=fs CPU_BYTES=$((8*(1<<30))) ./run-docker-otel-offload-prom.sh
#   PROM_PORT=9100 NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-offload-prom.sh
#
# All base-script knobs (OFFLOAD_MODE / SECONDARY_TIER / CPU_BYTES / DISK_DIR_HOST
# / NUM_CONVS / TIME_SCALE / ... ) pass straight through. DRIVER_SRC=<dir of
# run_otel_replay.py> optionally re-mounts the repo driver set (no rebuild).
export PROM_PORT="${PROM_PORT:-8000}"
export LOG_STATS="${LOG_STATS:-1}"
exec "$(dirname "${BASH_SOURCE[0]}")/run-docker-otel-offload.sh" "$@"
