#!/bin/bash
# run-serve-cputier-mha-iobound.sh — SERVER side of the MHA KV-IO-bound test,
# cputier backend (vLLM-native CPU + fs tiering). The counterpart of
# run-serve-certus-shmq-mha-iobound.sh; pairs with the SAME client,
# run-guidellm-mha-iobound.sh. Thin wrapper over run-serve-cputier.sh.
#
# Serves Qwen1.5-7B-Chat (MHA, 512 KiB/token — see the shmq wrapper's header for
# why this model makes the run I/O-bound). The only difference from the shmq arm is
# where the offload tiers live: here they are in-process (a /dev/shm CPU primary
# tier + an "fs" disk secondary tier) instead of the external certus-server.
#
# MADE I/O-BOUND the same way; the CPU primary tier (CPU_TIER_GB, default 32G) is
# the host-RAM cache. It is kept EQUAL to the certus-server --memory-tier-size the
# shmq arm uses so the two backends are compared with the same DRAM budget. The
# ~256 GiB reused working set (client PREFIX_TOKENS x PREFIX_COUNT) still dwarfs it,
# so both overflow into the fs/SSD secondary tier (real NVMe reads on reuse). Lower
# CPU_TIER_GB on BOTH arms for a more disk-dominated run.
#
# SELF-CONTAINED (no certus-server). Needs ~12G free host RAM for the 8G CPU tier +
# /dev/shm headroom, and disk space under FS_TIER_HOST for the working set. Stop
# certus-server and release its hugepages first, or the CPU tier may OOM at init.
#
# TP=1 on one A100-40G. Native 32768 window, no YaRN. First run downloads
# Qwen1.5-7B-Chat (~15 GB) into HF_CACHE.
#
# IMAGE: pinned to certus-offload:vllm0.30.0 (not the cputier base's 0.29 default)
# so this arm matches the SHMQ arm's vLLM 0.30 — a same-version A/B. Override IMAGE=
# to compare a different build.
#
#   ./run-serve-cputier-mha-iobound.sh                     # Qwen1.5-7B MHA, TP=1, 32768, 32G CPU + fs tier
#   CPU_TIER_GB=64 ./run-serve-cputier-mha-iobound.sh      # larger DRAM tier (match the shmq arm's server)
#   FS_TIER=0 ./run-serve-cputier-mha-iobound.sh           # CPU-only (no SSD tier) — DRAM-bound control
#   OFFLOAD_BLOCK_SIZE= ./run-serve-cputier-mha-iobound.sh # offload at the GPU block size (16)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen1.5-7B-Chat}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen1.5-7b}"   # matches run-guidellm-mha-iobound.sh MODEL
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"                # native window; no YaRN
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"                # 7B fits one A100-40G
GPU="${GPU:-0}"                                        # pin one GPU (TP=1); GPU=all exposes both
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}"
KV_CACHE_BYTES="${KV_CACHE_BYTES-}"                    # uncapped: ~20G pool > 16G floor at 32768
OFFLOAD_BLOCK_SIZE="${OFFLOAD_BLOCK_SIZE-64}"
# DRAM/CPU tier size. Keep EQUAL to the certus-server --memory-tier-size used by the
# shmq arm (run-serve-certus-shmq-mha-iobound.sh honors the same CPU_TIER_GB) so the
# two backends get the same host-memory cache for a fair A/B. The ~256 GiB working
# set still dwarfs it, so both spill to SSD.
CPU_TIER_GB="${CPU_TIER_GB:-32}"
CPU_GB="${CPU_GB:-$CPU_TIER_GB}"
CPU_BYTES="${CPU_BYTES:-$((CPU_GB * (1 << 30)))}"
FS_TIER="${FS_TIER:-1}"                                # fs/SSD secondary tier on
# Pin vLLM 0.30 to match the SHMQ arm (run-serve-certus-shmq-mha-iobound.sh) for a
# same-version A/B. The cputier base otherwise defaults to 0.29.
IMAGE="${IMAGE:-certus-offload:vllm0.30.0}"
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:---max-num-batched-tokens 8192}"

if pgrep -f 'certus-server.*--shm-path' >/dev/null; then
  echo "[mha-iobound] WARNING: certus-server is still running — stop it (and free hugepages) so the ${CPU_GB}G CPU tier has RAM" >&2
fi

echo "[mha-iobound] Qwen1.5-7B-Chat (MHA, 512 KiB/token) TP=${TENSOR_PARALLEL} GPU=${GPU} window=${MAX_MODEL_LEN} offload-block=${OFFLOAD_BLOCK_SIZE:-gpu} CPU-tier=${CPU_GB}G (=CPU_TIER_GB) fs-tier=${FS_TIER}"

exec env \
  MODEL="$MODEL" \
  SERVED_MODEL_NAME="$SERVED_MODEL_NAME" \
  MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  TENSOR_PARALLEL="$TENSOR_PARALLEL" \
  GPU="$GPU" \
  GPU_MEM_UTIL="$GPU_MEM_UTIL" \
  CPU_BYTES="$CPU_BYTES" \
  FS_TIER="$FS_TIER" \
  IMAGE="$IMAGE" \
  EXTRA_SERVE_ARGS="$EXTRA_SERVE_ARGS" \
  ${KV_CACHE_BYTES:+KV_CACHE_BYTES="$KV_CACHE_BYTES"} \
  ${OFFLOAD_BLOCK_SIZE:+OFFLOAD_BLOCK_SIZE="$OFFLOAD_BLOCK_SIZE"} \
  "${SCRIPT_DIR}/run-serve-cputier.sh"
