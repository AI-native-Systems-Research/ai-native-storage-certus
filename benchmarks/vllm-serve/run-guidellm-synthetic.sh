#!/bin/bash
# run-guidellm-synthetic.sh — CLIENT side of the synthetic multi-turn KV-offload test.
#
# Drives the vLLM server started by run-serve-synthetic.sh with a CLOSED-LOOP
# concurrent load: CONCURRENT_STREAMS fixed in-flight streams, each a 30-turn
# synthetic_text conversation (512-token prompts, 128-token replies), for MAX_SECONDS.
# This is the workload previously wired up in run-client-1.sh; here it is a documented,
# env-overridable wrapper paired with its serve script.
#
# The server must serve a window large enough for the accumulated 30-turn history
# (~19K tokens) — run-serve-synthetic.sh serves 32768 for exactly that reason.
#
#   ./run-guidellm-synthetic.sh                               # 32 streams, 600s
#   CONCURRENT_STREAMS=64 ./run-guidellm-synthetic.sh         # heavier closed-loop
#   MAX_SECONDS=120 ./run-guidellm-synthetic.sh               # short smoke run
#   DATA="kind=synthetic_text,prompt_tokens=1024,output_tokens=256,turns=10" ./run-guidellm-synthetic.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-qwen2.5-14b}" \
PROCESSOR="${PROCESSOR:-Qwen/Qwen2.5-14B-Instruct}" \
PORT="${PORT:-8000}" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-32}" \
MAX_SECONDS="${MAX_SECONDS:-600}" \
DATA="${DATA:-kind=synthetic_text,prompt_tokens=512,output_tokens=128,turns=30}" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
