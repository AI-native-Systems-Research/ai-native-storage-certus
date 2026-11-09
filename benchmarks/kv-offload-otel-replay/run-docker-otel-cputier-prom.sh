#!/bin/bash
# kv-offload-otel-replay CPU+Disk tiered ("cputier") + Prometheus.
# = run-docker-otel-cputier.sh with the client vLLM engine exposing Prometheus
# metrics. The bench drives vLLM through the offline LLM(...) engine (no OpenAI
# server), so there is no /metrics endpoint unless the driver opens one — and the
# OTel driver already does (run_otel_async calls common.start_prom_exporter(),
# BAKED into the image). So exposing metrics needs no rebuild: this wrapper only
# sets the two knobs the base script gates on, and the base script publishes the
# port:
#   * PROM_PORT=8000 — driver calls start_http_server(PROM_PORT)
#   * LOG_STATS=1    — so vLLM registers its PrometheusStatLogger metrics
#                      (an empty /metrics otherwise)
#
# Scrape from the host at:  http://127.0.0.1:8000/metrics
# (podman publishes IPv4 only — use 127.0.0.1, not localhost/::1.) These are the
# client-side vLLM + KV-offload (CPU + fs-tier) metrics for the tiered arm.
#
#   ./run-docker-otel-cputier-prom.sh
#   PROM_PORT=9100 NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-cputier-prom.sh
#   CPU_BYTES=$((32*(1<<30))) DISK_DIR_HOST=/mnt/certus1/kv-fs-tier ./run-docker-otel-cputier-prom.sh
#
# All cputier knobs (CPU_BYTES / DISK_DIR_HOST / SHM_BYTES / DISK_*_THREADS /
# NUM_CONVS / TIME_SCALE / ...) pass straight through. DRIVER_SRC=<dir of
# run_otel_replay.py> optionally re-mounts the repo driver set (no rebuild).
export PROM_PORT="${PROM_PORT:-8000}"
export LOG_STATS="${LOG_STATS:-1}"
exec "$(dirname "${BASH_SOURCE[0]}")/run-docker-otel-cputier.sh" "$@"
