#!/bin/bash
# run-guidellm-cc131k.sh — CLIENT side of the 131K cc-traces THROUGHPUT test.
#
# Drives the vLLM server started by run-serve-cc131k.sh at MAXIMUM throughput, using
# the cc/mooncake trace as the prompt source. Unlike RATE_TYPE=replay, the throughput
# profile ignores the trace's inter-arrival timing and keeps up to MAX_CONCURRENCY
# requests in flight to find the server's saturation point. Prints guidellm's
# throughput/latency tables and snapshots the KV-offload / prefix-cache counters.
#
# The server MUST serve at max_model_len >= the trace's longest input, or overflowing
# rows come back as HTTP 400 (that is why run-serve-cc131k.sh serves the 131072
# window). Bring the server up first.
#
#   ./run-guidellm-cc131k.sh                                  # throughput, 256 max in-flight, 600s
#   MAX_CONCURRENCY=128 ./run-guidellm-cc131k.sh              # cap concurrent in-flight requests
#   MAX_SECONDS=120 ./run-guidellm-cc131k.sh                  # shorter measurement window
#   MAX_REQUESTS=13413 ./run-guidellm-cc131k.sh               # bound by request count instead of time
#   DATA=/mnt/certus1/other-trace.mooncake.jsonl ./run-guidellm-cc131k.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

DATA="${DATA:-/mnt/certus1/cc-traces-weka-062126.mooncake-131k.jsonl}"

# Throughput profile: max_concurrency is REQUIRED (guidellm rejects a bare
# kind=throughput). Bound the run by wall-clock (MAX_SECONDS) unless the caller sets
# MAX_REQUESTS, which run-guidellm.sh gives precedence to.
MODEL="${MODEL:-qwen2.5-14b}" \
PROCESSOR="${PROCESSOR:-Qwen/Qwen2.5-14B-Instruct}" \
PORT="${PORT:-8000}" \
DATA="$DATA" \
RATE_TYPE="${RATE_TYPE:-throughput}" \
MAX_CONCURRENCY="${MAX_CONCURRENCY:-256}" \
TIME_SCALE=0 \
MAX_SECONDS="${MAX_SECONDS:-600}" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
