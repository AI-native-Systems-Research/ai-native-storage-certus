#!/bin/bash
# run-guidellm-synthetic-moe.sh — CLIENT side of the synthetic multi-turn
# KV-offload test for a sparse-MoE text model (default: Mixtral-8x7B-Instruct-FP8).
#
# Thin wrapper over run-guidellm.sh, paired with run-serve-cputier-mixtral-2gpu.sh
# / run-serve-certus-shmq-mixtral-2gpu.sh. It is the MoE/text sibling of
# run-guidellm-synthetic-multimodal.sh: same CLOSED-LOOP concurrent workload
# (CONCURRENT_STREAMS fixed in-flight streams, each a 49-turn synthetic_text
# conversation of 512-token prompts / 128-token replies) but text-only — no image
# --data source, so no Pillow/guidellm[vision] dependency.
#
# The only difference from run-guidellm-synthetic.sh is the default model
# identity: MODEL (the --served-model-name the server advertises) and PROCESSOR
# (the HF tokenizer for token accounting) default to Mixtral instead of Qwen. Both
# are env-overridable, so this also fronts any other MoE/text checkpoint served by
# a matching wrapper.
#
# The server must serve a window large enough for the accumulated conversation
# history (~640 tok/turn: 512 prompt + 128 reply). The turns=49 default peaks at
# ~31.4K tokens, deliberately just under Mixtral's native 32768 window — the
# deepest same-shape multi-turn run that fits with no YaRN rope-scaling. Push turns
# higher only alongside a larger served window (see the Mixtral serve wrappers).
#
#   ./run-guidellm-synthetic-moe.sh                               # 32 streams, 1200s, Mixtral
#   CONCURRENT_STREAMS=64 MAX_SECONDS=600 ./run-guidellm-synthetic-moe.sh
#   MODEL=mixtral-8x7b PROCESSOR=RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8 ./run-guidellm-synthetic-moe.sh
#   DATA="kind=synthetic_text,prompt_tokens=1024,output_tokens=256,turns=10" ./run-guidellm-synthetic-moe.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-mixtral-8x7b}" \
PROCESSOR="${PROCESSOR:-RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8}" \
PORT="${PORT:-8000}" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-32}" \
MAX_SECONDS="${MAX_SECONDS:-1200}" \
DATA="${DATA:-kind=synthetic_text,prompt_tokens=512,output_tokens=128,turns=49}" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
