#!/bin/bash
# run-serve-lmcache-mha-iobound.sh — SERVER side of the MHA KV-IO-bound test,
# LMCache backend (MP server: pinned-RAM L1 + fs_native SSD L2). The counterpart of
# run-serve-certus-shmq-mha-iobound.sh and run-serve-cputier-mha-iobound.sh; pairs
# with the SAME client, run-guidellm-mha-iobound.sh. Thin wrapper over
# run-serve-lmcache.sh.
#
# Serves Qwen1.5-7B-Chat (MHA, 512 KiB/token). The ~256 GiB reused working set
# dwarfs the CPU_TIER_GB L1 (default 32G, EQUAL to the sibling arms' DRAM tier), so
# reuse is read back from the SSD L2. DISK_GB (default 400G) must exceed the working
# set or L2 evicts it too.
#
# TP=1 on one A100-40G (as the siblings). It does NOT fit a 24 GB GPU at 32768:
# the window alone needs ~16 GiB of KV next to ~15 GiB of weights.
#
# Differences that remain (say so in results): LMCache stores 256-token chunks
# (CHUNK_SIZE) where the siblings offload 64-token blocks, and its L2 is
# write-through rather than written on eviction.
#
#   ./run-serve-lmcache-mha-iobound.sh                     # Qwen1.5-7B MHA, TP=1, 32768, 32G L1 + SSD L2
#   CPU_TIER_GB=64 ./run-serve-lmcache-mha-iobound.sh      # larger DRAM tier (match the shmq arm's server)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen1.5-7B-Chat}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen1.5-7b}"   # matches run-guidellm-mha-iobound.sh MODEL
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"
GPU="${GPU:-0}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}"
CPU_TIER_GB="${CPU_TIER_GB:-32}"
CPU_GB="${CPU_GB:-$CPU_TIER_GB}"
DISK_GB="${DISK_GB:-400}"
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:---max-num-batched-tokens 8192}"

echo "[mha-iobound] Qwen1.5-7B-Chat (MHA, 512 KiB/token) TP=${TENSOR_PARALLEL} GPU=${GPU} window=${MAX_MODEL_LEN} offload-block=${CHUNK_SIZE:-256} CPU-tier=${CPU_GB}G (=CPU_TIER_GB, LMCache L1) L2=${DISK_GB}G"

exec env \
  MODEL="$MODEL" SERVED_MODEL_NAME="$SERVED_MODEL_NAME" MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  TENSOR_PARALLEL="$TENSOR_PARALLEL" GPU="$GPU" GPU_MEM_UTIL="$GPU_MEM_UTIL" \
  CPU_GB="$CPU_GB" DISK_GB="$DISK_GB" EXTRA_SERVE_ARGS="$EXTRA_SERVE_ARGS" \
  "${SCRIPT_DIR}/run-serve-lmcache.sh"
