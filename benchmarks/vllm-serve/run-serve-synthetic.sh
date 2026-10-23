#!/bin/bash
# run-serve-synthetic.sh — SERVER side of the synthetic multi-turn KV-offload test.
#
# Serves Qwen2.5-14B behind the Certus-SHMQ offload connector for the concurrent
# synthetic_text workload driven by run-guidellm-synthetic.sh (512-token prompts,
# 128-token replies, 30-turn conversations, 32 in-flight streams). This is the
# "shared-prefix / growing-history" access pattern that exercises the offload read
# path without needing a large trace file.
#
# Why these values:
#   MAX_MODEL_LEN=32768   the 30-turn conversation resends its history each turn
#                         (server_history=False), so context grows to ~19K tokens by
#                         the final turn — past a 16K window (late turns 400). 32768
#                         is Qwen2.5's NATIVE window, so no YaRN rope-scaling, and a
#                         single sequence's KV (~6G) plus the 14B weights (~29G) fit
#                         one 40G A100 at TP=1. Bump MAX_MODEL_LEN / TENSOR_PARALLEL
#                         if you lengthen the conversation or prompts.
#   GPU_MEM_UTIL=0.9      headroom for activations + the 32 concurrent streams'
#                         working-set KV (the offload connector spills the rest).
#
# Prereqs are identical to run-serve-certus-shmq.sh (host certus-server publishing
# the mailbox, shmq image in the /mnt/certus1 store). All values are env-overridable.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct}" \
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-14b}" \
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}" \
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}" \
GPU="${GPU:-all}" \
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}" \
  exec "${SCRIPT_DIR}/run-serve-certus-shmq.sh"
