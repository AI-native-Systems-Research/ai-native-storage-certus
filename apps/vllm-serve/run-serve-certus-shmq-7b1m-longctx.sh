#!/bin/bash
# run-serve-certus-shmq-7b1m-longctx.sh — serve Qwen2.5-7B-Instruct-1M at a long
# window (default 524288) over the Certus-SHMQ KV-offload connector. One command;
# matches the run-guidellm-synthetic-longctx.sh client defaults (model qwen2.5-7b-1m).
#
# The Certus-SHMQ counterpart of run-serve-cputier-7b1m-longctx.sh: identical model
# prep (dense repack) and window/cap, but the KV offload tier is the EXTERNAL
# certus-server (CUDA IPC over a /dev/shm mailbox) instead of vLLM's in-process
# CPU+fs tiering. Hands off to run-serve-certus-shmq-longctx.sh.
#
# PREREQUISITE (unlike cputier, NOT self-contained): a host certus-server must
# already be running and publishing the mailbox at SHM_PATH before you start this
# (vfio-pci + hugepages, then target/release/certus-server --shm-path …). See
# run-serve-certus-shmq.sh's header for the full bring-up. SLAB_SIZE_BYTES must
# match the certus-server's block size.
#
# WHY A DENSE REPACK (same reason as the cputier wrapper): the stock 7B-1M reaches
# 1M via dual-chunk attention (DCA) + a sparse-attention config, but the connector
# image's vLLM crashes on the DCA path (FlashAttentionImpl got unexpected kwarg
# 'layer_idx'), and the sparse config — re-downloaded under HF_HUB_OFFLINE=0 —
# requires DCA. So we serve a LOCAL repack with dual_chunk_attention_config removed
# and NO sparse config: plain dense attention, loaded by local path. The repack is
# shared with the cputier wrapper (same ${HF_CACHE}/${DENSE_DIR_NAME}).
#
# QUALITY CAVEAT: dense attention is quality-correct to the model's native 262144
# (rope_theta=1e7, base window 262144); 262K-524K is admitted but degraded (DCA is
# what makes 262K-1M work). The synthetic conversations are ~41K tokens, far under.
#
# IMAGE: clamped to the vLLM 0.29 connector (certus-shmq-connector:vllm0.29.0) by
# default, for parity with the cputier wrapper (certus-offload:vllm0.29.0). The dense
# repack is version-agnostic (plain Qwen2), so it loads on 0.29.
# CAVEAT: vLLM 0.29's LBHNC KV layout can exhaust the SHMQ offload channels under
# concurrent load (RingError / EngineDeadError, external hits -> ~0). Run the host
# certus-server with a higher channel count (--channels 32+) and watch /metrics; if
# it still misbehaves, fall back with IMAGE=localhost/certus-shmq-connector:vllm0.28.0.
#
#   ./run-serve-certus-shmq-7b1m-longctx.sh                 # 7B-1M @ 524288, 16G GPU-KV cap -> SHMQ tier
#   MAX_MODEL_LEN=262144 ./run-serve-certus-shmq-7b1m-longctx.sh   # within native (full-quality) window
#   KV_CACHE_BYTES= ./run-serve-certus-shmq-7b1m-longctx.sh        # no GPU-KV cap
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-524288}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b-1m}"
DENSE_DIR_NAME="${DENSE_DIR_NAME:-qwen7b1m-dense}"   # under HF_CACHE (mounted at /root/.cache/huggingface)
# Cap GPU KV (per worker) so the reused working set spills to the SHMQ offload tier
# (the external certus-server). 16G is the smallest cap that reliably boots at the
# 524K window (the 14G theoretical floor sits on vLLM's concurrency>=1 edge and is
# rejected): ~599K-token pool, well below the client's PREFIX_COUNT=96 reused set.
# KV_CACHE_BYTES= (empty) removes the cap (derive from GPU_MEM_UTIL).
KV_CACHE_BYTES="${KV_CACHE_BYTES-16G}"
# Clamp the connector image to vLLM 0.29 by default (base defaults to 0.28); see the
# IMAGE caveat in the header. Override IMAGE= to pin another build.
IMAGE="${IMAGE:-localhost/certus-shmq-connector:vllm0.29.0}"

# ── Ensure the dense repack exists (build from the cached -1M snapshot) ───────────
snap="$(find "${HF_CACHE}/hub/models--Qwen--Qwen2.5-7B-Instruct-1M/snapshots" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | head -1)"
if [[ -z "$snap" ]]; then
  echo "error: Qwen2.5-7B-Instruct-1M not found under ${HF_CACHE}/hub — download it first:" >&2
  echo "  HF_HOME=${HF_CACHE} huggingface-cli download Qwen/Qwen2.5-7B-Instruct-1M" >&2
  exit 1
fi
dense="${HF_CACHE}/${DENSE_DIR_NAME}"
if [[ ! -f "${dense}/config.json" ]]; then
  echo "[7b1m] building dense repack at ${dense} (DCA removed, no sparse config)"
  rm -rf "$dense"; mkdir -p "$dense"
  snaphash="$(basename "$snap")"
  for f in "$snap"/*; do
    b="$(basename "$f")"
    case "$b" in config.json|sparse_attention_config.json) continue ;; esac
    # Relative symlink so it resolves on the host AND inside the container mount.
    ln -s "../hub/models--Qwen--Qwen2.5-7B-Instruct-1M/snapshots/${snaphash}/${b}" "${dense}/${b}"
  done
  python3 -I - "$snap/config.json" "$dense/config.json" <<'PY'
import json, sys
c = json.load(open(sys.argv[1]))
c.pop("dual_chunk_attention_config", None)   # the DCA trigger for the vLLM layer_idx crash
json.dump(c, open(sys.argv[2], "w"), indent=2)
PY
else
  echo "[7b1m] reusing dense repack at ${dense}"
fi

# Container path to the repack (HF_CACHE is bind-mounted at /root/.cache/huggingface).
MODEL_CTR="/root/.cache/huggingface/${DENSE_DIR_NAME}"

# Hand off to the generic SHMQ long-context wrapper. ROPE_YARN=0 (dense native 262144
# via rope_theta, no YaRN); KV/weights set so the preflight is accurate for the 7B.
exec env \
  HF_CACHE="$HF_CACHE" \
  MODEL="$MODEL_CTR" \
  SERVED_MODEL_NAME="$SERVED_MODEL_NAME" \
  MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  ROPE_YARN=0 \
  KV_BYTES_PER_TOKEN=$((56 * 1024)) \
  WEIGHTS_GB=15 \
  IMAGE="$IMAGE" \
  ${KV_CACHE_BYTES:+KV_CACHE_BYTES="$KV_CACHE_BYTES"} \
  "${SCRIPT_DIR}/run-serve-certus-shmq-longctx.sh"
