#!/bin/bash
# run-cc131k-example.sh — kv-cache-dynamics for the cc131k workload.
#
# Models block-level LRU KV-cache reuse for the 131K cc-traces replay served by
# apps/vllm-serve/run-serve-certus-shmq-cc131k.sh: Qwen2.5-14B-Instruct at a
# 131072-token window on 2x A100-40G (TP=2), driven by the inter-arrival turn
# intervals extracted from the cc-traces-weka-062126 trace.
#
# All values below are env-overridable, e.g.:
#   HBM_GB=24 CONCURRENT=4 ./run-cc131k-example.sh
#   DRAM_GB=0 SSD_GB=0 ./run-cc131k-example.sh      # HBM-only LRU, no offload
#   DIST=/path/to/other-intervals.yaml ./run-cc131k-example.sh
#   ./run-burstgpt-example.sh   # same, with BurstGPT
#       think-times (chat users, p50 131s / p99 22h) + cc-weka turns/contexts
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# --- Turn-interval distribution ------------------------------------------------
# The bundled YAML is the inter-arrival distribution of cc-traces-weka-062126
# (median 16.9s, p90 146.8s, p99 3098s) — the same trace the .mooncake-131k.jsonl
# replay is rendered from, so the per-turn timing is identical. Regenerate from
# the raw trace with, in benchmarks/synthetic-gen-by-llm/:
#   ./extract_interval_distribution.py /mnt/certus1/cc-traces-weka-062126.jsonl -o dist.yaml
DIST="${DIST:-${SCRIPT_DIR}/cc-trace-weka-062126-intervals.yaml}"

# --- Model attributes: Qwen2.5-14B-Instruct (from its config.json) -------------
# 48 layers, 8 KV heads (GQA), head_dim 128, BF16 => 192 KB/token KV footprint;
# a full 131072-token sequence is 24.00 GB of KV. block_size 64 matches the
# cc-trace hash_id granularity.
NUM_LAYERS="${NUM_LAYERS:-48}"
NUM_KV_HEADS="${NUM_KV_HEADS:-8}"
HEAD_DIM="${HEAD_DIM:-128}"
BYTES_PER_ELEMENT="${BYTES_PER_ELEMENT:-2}"   # BF16
BLOCK_SIZE="${BLOCK_SIZE:-64}"

# --- Cache capacity ------------------------------------------------------------
# TP=2 on 2x A100-40G at GPU_MEM_UTIL=0.9 => ~72 GB usable; ~28 GB holds the 14B
# BF16 weights, leaving ~40 GB for the KV cache. One full 131K sequence alone is
# 24 GB, so the window is heavily oversubscribed under concurrency — exactly the
# regime where LRU eviction decides the achieved reuse rate.
# (CACHE_GB is still honoured as the old name for HBM_GB.)
HBM_GB="${HBM_GB:-${CACHE_GB:-20}}"

# Offload tiers below HBM: blocks evicted from HBM demote to DRAM, then SSD, and
# are promoted back to HBM when reused. 0 disables a tier; SSD_GB=inf is
# unbounded storage. Sized for this node (503 GB host DRAM, 4x 2.9 TB NVMe):
#   DRAM_GB=128   a quarter of host RAM, leaving room for the serving stack;
#                 the cc131k CPU-tier knee measured on vLLM sits ~256G, so 128G
#                 is deliberately below saturation and DRAM hit rate is visible.
#   SSD_GB=2048   a 2 TiB KV partition — well under one 2.9 TB drive, and finite
#                 so bottom-tier drops show up (inf never drops).
DRAM_GB="${DRAM_GB:-128}"
SSD_GB="${SSD_GB:-20000}"

# --- Shared prefix -------------------------------------------------------------
# Tokens of a common prefix (system prompt + tool definitions) every
# conversation opens with; its full blocks are cached once for all sessions.
# ASSUMPTION: the trace's hash_ids are conversation-local, so it cannot confirm
# cross-session sharing. 20000 sits under the floor of the opening context
# (the 2nd main-model request has >= 37.5K tokens in 90% of conversations,
# median 54K), i.e. a plausible Claude Code system prompt + tool-definition
# block. Set PREFIX_TOKENS=0 for no sharing.
PREFIX_TOKENS="${PREFIX_TOKENS:-20000}"

# --- Cross-session sharing ---------------------------------------------------
# SHARED_FRACTION: leading fraction (0-1) of each conversation's live context,
# after the prefix, that is identical across its share group — e.g. agents
# reading the same repository. Cached once per group; idealised as front-loaded
# content, so it is an upper bound on prefix sharing. SHARE_GROUPS: number of
# groups (sessions dealt round-robin). 0 = fully private (the trace's hash ids
# are conversation-local, so it shows no such sharing).
SHARED_FRACTION="${SHARED_FRACTION:-0}"
SHARE_GROUPS="${SHARE_GROUPS:-1}"

# --- Admission policy ----------------------------------------------------------
# interval (default): turns ordered by the trace's sampled think-time gaps.
# round-robin: cycle through active sessions (LRU worst case); random: uniform.
ADMISSION="${ADMISSION:-interval}"

# --- Context window + turn counts (both trace-derived) -------------------------
# MAX_MODEL_LEN=131072 is the served window; a conversation's live KV becomes a
# rolling window capped at 2048 blocks (24 GB), the real single-131K-sequence
# footprint — which is how the deep cc conversations stay servable.
# TURN_DIST samples each conversation's turn count from the cc trace's own
# per-conversation turn distribution (median 67, mean 149, max 3052); set it to
# the empty string to fall back to a fixed NUM_TURNS instead.
MAX_MODEL_LEN="${MAX_MODEL_LEN:-131072}"
TURN_DIST="${TURN_DIST-${SCRIPT_DIR}/cc-trace-weka-062126-turns.yaml}"
NUM_TURNS="${NUM_TURNS:-40}"   # used only when TURN_DIST is empty

# --- Per-turn token sizes (trace-derived) --------------------------------------
# From the per-conversation weka-split of the trace (main-model requests): a
# turn's input grows by a mean 1762 tokens over the previous turn's, of which the
# previous response is a mean 1398 output tokens — so 364 new prompt + 1398 gen.
# With the shared prefix the private context fills the 131K window after ~63
# turns (trace median 62 turns/conv), then slides for deeper conversations.
AVG_PROMPT_TOKENS="${AVG_PROMPT_TOKENS:-364}"
AVG_GEN_TOKENS="${AVG_GEN_TOKENS:-1398}"

# --- Load generation -----------------------------------------------------------
# Fewer than two full-131K contexts fit in HBM (24 GB each vs ~40 GB), so 8 open
# conversations oversubscribe HBM and spill into DRAM/SSD between turns; run the
# trace's ~393 conversations through it.
CONCURRENT="${CONCURRENT:-8}"
SESSIONS="${SESSIONS:-393}"
SEED="${SEED:-1}"

ARGS=(
  --num-layers "$NUM_LAYERS"
  --num-kv-heads "$NUM_KV_HEADS"
  --head-dim "$HEAD_DIM"
  --bytes-per-element "$BYTES_PER_ELEMENT"
  --block-size "$BLOCK_SIZE"
  --avg-prompt-tokens "$AVG_PROMPT_TOKENS"
  --avg-gen-tokens "$AVG_GEN_TOKENS"
  --max-model-len "$MAX_MODEL_LEN"
  --hbm-gb "$HBM_GB"
  --dram-gb "$DRAM_GB"
  --ssd-gb "$SSD_GB"
  --prefix-tokens "$PREFIX_TOKENS"
  --admission "$ADMISSION"
  --shared-fraction "$SHARED_FRACTION"
  --share-groups "$SHARE_GROUPS"
  --concurrent-sessions "$CONCURRENT"
  --sessions "$SESSIONS"
  --seed "$SEED"
)
if [[ -n "$TURN_DIST" ]]; then
  ARGS+=(--turn-distribution "$TURN_DIST")
else
  ARGS+=(--num-turns "$NUM_TURNS")
fi

# Extra command-line args (e.g. --json out.json) pass straight through.
exec python3 "${SCRIPT_DIR}/kv_cache_dynamics.py" "$DIST" "${ARGS[@]}" "$@"
