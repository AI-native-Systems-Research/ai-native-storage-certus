#!/bin/bash
# run-guidellm-mixtral-xlong.sh — CLIENT side of the EXTRA-deep (256K-context)
# Mixtral KV-offload stress test. Deeper sibling of run-guidellm-mixtral-long.sh.
#
# Conversations are ~2x deeper again: turns=392 at 512-token prompts / 128-token
# replies peaks at ~256K tokens/conversation. At CONCURRENT_STREAMS=32 that is a
# ~1 TB KV working set, maximizing DRAM->SSD spill reach on this hardware.
#
# IMPORTANT — 256K does NOT fit the HBM KV pool at fp16 KV (one 256K sequence
# needs 262,144 tokens but the pool holds ~174,000). The SERVER MUST therefore
# run with FP8 KV cache to roughly double the pool (~348,000 tokens), AND force
# YaRN factor 8 (Mixtral native window is 32768, ships no YaRN):
#     MAX_MODEL_LEN=262144 \
#     EXTRA_SERVE_ARGS='--kv-cache-dtype fp8 --hf-overrides {"rope_scaling":{"rope_type":"yarn","factor":8.0,"original_max_position_embeddings":32768}}' \
#       ./run-serve-certus-shmq.sh        # (or run-serve-cputier.sh)
# Without --kv-cache-dtype fp8 the server fails the startup KV-sizing check
# ("max seq len larger than KV cache"); without the 256K window every request 400s.
# Caveats: FP8 KV is lower precision and halves KV bytes/token (deeper reach,
# ~same total bytes); YaRN factor 8 destroys output quality. This is a KV-cache
# STRESS test, not a quality run.
#
# Env-overridable. Examples:
#   ./run-guidellm-mixtral-xlong.sh                                 # 32 streams, 1200s, turns=392
#   CONCURRENT_STREAMS=16 ./run-guidellm-mixtral-xlong.sh           # lighter concurrency
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-mixtral-8x7b}" \
PROCESSOR="${PROCESSOR:-RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8}" \
PORT="${PORT:-8000}" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-32}" \
MAX_SECONDS="${MAX_SECONDS:-1200}" \
DATA="${DATA:-kind=synthetic_text,prompt_tokens=512,output_tokens=128,turns=392}" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
