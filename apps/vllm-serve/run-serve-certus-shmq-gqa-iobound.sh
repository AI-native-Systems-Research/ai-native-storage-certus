#!/bin/bash
# run-serve-certus-shmq-gqa-iobound.sh — GQA counterpart of
# run-serve-certus-shmq-mha-iobound.sh. SERVER side of the KV-IO test, but serving
# Qwen2.5-7B-Instruct (GROUPED-QUERY attention) over the Certus-SHMQ offload
# connector. Pairs with run-guidellm-gqa-iobound.sh. Thin wrapper over
# run-serve-certus-shmq.sh.
#
# WHY THIS MODEL (the controlled variable vs the MHA arm): Qwen2.5-7B-Instruct is
# the GQA sibling of the MHA Qwen1.5-7B-Chat used by the mha-iobound scripts — same
# family, ~7.6B params, same native 32768 window, head_dim 128, fp16 KV. The one
# thing that changes is attention: 28 layers, 28 Q heads but only 4 KV heads (8:1
# grouping) instead of Qwen1.5's 32 layers x 32 KV heads (no grouping). So its KV is
#     28 L x 4 KV-heads x 128 x 2 (K,V) x 2 B (fp16) = 56 KiB / token
# which is 1/9th the 512 KiB/token of the MHA arm. Every cached/evicted/reloaded
# token therefore moves ~9x FEWER bytes for the same amount of compute. Running the
# IDENTICAL workload against this model is the experiment: it shows how much GQA's
# KV compression relieves the KV-I/O bottleneck the MHA arm is pinned by. fp16 KV is
# kept (NOT fp8) so the only compression in play is GQA, matching the MHA arm.
#
# ── IS THIS STILL I/O-BOUND? (read before interpreting results) ───────────────────
# At the SHARED default workload (client PREFIX_TOKENS=8192 x PREFIX_COUNT=64) the
# reused working set is
#     64 x 8192 tok x 56 KiB/tok  =  ~28 GiB of prefix KV
# vs ~256 GiB on the MHA arm. ~28 GiB FITS inside a 32 G DRAM tier, so at the default
# tier this run is DRAM-resident, NOT SSD-bound — that is precisely the effect of
# GQA, and the honest default for an apples-to-apples workload comparison with MHA.
# To drive the SAME SSD-read regime as the MHA arm (byte-matched, ~224 GiB working
# set), raise the client's PREFIX_COUNT to ~512, or shrink the DRAM tier via
# CPU_TIER_GB on BOTH arms. See run-guidellm-gqa-iobound.sh for the client knobs.
#
# TP=1: the 7B weights (~15 GB) + KV fit one A100-40G. At GPU_MEM_UTIL=0.9 the KV
# pool is ~20 GB (~366K tokens at 56 KiB/token), far above the one-sequence floor at
# the 32768 window (56 KiB x 32768 = ~1.75 GiB) — so no KV cap is needed, and any
# spill that drives I/O comes from cross-conversation oversubscription, not a cap.
#
# NATIVE 32768 window, no YaRN. NOTE: unlike "Qwen1.5", the name "Qwen2.5" DOES match
# the base's *qwen2* YaRN auto-path — but that path only activates ABOVE the native
# length, so staying at MAX_MODEL_LEN=32768 keeps YaRN off and the KV math exact.
#
# DRAM tier: lives in the external certus-server (--memory-tier-size). For a fair
# A/B against the cputier arm it must EQUAL that arm's CPU tier: both honor
# CPU_TIER_GB (default 32G). This script can't set the server's size, so it checks
# the running server and warns if it differs. (At default params the ~28 GiB working
# set fits this tier — see the I/O-bound note above.)
#
# PREREQUISITE: a host certus-server publishing the mailbox at SHM_PATH (any tier
# size). First run downloads Qwen2.5-7B-Instruct (~15 GB) into HF_CACHE.
#
#   ./run-serve-certus-shmq-gqa-iobound.sh                 # Qwen2.5-7B GQA, TP=1, 32768, 64-tok offload blocks
#   GPU=all ./run-serve-certus-shmq-gqa-iobound.sh         # expose both GPUs (default pins GPU 0)
#   KV_CACHE_BYTES=4G ./run-serve-certus-shmq-gqa-iobound.sh    # cap GPU KV to force spill even at 56 KiB/tok
#   CPU_TIER_GB=8 ./run-serve-certus-shmq-gqa-iobound.sh   # smaller DRAM tier -> push the ~28 GiB WS to SSD
#   OFFLOAD_BLOCK_SIZE= ./run-serve-certus-shmq-gqa-iobound.sh  # offload at the GPU block size (16)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b}"   # matches run-guidellm-gqa-iobound.sh MODEL
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"                # native window; no YaRN
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"                # 7B fits one A100-40G
GPU="${GPU:-0}"                                        # pin one GPU (TP=1); GPU=all exposes both
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}"
# No KV cap by default: ~20G/util pool >> ~1.75G one-sequence floor at 32768, and any
# I/O is driven by oversubscription, not a cap. KV_CACHE_BYTES=4G to cap and spill more.
KV_CACHE_BYTES="${KV_CACHE_BYTES-}"
OFFLOAD_BLOCK_SIZE="${OFFLOAD_BLOCK_SIZE-}"          # must stay empty: the connector offloads one GPU block per key
# Pin vLLM 0.30 to match the cputier arm (run-serve-cputier-gqa-iobound.sh) for a
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
    echo "[gqa-iobound] WARNING: no running certus-server found — start it with --memory-tier-size ${CPU_TIER_GB}G, e.g." >&2
    echo "[gqa-iobound]          MEMORY_TIER_SIZE=${CPU_TIER_GB}G CHANNELS=32 ./start-certus-optimized.sh" >&2
  else
    tier="$(sed -n 's/.*--memory-tier-size[ =]\([^ ]*\).*/\1/p' <<<"$srv_cmd")"
    if [[ "${tier^^}" != "${CPU_TIER_GB}G" ]]; then
      echo "[gqa-iobound] WARNING: certus-server memory tier is '${tier:-default}', the fair-A/B value is CPU_TIER_GB=${CPU_TIER_GB}G (= the cputier CPU tier)." >&2
      echo "[gqa-iobound]          restart it: MEMORY_TIER_SIZE=${CPU_TIER_GB}G CHANNELS=32 ./start-certus-optimized.sh  (or set ACK_MEM=1 to ignore)" >&2
    else
      echo "[gqa-iobound] certus-server memory tier ${tier} == CPU_TIER_GB (matches the cputier arm)"
    fi
  fi
fi

echo "[gqa-iobound] Qwen2.5-7B-Instruct (GQA, 56 KiB/token) TP=${TENSOR_PARALLEL} GPU=${GPU} window=${MAX_MODEL_LEN} offload-block=${OFFLOAD_BLOCK_SIZE:-gpu} KV=${KV_CACHE_BYTES:-util${GPU_MEM_UTIL}} DRAM-tier=${CPU_TIER_GB}G (certus-server)"

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
