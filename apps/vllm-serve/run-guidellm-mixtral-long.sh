#!/bin/bash
# run-guidellm-mixtral-long.sh — CLIENT side of the LONG-CONTEXT Mixtral KV-offload
# stress test (a heavier sibling of run-guidellm-synthetic-moe.sh).
#
# Same closed-loop concurrent synthetic_text workload as the moe driver, but the
# conversations are ~4x deeper: turns=196 at 512-token prompts / 128-token replies
# peaks at ~126K tokens/conversation. At CONCURRENT_STREAMS=32 that is a ~512 GB
# KV working set (Mixtral-8x7B ~128 KiB/token), driving heavy DRAM->SSD spill.
#
# IMPORTANT — the SERVER must be started with a matching long window, or every
# overflowing request comes back HTTP 400. Mixtral's native window is 32768 and it
# ships NO YaRN config, so you must FORCE rope-scaling on the serve side:
#     MAX_MODEL_LEN=131072 \
#     EXTRA_SERVE_ARGS='--hf-overrides {"rope_scaling":{"rope_type":"yarn","factor":4.0,"original_max_position_embeddings":32768}}' \
#       ./run-serve-certus-shmq.sh        # (or run-serve-cputier.sh for the cputier backend)
# YaRN degrades output quality on Mixtral (it was not trained long-context); this
# is a KV-cache STRESS test, not a quality run.
#
# Everything is env-overridable. Examples:
#   ./run-guidellm-mixtral-long.sh                                  # 32 streams, 1200s, turns=196
#   CONCURRENT_STREAMS=64 ./run-guidellm-mixtral-long.sh            # more churn
#   DATA="kind=synthetic_text,prompt_tokens=512,output_tokens=128,turns=98" ./run-guidellm-mixtral-long.sh  # 2x (needs 65536 window)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-mixtral-8x7b}" \
PROCESSOR="${PROCESSOR:-RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8}" \
PORT="${PORT:-8000}" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-32}" \
MAX_SECONDS="${MAX_SECONDS:-1200}" \
DATA="${DATA:-kind=synthetic_text,prompt_tokens=512,output_tokens=128,turns=196}" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
