#!/bin/bash
# run-guidellm-cc131k.sh — CLIENT side of the 131K cc-traces KV-offload test.
#
# Drives the vLLM server started by run-serve-cc131k.sh using the cc/mooncake trace
# as the prompt source, in CLOSED-LOOP mode: a fixed CONCURRENT_STREAMS requests are
# kept in flight, a new one starting only when one finishes.
#
# Why closed-loop and not replay/throughput: the cc trace is ~94% leading-prefix
# reuse, but its rows are HUGE (prompts up to ~131K tokens ≈ 25 GB KV each) and the
# trace is dense (>11K rows). Replay honors the timestamps but applies NO concurrency
# cap, so thousands of giant requests pile up, none complete (request_iterations=0,
# cancelled at the window end), nothing is ever cached/offloaded, and both prefix and
# external-cache hit rates sit at ~0. Throughput with a high max_concurrency does the
# same. A small fixed stream count lets each large request actually finish, so its
# prefix lands in GPU, evicts, offloads to the tier, and the next same-prefix request
# hits it — which is the behaviour this test exists to measure. Keep CONCURRENT_STREAMS
# low: even a handful of 131K-token contexts can exceed GPU KV capacity.
#
# The server MUST serve at max_model_len >= the trace's longest input, or overflowing
# rows come back as HTTP 400 (that is why run-serve-cc131k.sh serves the 131072
# window). Bring the server up first.
#
#   ./run-guidellm-cc131k.sh                                  # 4 fixed streams, 600s
#   CONCURRENT_STREAMS=8 ./run-guidellm-cc131k.sh             # more in-flight (watch GPU KV)
#   RATE_TYPE=replay TIME_SCALE=1.0 ./run-guidellm-cc131k.sh  # trace-timed arrivals (uncapped — floods)
#   RATE_TYPE=throughput MAX_CONCURRENCY=16 ./run-guidellm-cc131k.sh  # capped saturation test
#   MAX_SECONDS=120 ./run-guidellm-cc131k.sh                  # shorter measurement window
#   MAX_REQUESTS=2000 ./run-guidellm-cc131k.sh                # bound by request count instead of time
#   DATA=/mnt/certus1/other-trace.mooncake.jsonl ./run-guidellm-cc131k.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

DATA="${DATA:-/mnt/certus1/cc-traces-weka-062126.mooncake-131k.jsonl}"

# Default to closed-loop concurrent with a small fixed stream count so the large
# prefix-bearing requests complete and populate the GPU/offload tier (see header).
# Bound the run by wall-clock (MAX_SECONDS) unless the caller sets MAX_REQUESTS,
# which run-guidellm.sh gives precedence to.
MODEL="${MODEL:-qwen2.5-14b}" \
PROCESSOR="${PROCESSOR:-Qwen/Qwen2.5-14B-Instruct}" \
PORT="${PORT:-8000}" \
DATA="$DATA" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-4}" \
MAX_SECONDS="${MAX_SECONDS:-600}" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
