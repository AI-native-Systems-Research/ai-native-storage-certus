#!/bin/bash
# run-serve-certus-shmq-50pt32g.sh — SERVER side of the "50pt@32G" experiment,
# Certus-SHMQ backend. Pairs with run-guidellm-50pt32g.sh. Thin wrapper over
# run-serve-certus-shmq.sh.
#
# Parameters come from the dram_dashboard preset
#   ~/certus/tools/kv-cache-dynamics/dram-dashboard-config-50pt@32G.yaml
# mapped onto vLLM + certus-server as follows:
#
#   preset key                      -> here
#   layers 48 / kvheads 8 / hd 128  -> Qwen2.5-14B-Instruct (192 KiB KV/token, fp16 bpe 2)
#   hbm 10 (GB, total GPU KV)       -> KV_CACHE_BYTES=5G per worker x TP=2
#   dram 32 (the "@32G" point)      -> certus-server --memory-tier-size 32G (NOT set
#                                      here — checked against the running server below)
#   ssd 20000 (≈ unbounded)         -> the certus-server's NVMe devices
#   mml 500000                      -> MAX_MODEL_LEN=49152 (see DEVIATION)
#   block 64                        -> OFFLOAD_BLOCK_SIZE=64: offloaded blocks are 64
#                                      tokens (6 MiB per TP worker for the 14B, vs
#                                      1.5 MiB at the default 16). GPU prefix caching
#                                      keeps vLLM's 16-token block.
#
# DEVIATION — the window: vLLM requires ONE max_model_len sequence to fit in GPU KV.
# At 192 KiB/token a 500000 window needs ~92 GiB of GPU KV, but hbm=10G holds only
# ~54.6K tokens (and 2x A100-40G tops out at ~240K anyway). We keep hbm faithful
# (it sets the HBM:DRAM ratio that defines the 32G point) and shrink the window to
# 49152, just under the 10G pool with headroom. The client clamps conversation
# depth to fit (see run-guidellm-50pt32g.sh). For a larger window instead (HBM no
# longer 10G):
#   MAX_MODEL_LEN=131072 KV_CACHE_BYTES=12G ./run-serve-certus-shmq-50pt32g.sh
#
# PREREQUISITE: a host certus-server publishing the mailbox at SHM_PATH with a 32G
# memory tier, e.g.  MEMORY_TIER_SIZE=32G CHANNELS=32 ./start-certus-optimized.sh
#
#   ./run-serve-certus-shmq-50pt32g.sh                     # 14B @ 49152, 10G GPU KV, 64-token offload blocks
#   OFFLOAD_BLOCK_SIZE= ./run-serve-certus-shmq-50pt32g.sh # offload at the GPU block size (16)
#   DRAM_GB=64 ./run-serve-certus-shmq-50pt32g.sh          # expect a 64G server tier (other sweep point)
#   ACK_DRAM=1 ./run-serve-certus-shmq-50pt32g.sh          # skip the server tier-size check
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-14b}"   # matches run-guidellm-50pt32g.sh MODEL
MAX_MODEL_LEN="${MAX_MODEL_LEN:-49152}"
# TP=2: the 14B weights (~29G) plus KV do not fit one 40G A100.
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}"
# hbm=10 is TOTAL GPU KV; KV_CACHE_BYTES is PER WORKER, so 10G / TP=2 = 5G.
KV_CACHE_BYTES="${KV_CACHE_BYTES-5G}"
OFFLOAD_BLOCK_SIZE="${OFFLOAD_BLOCK_SIZE-}"
DRAM_GB="${DRAM_GB:-32}"                                # expected certus-server memory tier
# Bound the prefill chunk so activation memory stays flat on long prompts.
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:---max-num-batched-tokens 8192}"

# ── Check the running certus-server's memory tier matches the experiment's DRAM ──
# The DRAM tier lives in the external certus-server, so this script cannot set it;
# read --memory-tier-size off its command line and warn on a mismatch.
if [[ "${ACK_DRAM:-0}" == "0" ]]; then
  srv_cmd="$(pgrep -a -f 'certus-server.*--shm-path' | head -1 || true)"
  if [[ -z "$srv_cmd" ]]; then
    echo "[50pt32g] WARNING: no running certus-server found (start it first with --memory-tier-size ${DRAM_GB}G)" >&2
  else
    tier="$(sed -n 's/.*--memory-tier-size[ =]\([^ ]*\).*/\1/p' <<<"$srv_cmd")"
    if [[ "${tier^^}" != "${DRAM_GB}G" ]]; then
      echo "[50pt32g] WARNING: certus-server memory tier is '${tier:-default}', experiment expects ${DRAM_GB}G" >&2
    else
      echo "[50pt32g] certus-server memory tier ${tier} (matches DRAM_GB=${DRAM_GB})"
    fi
  fi
fi

echo "[50pt32g] HBM ${KV_CACHE_BYTES:-uncapped}/worker x TP=${TENSOR_PARALLEL}, window ${MAX_MODEL_LEN}, offload block ${OFFLOAD_BLOCK_SIZE:-gpu} tok, DRAM ${DRAM_GB}G (certus-server)"

# Hand off via `env` so the optional vars can be conditionally included: a
# `${VAR:+VAR=val}` env ARG either passes the assignment or vanishes. Image stays at
# the base default (vLLM 0.28): 0.29's LBHNC layout exhausts SHMQ channels under load.
exec env \
  MODEL="$MODEL" \
  SERVED_MODEL_NAME="$SERVED_MODEL_NAME" \
  MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  TENSOR_PARALLEL="$TENSOR_PARALLEL" \
  GPU="${GPU:-all}" \
  GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}" \
  EXTRA_SERVE_ARGS="$EXTRA_SERVE_ARGS" \
  ${KV_CACHE_BYTES:+KV_CACHE_BYTES="$KV_CACHE_BYTES"} \
  ${OFFLOAD_BLOCK_SIZE:+OFFLOAD_BLOCK_SIZE="$OFFLOAD_BLOCK_SIZE"} \
  "${SCRIPT_DIR}/run-serve-certus-shmq.sh"
