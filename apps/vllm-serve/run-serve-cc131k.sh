#!/bin/bash
# run-serve-cc131k.sh — SERVER side of the 131K cc-traces KV-offload replay test.
#
# Serves Qwen2.5-14B behind the Certus-SHMQ offload connector with a context
# window large enough to replay the 128K-context cc/mooncake traces
# (e.g. cc-traces-weka-*.mooncake-131k.jsonl). Pair with run-guidellm-cc131k.sh.
#
# Why these values (learned the hard way — a 16K server 400s ~92% of this trace):
#   MAX_MODEL_LEN=131072  the trace's inputs reach ~131K tokens (median ~88K); a
#                         smaller window rejects most rows with HTTP 400. This
#                         auto-enables Qwen YaRN rope-scaling (factor 131072/32768=4).
#   TENSOR_PARALLEL=2     a single 131K sequence's KV (~24G) plus the 14B weights
#                         (~29G) does NOT fit one 40G A100 — vLLM fails the startup
#                         KV-cache sizing check. Sharding across both A100s puts it
#                         at ~27G/GPU. Requires GPU=all so both devices are visible.
#   GPU_MEM_UTIL=0.9      leave headroom for activations/overhead per GPU.
#
# Prereqs (same as run-serve-certus-shmq.sh): a host certus-server must already be
# publishing the mailbox at SHM_PATH (default /dev/shm/certus-shmq), the shmq image
# must be in the /mnt/certus1 podman store, and both A100s must be free. All values
# below are overridable from the environment.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct}" \
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-14b}" \
MAX_MODEL_LEN="${MAX_MODEL_LEN:-131072}" \
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}" \
GPU="${GPU:-all}" \
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}" \
  exec "${SCRIPT_DIR}/run-serve-certus-shmq.sh"
