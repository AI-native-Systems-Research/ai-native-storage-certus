#!/bin/bash
# run-serve-lmcache-gqa-iobound.sh — SERVER side of the GQA KV-IO test, LMCache
# backend (MP server: pinned-RAM L1 + fs_native SSD L2). The counterpart of
# run-serve-certus-shmq-gqa-iobound.sh and run-serve-cputier-gqa-iobound.sh; pairs
# with the SAME client, run-guidellm-gqa-iobound.sh. Thin wrapper over
# run-serve-lmcache.sh.
#
# Serves Qwen2.5-7B-Instruct (GQA, 56 KiB/token) with the same window, TP and
# batching as the sibling arms. The L1 is CPU_TIER_GB (default 32G), EQUAL to the
# cputier CPU tier and the certus-server --memory-tier-size, so all three get the
# same DRAM budget. As on the other arms, the default ~28 GiB working set fits that
# DRAM tier; raise the client's PREFIX_COUNT (~512) or lower CPU_TIER_GB on every
# arm to reach the SSD. The L2 holds DISK_GB (default 400G) under DISK_DIR/l2.
#
# Differences that remain (say so in results): LMCache stores 256-token chunks
# (CHUNK_SIZE) where the siblings offload 64-token blocks, and its L2 is
# write-through rather than written on eviction.
#
#   ./run-serve-lmcache-gqa-iobound.sh                     # Qwen2.5-7B GQA, TP=1, 32768, 32G L1 + SSD L2
#   CPU_TIER_GB=8 ./run-serve-lmcache-gqa-iobound.sh       # smaller DRAM tier -> push the ~28 GiB WS to SSD
#   CHUNK_SIZE=64 ./run-serve-lmcache-gqa-iobound.sh       # match the siblings' 64-token offload block
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b}"   # matches run-guidellm-gqa-iobound.sh MODEL
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"                # native window; no YaRN
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"
GPU="${GPU:-0}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}"
CPU_TIER_GB="${CPU_TIER_GB:-32}"
CPU_GB="${CPU_GB:-$CPU_TIER_GB}"
DISK_GB="${DISK_GB:-400}"
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:---max-num-batched-tokens 8192}"

echo "[gqa-iobound] Qwen2.5-7B-Instruct (GQA, 56 KiB/token) TP=${TENSOR_PARALLEL} GPU=${GPU} window=${MAX_MODEL_LEN} offload-block=${CHUNK_SIZE:-256} CPU-tier=${CPU_GB}G (=CPU_TIER_GB, LMCache L1) L2=${DISK_GB}G"

exec env \
  MODEL="$MODEL" SERVED_MODEL_NAME="$SERVED_MODEL_NAME" MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  TENSOR_PARALLEL="$TENSOR_PARALLEL" GPU="$GPU" GPU_MEM_UTIL="$GPU_MEM_UTIL" \
  CPU_GB="$CPU_GB" DISK_GB="$DISK_GB" EXTRA_SERVE_ARGS="$EXTRA_SERVE_ARGS" \
  "${SCRIPT_DIR}/run-serve-lmcache.sh"
