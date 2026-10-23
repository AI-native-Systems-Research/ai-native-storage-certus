#!/bin/bash
# run-guidellm-video.sh — CLIENT side of the synthetic VIDEO multimodal KV-stress test.
#
# Drives a vision-language server (Qwen3-VL-32B behind the Certus-SHMQ or cputier
# offload connector) with text+VIDEO chat requests. Each request carries a synthetic
# mp4 clip whose FRAMES expand into vision tokens during prefill — video is the
# single heaviest KV-cache multimodal load, because the per-request prefill KV grows
# ~linearly with frame count. The goal here is to stress the KV cache / offload tier
# as hard as possible: a few huge-context requests in flight, each filling the window
# with video tokens, forcing massive DRAM->SSD spill.
#
# How the request is assembled (run-guidellm.sh merges --data sources column-wise):
#   DATA        -> kind=synthetic_text  -> "prompt" column (a short text instruction)
#   VIDEO_DATA  -> kind=synthetic_video -> "video"  column (base64 data:video/mp4 URL)
# The openai_http backend renders the video column as a video_url content part.
#
# FRAMES is the KV lever. Video tokens for Qwen3-VL scale ~ frames * (H*W)/(patch^2)
# with temporal+spatial merging, so raise FRAMES (and/or RESOLUTION) until the clip's
# tokens approach the served window. Calibrate against the server's reported prompt
# tokens; if requests overflow the window they come back HTTP 400.
#
# SERVER REQUIREMENTS (set at serve launch, not here):
#   1. Video enabled:   EXTRA_SERVE_ARGS='--limit-mm-per-prompt {"image":0,"video":1}'
#   2. A large window to hold the video's tokens, and FP8 KV so it fits the pool, e.g.
#        MAX_MODEL_LEN=262144 \
#        EXTRA_SERVE_ARGS='--limit-mm-per-prompt {"image":0,"video":1} --kv-cache-dtype fp8'
#      (both args go in one EXTRA_SERVE_ARGS string). Qwen3-VL is natively long-context,
#      so no YaRN is required for the VL model.
#   3. guidellm host needs imageio + imageio-ffmpeg (mp4 encode) — run-guidellm.sh checks.
#
# Examples:
#   ./run-guidellm-video.sh                                  # 16 streams, 720p, 64 frames, 1 turn, 600s
#   FRAMES=128 RESOLUTION=480p ./run-guidellm-video.sh       # more frames, smaller frames
#   CONCURRENT_STREAMS=8 FRAMES=256 ./run-guidellm-video.sh  # fewer, deeper clips
#   TURNS=10 ./run-guidellm-video.sh                         # multi-turn sessions (growing text history)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PATH="$HOME/venv_guidellm/bin:$PATH"
HF_HOME=/mnt/certus1/hf-cache

# ── Output location: video runs produce HUGE guidellm JSONs — guidellm serializes
#    each request's base64 mp4 data URL into the output (GB in minutes). run-guidellm.sh
#    would default OUTPUT to benchmarks/vllm-serve/ (under /home) and fill the home volume.
#    Force it onto /mnt/certus1 (the standing "results on /mnt/certus1, never /home" rule).
#    Override OUTPUT or OUT_DIR to relocate; never let it land on /home for video.
OUT_DIR="${OUT_DIR:-/mnt/certus1/vllm-serve-matrix/video}"
mkdir -p "$OUT_DIR" 2>/dev/null || true
OUTPUT="${OUTPUT:-${OUT_DIR}/video_$(date +%Y%m%d_%H%M%S).json}"

# ── Text side: minimal — the video dominates prefill KV. ────────────────────────
#    TURNS is the number of conversation turns per session (default 1 = one-shot).
#    turns>1 makes each session multi-turn, accumulating history across turns; this
#    is the lever for in-session prefix reuse. NOTE: with turns>1 guidellm appends a
#    new synthetic_video per turn (one video column per row), so the clip is not
#    necessarily re-sent across turns — the reusable prefix is the growing TEXT
#    history, not the video. Set DATA directly to override the whole text spec.
TURNS="${TURNS:-8}"
DATA="${DATA:-kind=synthetic_text,prompt_tokens=128,output_tokens=128,turns=${TURNS}}"

# ── Video side: FRAMES is the KV lever (more frames -> more vision tokens -> more KV).
#    RESOLUTION is a guidellm preset (240p/360p/480p/540p/720p/1080p/...); content=noise
#    is least-compressible (biggest mp4, though token count tracks frames/resolution).
RESOLUTION="${RESOLUTION:-720p}"
FRAMES="${FRAMES:-64}"
FPS="${FPS:-8}"
VIDEO_CONTENT="${VIDEO_CONTENT:-noise}"
VIDEO_DATA="${VIDEO_DATA:-kind=synthetic_video,resolution=${RESOLUTION},frames=${FRAMES},fps=${FPS},content=${VIDEO_CONTENT}}"

MODEL="${MODEL:-qwen3-vl-32b}" \
PROCESSOR="${PROCESSOR:-Qwen/Qwen3-VL-32B-Instruct}" \
PORT="${PORT:-8000}" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-32}" \
MAX_SECONDS="${MAX_SECONDS:-600}" \
DATA="$DATA" \
VIDEO_DATA="$VIDEO_DATA" \
OUTPUT="$OUTPUT" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
