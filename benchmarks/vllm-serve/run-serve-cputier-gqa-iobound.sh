#!/bin/bash
# run-serve-cputier-gqa-iobound.sh — GQA counterpart of
# run-serve-cputier-mha-iobound.sh. SERVER side of the KV-IO test, cputier backend
# (vLLM-native CPU + fs tiering). The counterpart of
# run-serve-certus-shmq-gqa-iobound.sh; pairs with the SAME client,
# run-guidellm-gqa-iobound.sh. Thin wrapper over run-serve-cputier.sh.
#
# Serves Qwen2.5-7B-Instruct (GQA, 56 KiB/token — see the shmq GQA wrapper's header
# for why this model is the controlled MHA->GQA swap and what it does to the I/O
# profile). The only difference from the shmq GQA arm is where the offload tiers
# live: here they are in-process (a /dev/shm CPU primary tier + an "fs" disk
# secondary tier) instead of the external certus-server.
#
# ── IS THIS STILL I/O-BOUND? ──────────────────────────────────────────────────────
# At the shared default workload (client PREFIX_TOKENS=8192 x PREFIX_COUNT=64) the
# reused working set is ~28 GiB (vs ~256 GiB on the MHA arm) because GQA's KV is 56
# vs 512 KiB/token. ~28 GiB FITS a 32 G CPU tier, so at the default tier this run
# stays in host RAM, NOT the fs/SSD secondary tier — the expected effect of GQA. To
# drive the same SSD-read regime as the MHA arm (byte-matched, ~224 GiB), raise the
# client's PREFIX_COUNT to ~512, or lower CPU_TIER_GB on BOTH arms. The CPU primary
# tier (CPU_TIER_GB, default 32G) is kept EQUAL to the certus-server
# --memory-tier-size the shmq arm uses so the two backends share the same DRAM
# budget.
#
# SELF-CONTAINED (no certus-server). Needs enough free host RAM for the CPU tier +
# /dev/shm headroom, and disk space under FS_TIER_HOST for whatever spills. Stop
# certus-server and release its hugepages first, or the CPU tier may OOM at init.
#
# TP=1 on one A100-40G. Native 32768 window, no YaRN (the *qwen2* YaRN auto-path only
# activates above native). First run downloads Qwen2.5-7B-Instruct (~15 GB) into
# HF_CACHE.
#
# IMAGE: pinned to certus-offload:vllm0.30.0 (not the cputier base's 0.29 default)
# so this arm matches the SHMQ arm's vLLM 0.30 — a same-version A/B. Override IMAGE=
# to compare a different build.
#
#   ./run-serve-cputier-gqa-iobound.sh                     # Qwen2.5-7B GQA, TP=1, 32768, 32G CPU + fs tier
#   CPU_TIER_GB=8 ./run-serve-cputier-gqa-iobound.sh       # smaller DRAM tier -> push the ~28 GiB WS to SSD
#   FS_TIER=0 ./run-serve-cputier-gqa-iobound.sh           # CPU-only (no SSD tier) — DRAM-bound control
#   OFFLOAD_BLOCK_SIZE=64 ./run-serve-cputier-gqa-iobound.sh # 64-token offload blocks (set it on the Certus arm too)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b}"   # matches run-guidellm-gqa-iobound.sh MODEL
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"                # native window; no YaRN
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"                # 7B fits one A100-40G
GPU="${GPU:-0}"                                        # pin one GPU (TP=1); GPU=all exposes both
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}"
KV_CACHE_BYTES="${KV_CACHE_BYTES-}"                    # uncapped: ~20G pool >> ~1.75G floor at 32768
# Default: the GPU block (16 tokens), as the Certus arm, so both arms offload at the
# same granularity. Set the same OFFLOAD_BLOCK_SIZE on both arms to compare another size.
OFFLOAD_BLOCK_SIZE="${OFFLOAD_BLOCK_SIZE-}"
# DRAM/CPU tier size. Keep EQUAL to the certus-server --memory-tier-size used by the
# shmq GQA arm (run-serve-certus-shmq-gqa-iobound.sh honors the same CPU_TIER_GB) so
# the two backends get the same host-memory cache for a fair A/B. At the default
# workload the ~28 GiB working set fits this tier — see the I/O-bound note above.
CPU_TIER_GB="${CPU_TIER_GB:-32}"
CPU_GB="${CPU_GB:-$CPU_TIER_GB}"
CPU_BYTES="${CPU_BYTES:-$((CPU_GB * (1 << 30)))}"
FS_TIER="${FS_TIER:-1}"                                # fs/SSD secondary tier on
# Pin vLLM 0.30 to match the SHMQ arm (run-serve-certus-shmq-gqa-iobound.sh) for a
# same-version A/B. The cputier base otherwise defaults to 0.29.
IMAGE="${IMAGE:-certus-offload:vllm0.30.0}"
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:---max-num-batched-tokens 8192}"

if pgrep -f 'certus-server.*--shm-path' >/dev/null; then
  echo "[gqa-iobound] WARNING: certus-server is still running — stop it (and free hugepages) so the ${CPU_GB}G CPU tier has RAM" >&2
fi

echo "[gqa-iobound] Qwen2.5-7B-Instruct (GQA, 56 KiB/token) TP=${TENSOR_PARALLEL} GPU=${GPU} window=${MAX_MODEL_LEN} offload-block=${OFFLOAD_BLOCK_SIZE:-gpu} CPU-tier=${CPU_GB}G (=CPU_TIER_GB) fs-tier=${FS_TIER}"

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
