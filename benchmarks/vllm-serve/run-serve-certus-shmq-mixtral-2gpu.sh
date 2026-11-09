#!/bin/bash
# run-serve-certus-shmq-mixtral-2gpu.sh — SERVER side of the Mixtral-8x7B (MoE)
# KV-offload test on the Certus-SHMQ backend, sharded across BOTH A100s.
# TEXT-ONLY (Mixtral is not multimodal).
#
# Serves Mixtral-8x7B-Instruct behind the Certus-SHMQ offload connector, ready for
# guidellm to drive a synthetic multi-turn text workload against
# /v1/chat/completions. This is a thin wrapper over run-serve-certus-shmq.sh — it
# only sets the model/parallelism/window env and inherits all the connector +
# container plumbing (SHM_PATH, image, store, fix#3 scheduler patch, port
# publishing) from that base script. Unlike the Qwen3-VL wrappers there is NO
# --limit-mm-per-prompt (EXTRA_SERVE_ARGS unset) — Mixtral takes text only.
#
# Why these values (kept in lockstep with the cputier wrapper for comparability):
#   MODEL=RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8
#                         Mixtral-8x7B is a sparse MoE (8 experts, top-2; 46.7B
#                         total, ~12.9B active/token). bf16 weights (~93G) do NOT
#                         fit 2x40G A100; the RedHatAI FP8 checkpoint
#                         (compressed-tensors, ~47G -> ~23.5G/GPU at TP=2) does,
#                         with real KV headroom. Ungated re-upload of
#                         mistralai/Mixtral-8x7B-Instruct-v0.1. Override MODEL= +
#                         DTYPE= for a different checkpoint.
#   DTYPE=auto            read the compressed-tensors FP8 config from the
#                         checkpoint (do NOT force float16).
#   TENSOR_PARALLEL=2     shard the experts + attention across both A100s.
#                         Requires GPU=all so both devices are visible.
#   MAX_MODEL_LEN=32768   Mixtral's native window; the synthetic multi-turn
#                         workload accumulates ~19K tokens, which fits. (No YaRN:
#                         the base only rope-scales qwen2, and this is native.)
#   GPU_MEM_UTIL=0.85     text-only, so no vision-activation spike to reserve HBM
#                         for; ~23.5G weights/GPU + this cap leaves ~8G/GPU KV pool.
#                         The offload connector spills reused prefixes to the SHMQ
#                         tier.
#
# Prereqs are identical to run-serve-certus-shmq.sh (host certus-server publishing
# the mailbox at SHM_PATH, shmq image in the /mnt/certus1 podman store, both A100s
# free) plus the Mixtral FP8 checkpoint reachable (HF cache on /mnt/certus1). All
# values below are env-overridable. Pair with the text guidellm driver once up.
#
#   ./run-serve-certus-shmq-mixtral-2gpu.sh
#   PORT=9000 ./run-serve-certus-shmq-mixtral-2gpu.sh
#   GPU_MEM_UTIL=0.80 ./run-serve-certus-shmq-mixtral-2gpu.sh   # more conservative HBM
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8}" \
DTYPE="${DTYPE:-auto}" \
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-mixtral-8x7b}" \
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}" \
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}" \
GPU="${GPU:-all}" \
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.85}" \
  exec "${SCRIPT_DIR}/run-serve-certus-shmq.sh"
