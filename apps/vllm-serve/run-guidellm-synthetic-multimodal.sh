#!/bin/bash
# run-guidellm-synthetic-multimodal.sh — CLIENT side of the synthetic MULTIMODAL
# KV-offload test.
#
# Drives a vLLM server that serves a vision-language model (pair with
# run-serve-certus-shmq-multimodal-2gpu.sh, i.e. Qwen3-VL-32B behind the
# Certus-SHMQ offload connector) with a synthetic text+image workload. Each
# request is a chat completion carrying a text prompt AND a synthetic image, so
# it exercises the VL prefill / multimodal path rather than text-only.
#
# How the multimodal request is assembled: guidellm merges TWO --data sources
# column-wise into one row —
#   DATA        -> kind=synthetic_text  -> "prompt" column (the text)
#   IMAGE_DATA  -> kind=synthetic_image -> "image"  column (a base64 image)
# The openai_http backend defaults to /v1/chat/completions and renders the image
# column as an image_url content part. This wrapper just sets those two specs plus
# the VL model/tokenizer and execs the shared run-guidellm.sh, inheriting its
# preflight, profile handling and before/after /metrics snapshot.
#
# The run is bounded to MAX_SECONDS (default 600s) via run-guidellm.sh's
# --constraint kind=max_duration.
#
# Prerequisites:
#   1. The multimodal server is already up:
#        ./run-serve-certus-shmq-multimodal-2gpu.sh
#   2. guidellm is installed on the HOST (pip install guidellm).
#
# Examples:
#   ./run-guidellm-synthetic-multimodal.sh                          # 16 streams, 720p image, 600s
#   CONCURRENT_STREAMS=8 ./run-guidellm-synthetic-multimodal.sh     # lighter closed-loop
#   RESOLUTION=1080p ./run-guidellm-synthetic-multimodal.sh         # bigger images (more prefill)
#   RATE_TYPE=throughput MAX_SECONDS=120 ./run-guidellm-synthetic-multimodal.sh
#   IMAGES_PER_REQUEST=2 ./run-guidellm-synthetic-multimodal.sh     # 2 images/prompt (server must allow it)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── Text side (synthetic_text -> "prompt"): a shared prefix per group gives the
#    KV-offload read path something to reuse; the unique question + reply are the
#    per-request work. Folded into synthetic_text by run-guidellm.sh.
#    prefix_tokens=8192, prefix_count=128: sized to force heavy KV-cache spilling.
#    Reusable working set ~ prefix_count * prefix_tokens = 128 * 8192 = 1,048,576
#    tokens ≈ 9.2x the measured HBM KV pool (~114k tokens at TP=2, GPU_MEM_UTIL=0.80),
#    so the bulk of the reusable KV must live in / be re-read from the offload tier.
#    Per-request stays under MAX_MODEL_LEN=32768 (8192+256+128=8576 tokens/request).
DATA="${DATA:-prompt_tokens=256,output_tokens=128,prefix_tokens=8192,prefix_count=128,turns=100}"

PATH="$HOME/venv_guidellm/bin:$PATH"
HF_HOME=/mnt/certus1/hf-cache

# ── Image side (synthetic_image -> "image"): a synthetic JPEG per request.
#    RESOLUTION is a guidellm preset (240p/360p/480p/540p/720p/1080p/1440p/2160p/4k);
#    IMAGES_PER_REQUEST>1 requires the server's --limit-mm-per-prompt to allow it.
RESOLUTION="${RESOLUTION:-1080p}"
IMAGES_PER_REQUEST="${IMAGES_PER_REQUEST:-1}"
IMAGE_CONTENT="${IMAGE_CONTENT:-noise}"   # gradient|noise|solid|checkerboard (noise = least-compressible)
IMAGE_DATA="${IMAGE_DATA:-kind=synthetic_image,resolution=${RESOLUTION},content=${IMAGE_CONTENT},images_per_request=${IMAGES_PER_REQUEST}}"

MODEL="${MODEL:-qwen3-vl-32b}" \
PROCESSOR="${PROCESSOR:-Qwen/Qwen3-VL-32B-Instruct}" \
PORT="${PORT:-8000}" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-16}" \
MAX_SECONDS="${MAX_SECONDS:-600}" \
DATA="$DATA" \
IMAGE_DATA="$IMAGE_DATA" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
