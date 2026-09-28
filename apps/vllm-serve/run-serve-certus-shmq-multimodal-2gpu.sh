#!/bin/bash
# run-serve-certus-shmq-multimodal-2gpu.sh — SERVER side of the Qwen3-VL-32B
# multimodal KV-offload test, sharded across BOTH A100s.
#
# Serves Qwen3-VL-32B-Instruct behind the Certus-SHMQ offload connector with
# multimodal (image) input enabled, ready for guidellm to drive a synthetic
# multi-modal workload against /v1/chat/completions. This is a thin wrapper over
# run-serve-certus-shmq.sh — it only sets the model/parallelism/multimodal env
# and inherits all the connector + container plumbing (SHM_PATH, image, store,
# fix#3 scheduler patch, port publishing) from that base script.
#
# Why these values:
#   MODEL=Qwen/Qwen3-VL-32B-Instruct
#                         the 32B vision-language model. NOTE: fp16 weights are
#                         ~64G, which does NOT fit 2x40G A100 once you add KV cache
#                         + activations — expect an OOM / KV-sizing abort at
#                         startup with the default DTYPE=float16. If it aborts,
#                         serve an FP8/AWQ checkpoint instead (set MODEL= and
#                         DTYPE=auto), or lower MAX_MODEL_LEN / GPU count.
#   TENSOR_PARALLEL=2     shard the 32B weights across both A100s (~half each).
#                         Requires GPU=all so both devices are visible.
#   GPU=all               expose both A100s to the container.
#   MAX_MODEL_LEN=32768   modest window to keep KV pressure down given the large
#                         weights; a single image already costs many tokens.
#                         Qwen3-VL's native window is far larger (256K) so no YaRN
#                         rope-scaling is needed — bump this if prompts/history or
#                         image counts grow (watch GPU memory).
#   GPU_MEM_UTIL=0.92     squeeze a little more HBM for weights + KV; the offload
#                         connector spills reused prefixes to the SHMQ tier.
#
# Multimodal: --limit-mm-per-prompt allows up to 2 images per prompt (matches the
# synthetic MM workload). Passed through the base script's EXTRA_SERVE_ARGS hook.
# The JSON is compact (no spaces) so it survives the base script's word-split.
#
# Prereqs are identical to run-serve-certus-shmq.sh (host certus-server publishing
# the mailbox at SHM_PATH, shmq image in the /mnt/certus1 podman store, both A100s
# free). All values below are env-overridable. Pair with the multimodal guidellm
# driver once the server is up.
#
#   ./run-serve-certus-shmq-multimodal-2gpu.sh
#   PORT=9000 ./run-serve-certus-shmq-multimodal-2gpu.sh
#   MODEL=Qwen/Qwen3-VL-32B-Instruct-FP8 DTYPE=auto ./run-serve-certus-shmq-multimodal-2gpu.sh
#   MM_IMAGES=1 ./run-serve-certus-shmq-multimodal-2gpu.sh      # cap images/prompt
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Images allowed per prompt (compact JSON — no spaces — for the base word-split).
MM_IMAGES="${MM_IMAGES:-2}"
MM_ARGS="--limit-mm-per-prompt {\"image\":${MM_IMAGES}}"

MODEL="${MODEL:-Qwen/Qwen3-VL-32B-Instruct-FP8}" \
DTYPE="auto" \
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen3-vl-32b}" \
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}" \
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}" \
GPU="${GPU:-all}" \
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.80}" \
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:-${MM_ARGS}}" \
  exec "${SCRIPT_DIR}/run-serve-certus-shmq.sh"
