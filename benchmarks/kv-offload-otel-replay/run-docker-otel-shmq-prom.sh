#!/bin/bash
# kv-offload-otel-replay on Certus-SHMQ + Prometheus — run-docker-otel-shmq.sh
# with the client vLLM engine exposing Prometheus metrics on port 8000.
#
# Like run-docker-otel-shmq.sh this is CLIENT-ONLY: a host certus-server must
# already be running and have published the mailbox at SHM_PATH (see that
# script's header for how to start it). This wrapper only adds the exporter knobs
# the base script gates on:
#   * PROM_PORT=8000 — the driver calls start_http_server(8000)
#   * LOG_STATS=1    — so vLLM registers its PrometheusStatLogger metrics
# and the base script publishes -p 8000:8000. No rebuild needed.
#
# NOTE: these are the CLIENT-side vLLM + KV-offload metrics. The SPDK/SSD-side
# counters live in the host certus-server and are NOT in this registry.
#
# Scrape from the host at:  http://127.0.0.1:8000/metrics
# (podman publishes IPv4 only — use 127.0.0.1, not localhost/::1.)
#
#   ./run-docker-otel-shmq-prom.sh
#   PROM_PORT=9100 NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-shmq-prom.sh
#
# SHM_PATH / SLAB_SIZE_BYTES / WAIT_SECS / NUM_CONVS / TIME_SCALE / ... pass
# straight through. DRIVER_SRC=<repo root> optionally re-mounts the repo driver +
# shared modules over the baked copies (no rebuild).
export PROM_PORT="${PROM_PORT:-8000}"
export LOG_STATS="${LOG_STATS:-1}"
exec "$(dirname "${BASH_SOURCE[0]}")/run-docker-otel-shmq.sh" "$@"
