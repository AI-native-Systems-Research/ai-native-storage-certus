#!/bin/bash
# run-serve-cputier-50pt32g.sh — SERVER side of the "50pt@32G" experiment, cputier
# backend (vLLM-native CPU + fs tiering). The counterpart of
# run-serve-certus-shmq-50pt32g.sh; pairs with run-guidellm-50pt32g.sh. Thin wrapper
# over run-serve-cputier.sh.
#
# Parameters come from the dram_dashboard preset
#   ~/certus/tools/kv-cache-dynamics/dram-dashboard-config-50pt@32G.yaml
# mapped identically to the SHMQ arm, except the offload tiers live in-process:
#
#   preset key                      -> here
#   layers 48 / kvheads 8 / hd 128  -> Qwen2.5-14B-Instruct (192 KiB KV/token, fp16 bpe 2)
#   hbm 10 (GB, total GPU KV)       -> KV_CACHE_BYTES=5G per worker x TP=2
#   dram 32 (the "@32G" point)      -> CPU_BYTES=32G CPU primary tier (cpu_bytes_to_use is
#                                      the TOTAL across TP workers, so this matches the
#                                      SHMQ arm's 32G certus-server memory tier)
#   ssd 20000 (≈ unbounded)         -> fs secondary tier under FS_TIER_HOST (unbounded)
#   mml 500000                      -> MAX_MODEL_LEN=49152 (same DEVIATION as the SHMQ arm)
#   block 64                        -> OFFLOAD_BLOCK_SIZE=64 (64-token offload blocks;
#                                      GPU prefix caching keeps vLLM's 16-token block)
#
# SELF-CONTAINED (no certus-server), but it needs ~36G of free host RAM for the
# pinned CPU tier + /dev/shm headroom. Stop certus-server and release its hugepages
# before running this arm, or the CPU tier may OOM at init.
#
# IMAGE: run-serve-cputier.sh defaults to certus-offload:vllm0.29.0, while the SHMQ arm
# uses the vLLM 0.28 connector. For a same-version comparison, pin one or the other,
# e.g. IMAGE=certus-offload:vllm0.28.0 if that image is built.
#
#   ./run-serve-cputier-50pt32g.sh                          # 14B @ 49152, 10G GPU KV, 32G CPU + fs tier
#   CPU_GB=64 ./run-serve-cputier-50pt32g.sh                # another DRAM sweep point
#   FS_TIER=0 ./run-serve-cputier-50pt32g.sh                # CPU-only (no SSD tier)
#   OFFLOAD_BLOCK_SIZE= ./run-serve-cputier-50pt32g.sh      # offload at the GPU block size (16)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-14b}"   # matches run-guidellm-50pt32g.sh MODEL
MAX_MODEL_LEN="${MAX_MODEL_LEN:-49152}"
# TP=2: the 14B weights (~29G) plus KV do not fit one 40G A100.
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}"
# hbm=10 is TOTAL GPU KV; KV_CACHE_BYTES is PER WORKER, so 10G / TP=2 = 5G.
KV_CACHE_BYTES="${KV_CACHE_BYTES-5G}"
OFFLOAD_BLOCK_SIZE="${OFFLOAD_BLOCK_SIZE-64}"
CPU_GB="${CPU_GB:-32}"
CPU_BYTES="${CPU_BYTES:-$((CPU_GB * (1 << 30)))}"
FS_TIER="${FS_TIER:-1}"
# Bound the prefill chunk so activation memory stays flat on long prompts.
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:---max-num-batched-tokens 8192}"

if pgrep -f 'certus-server.*--shm-path' >/dev/null; then
  echo "[50pt32g] WARNING: certus-server is still running — stop it (and free hugepages) so the ${CPU_GB}G CPU tier has RAM" >&2
fi

echo "[50pt32g] HBM ${KV_CACHE_BYTES:-uncapped}/worker x TP=${TENSOR_PARALLEL}, window ${MAX_MODEL_LEN}, offload block ${OFFLOAD_BLOCK_SIZE:-gpu} tok, CPU tier ${CPU_GB}G, fs tier ${FS_TIER}"

# Hand off via `env` so the optional vars can be conditionally included: a
# `${VAR:+VAR=val}` env ARG either passes the assignment or vanishes.
exec env \
  MODEL="$MODEL" \
  SERVED_MODEL_NAME="$SERVED_MODEL_NAME" \
  MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  TENSOR_PARALLEL="$TENSOR_PARALLEL" \
  GPU="${GPU:-all}" \
  GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}" \
  CPU_BYTES="$CPU_BYTES" \
  FS_TIER="$FS_TIER" \
  EXTRA_SERVE_ARGS="$EXTRA_SERVE_ARGS" \
  ${KV_CACHE_BYTES:+KV_CACHE_BYTES="$KV_CACHE_BYTES"} \
  ${OFFLOAD_BLOCK_SIZE:+OFFLOAD_BLOCK_SIZE="$OFFLOAD_BLOCK_SIZE"} \
  "${SCRIPT_DIR}/run-serve-cputier.sh"
