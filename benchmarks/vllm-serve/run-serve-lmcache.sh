#!/bin/bash
# run-serve-lmcache.sh — vLLM OpenAI-compatible API server wired to LMCache (MP
# server: pinned-RAM L1 written through to an SSD L2), for interactive /
# benchmarking clients (guidellm, aiperf, the OpenAI SDK, curl).
#
# The "lmcache" counterpart of run-serve-cputier.sh (vLLM-native CPU + fs tiering)
# and run-serve-certus-shmq.sh: same model, endpoint and engine flags, but the KV
# offload tiers live in an LMCache server container. Runs in the FOREGROUND like the
# sibling serve scripts (Ctrl-C stops vLLM and the LMCache server). The deployment
# itself — image, both containers, flags and why — is deploy/lmcache/run.sh; this
# script only maps the shared benchmark knobs onto it.
#
# Tier mapping vs cputier, for a fair A/B:
#   CPU_BYTES / CPU_GB      -> LMCache L1 (--l1-size-gb), same DRAM budget
#   FS_TIER_HOST            -> DISK_DIR (L2 in DISK_DIR/l2, fs_native, O_DIRECT)
#   OFFLOAD_BLOCK_SIZE      -> no equivalent; LMCache stores CHUNK_SIZE-token chunks
#                              (default 256). Set CHUNK_SIZE to match if you want.
# LMCache's L2 is WRITE-THROUGH (every stored chunk is also written to SSD); the
# cputier fs tier and Certus write to SSD on eviction. Note it when comparing.
#
#   ./run-serve-lmcache.sh                                  # Qwen2.5-7B, :8000, 8G L1 + 400G L2
#   CPU_GB=32 ./run-serve-lmcache.sh                        # bigger L1 (same as a 32G cputier/Certus tier)
#   DISK_DIR=/other/nvme DISK_GB=1000 ./run-serve-lmcache.sh
#   MAX_MODEL_LEN=131072 ./run-serve-lmcache.sh             # 128K window (YaRN on Qwen2.x)
#   KV_CACHE_BYTES=4G ./run-serve-lmcache.sh                # cap GPU KV; push reuse to LMCache
#
# Clients then use:
#   Base URL:  http://127.0.0.1:${PORT}/v1
#   Model:     ${SERVED_MODEL_NAME}
#   /metrics:  http://127.0.0.1:${PORT}/metrics   (vllm:external_prefix_cache_hits = LMCache hits)
#              http://127.0.0.1:${LMC_HTTP_PORT:-8080}/metrics  (lmcache_mp_l1_* / lmcache_mp_l2_*)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY_DIR="$(cd "${SCRIPT_DIR}/../../deploy/lmcache" && pwd)"

MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}"
GPU="${GPU:-0}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.90}"
TENSOR_PARALLEL="${TENSOR_PARALLEL:-1}"
IMAGE="${IMAGE:-localhost/vllm-lmcache-bench:vllm0.30.0}"

# CPU_BYTES (cputier's knob) wins if given, else CPU_GB.
if [[ -n "${CPU_BYTES:-}" ]]; then
  CPU_GB=$(( CPU_BYTES / (1 << 30) ))
fi
CPU_GB="${CPU_GB:-8}"
DISK_DIR="${DISK_DIR:-${FS_TIER_HOST:-/cache/lmcache}}"
DISK_GB="${DISK_GB:-400}"

if pgrep -f 'certus-server.*--shm-path' >/dev/null; then
  echo "[serve] WARNING: certus-server is still running — stop it (and free hugepages) so the ${CPU_GB}G L1 has RAM" >&2
fi
hp="$(awk '/^HugePages_Total:/{print $2}' /proc/meminfo)"
if [[ -n "${hp:-}" && "${hp}" -gt 0 ]]; then
  echo "warning: ${hp} hugepages reserved on the host — they reduce RAM available to the L1 tier." >&2
fi

exec env FOREGROUND=1 \
  IMAGE="$IMAGE" MODEL="$MODEL" SERVED_MODEL_NAME="$SERVED_MODEL_NAME" \
  MAX_MODEL_LEN="$MAX_MODEL_LEN" GPU="$GPU" GPU_MEM_UTIL="$GPU_MEM_UTIL" \
  TENSOR_PARALLEL="$TENSOR_PARALLEL" CPU_GB="$CPU_GB" DISK_DIR="$DISK_DIR" DISK_GB="$DISK_GB" \
  "${DEPLOY_DIR}/run.sh"
