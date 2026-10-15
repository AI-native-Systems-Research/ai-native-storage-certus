#!/bin/bash
# run-serve-certus-shmq-mha-iobound.sh — SERVER side of the MHA KV-IO-bound test.
# Serves Qwen1.5-7B-Chat (MULTI-HEAD attention) over the Certus-SHMQ offload
# connector. Pairs with run-guidellm-mha-iobound.sh. Thin wrapper over
# run-serve-certus-shmq.sh.
#
# WHY THIS MODEL: Qwen1.5-7B-Chat is the MHA counterpart of the GQA Qwen2.5 line —
# 32 layers, 32 Q heads, 32 KV heads (NO grouping), head_dim 128. So its KV is the
# full
#     32 L x 32 KV-heads x 128 x 2 (K,V) x 2 B (fp16) = 512 KiB / token
# which is 9x the 56 KiB/token of GQA Qwen2.5-7B and 2.7x the 192 KiB/token of the
# 14B. Every cached/evicted/reloaded token therefore moves 9x the bytes for the
# same amount of compute, which is exactly how you shift the bottleneck from
# compute to KV I/O. fp16 KV is kept (NOT fp8) to keep bytes/token at the maximum.
#
# HOW THE RUN IS MADE I/O-BOUND (the client does the rest):
#   * big shared prefixes, massively oversubscribed (client PREFIX_TOKENS x
#     PREFIX_COUNT) so the reused working set is hundreds of GiB — far past GPU KV
#     and the DRAM tier — so reuse hits land on the SSD tier (real NVMe reads).
#   * short outputs (client OUTPUT_TOKENS=32) so decode (HBM-bandwidth bound, not
#     tier I/O) is a small slice of each turn.
#   * many deep turns so a resumed conversation reloads a large accumulated chain
#     from the tier on every turn.
#
# TP=1: the 7B weights (~15 GB) + KV fit one A100-40G. At GPU_MEM_UTIL=0.9 the KV
# pool is ~20 GB (~40K tokens at 512 KiB/token), above the one-sequence floor at
# the 32768 window (16 GB) with headroom — so no KV cap is needed, and the spill
# that drives I/O comes from cross-conversation oversubscription, not a cap.
#
# NATIVE 32768 window, no YaRN (the base's YaRN path triggers only for *qwen2*
# model names and only above native; "Qwen1.5" is neither). A larger window is not
# available: YaRN to 65536 would push the one-sequence floor to 32 GB, which does
# not fit alongside weights on 40G.
#
# DRAM tier: lives in the external certus-server (--memory-tier-size). For a fair
# A/B against the cputier arm it must EQUAL that arm's CPU tier: both honor
# CPU_TIER_GB (default 32G). This script can't set the server's size, so it checks
# the running server and warns if it differs. The ~256 GiB working set dwarfs it, so
# reuse still spills to SSD; lower CPU_TIER_GB on BOTH arms for a more SSD-dominated run.
#
# PREREQUISITE: a host certus-server publishing the mailbox at SHM_PATH (any tier
# size). First run downloads Qwen1.5-7B-Chat (~15 GB) into HF_CACHE.
#
#   ./run-serve-certus-shmq-mha-iobound.sh                 # Qwen1.5-7B MHA, TP=1, 32768, 64-tok offload blocks
#   GPU=all ./run-serve-certus-shmq-mha-iobound.sh         # expose both GPUs (default pins GPU 0)
#   KV_CACHE_BYTES=18G ./run-serve-certus-shmq-mha-iobound.sh   # cap GPU KV to force even more spill
#   OFFLOAD_BLOCK_SIZE= ./run-serve-certus-shmq-mha-iobound.sh  # offload at the GPU block size (16)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen1.5-7B-Chat}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen1.5-7b}"   # matches run-guidellm-mha-iobound.sh MODEL
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"                # native window; no YaRN
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"                # 7B fits one A100-40G
GPU="${GPU:-0}"                                        # pin one GPU (TP=1); GPU=all exposes both
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}"
# No KV cap by default: ~20G/util pool > 16G one-sequence floor at 32768, and I/O
# is driven by oversubscription, not a cap. KV_CACHE_BYTES=18G to cap and spill more.
KV_CACHE_BYTES="${KV_CACHE_BYTES-}"
OFFLOAD_BLOCK_SIZE="${OFFLOAD_BLOCK_SIZE-64}"          # 64-token offload blocks (3.5 MiB each at 512 KiB/tok)
# Pin vLLM 0.30 to match the cputier arm (run-serve-cputier-mha-iobound.sh) for a
# same-version A/B. This is also the base default, set explicitly here for parity.
IMAGE="${IMAGE:-localhost/certus-shmq-connector:vllm0.30.0}"
# Chunked prefill so an 8K+ shared prefix / deep reload keeps activation flat.
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:---max-num-batched-tokens 8192}"

# DRAM/CPU tier: the external certus-server's --memory-tier-size (this script cannot
# set it). Keep it EQUAL to the cputier arm's CPU_TIER_GB for a fair A/B — the two
# backends must get the same host-memory cache. Check the running server and warn.
CPU_TIER_GB="${CPU_TIER_GB:-32}"
if [[ "${ACK_MEM:-0}" == "0" ]]; then
  srv_cmd="$(pgrep -a -f 'certus-server.*--shm-path' | head -1 || true)"
  if [[ -z "$srv_cmd" ]]; then
    echo "[mha-iobound] WARNING: no running certus-server found — start it with --memory-tier-size ${CPU_TIER_GB}G, e.g." >&2
    echo "[mha-iobound]          MEMORY_TIER_SIZE=${CPU_TIER_GB}G CHANNELS=32 ./start-certus-optimized.sh" >&2
  else
    tier="$(sed -n 's/.*--memory-tier-size[ =]\([^ ]*\).*/\1/p' <<<"$srv_cmd")"
    if [[ "${tier^^}" != "${CPU_TIER_GB}G" ]]; then
      echo "[mha-iobound] WARNING: certus-server memory tier is '${tier:-default}', the fair-A/B value is CPU_TIER_GB=${CPU_TIER_GB}G (= the cputier CPU tier)." >&2
      echo "[mha-iobound]          restart it: MEMORY_TIER_SIZE=${CPU_TIER_GB}G CHANNELS=32 ./start-certus-optimized.sh  (or set ACK_MEM=1 to ignore)" >&2
    else
      echo "[mha-iobound] certus-server memory tier ${tier} == CPU_TIER_GB (matches the cputier arm)"
    fi
  fi
fi

echo "[mha-iobound] Qwen1.5-7B-Chat (MHA, 512 KiB/token) TP=${TENSOR_PARALLEL} GPU=${GPU} window=${MAX_MODEL_LEN} offload-block=${OFFLOAD_BLOCK_SIZE:-gpu} KV=${KV_CACHE_BYTES:-util${GPU_MEM_UTIL}} DRAM-tier=${CPU_TIER_GB}G (certus-server)"

exec env \
  MODEL="$MODEL" \
  SERVED_MODEL_NAME="$SERVED_MODEL_NAME" \
  MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  TENSOR_PARALLEL="$TENSOR_PARALLEL" \
  GPU="$GPU" \
  GPU_MEM_UTIL="$GPU_MEM_UTIL" \
  IMAGE="$IMAGE" \
  EXTRA_SERVE_ARGS="$EXTRA_SERVE_ARGS" \
  ${KV_CACHE_BYTES:+KV_CACHE_BYTES="$KV_CACHE_BYTES"} \
  ${OFFLOAD_BLOCK_SIZE:+OFFLOAD_BLOCK_SIZE="$OFFLOAD_BLOCK_SIZE"} \
  "${SCRIPT_DIR}/run-serve-certus-shmq.sh"
