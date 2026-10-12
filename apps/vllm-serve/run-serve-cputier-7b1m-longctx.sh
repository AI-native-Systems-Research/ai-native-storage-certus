#!/bin/bash
# run-serve-cputier-7b1m-longctx.sh — serve Qwen2.5-7B-Instruct-1M at a long window
# (default 524288) with vLLM native CPU+fs KV offload. One command; matches the
# run-guidellm-synthetic-longctx.sh client defaults (model name qwen2.5-7b-1m).
#
# WHY A DENSE REPACK: the stock 7B-1M reaches 1M via dual-chunk attention (DCA) +
# a sparse-attention config, but this cputier image's vLLM 0.29.0 DCA path crashes
# at worker init (FlashAttentionImpl.__init__ got an unexpected kwarg 'layer_idx'),
# and the sparse config — which re-downloads under HF_HUB_OFFLINE=0 — requires DCA
# (verify_dual_chunk_attention_config then indexes a None). So we serve a LOCAL
# repack with dual_chunk_attention_config removed and NO sparse config: plain dense
# attention, loaded by local path so the hub can't re-add either file.
#
# QUALITY CAVEAT: dense attention is quality-correct to the model's native 262144
# (its long context comes from rope_theta=1e7, base window 262144). Between 262144
# and the 524288 admission ceiling, attention quality degrades (DCA is what makes
# 262K-1M work). The synthetic conversations are ~41K tokens, far under 262K, so
# this does not bite in practice. For full-quality >262K you need a vLLM build with
# working DCA (not this image).
#
#   ./run-serve-cputier-7b1m-longctx.sh                     # 7B-1M at 524288, served as qwen2.5-7b-1m
#   MAX_MODEL_LEN=262144 ./run-serve-cputier-7b1m-longctx.sh   # stay within the native (full-quality) window
#   PORT=9000 ./run-serve-cputier-7b1m-longctx.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-524288}"
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b-1m}"
DENSE_DIR_NAME="${DENSE_DIR_NAME:-qwen7b1m-dense}"   # lives under HF_CACHE (mounted at /root/.cache/huggingface)
# Cap GPU KV (per worker) so the reused working set spills to the CPU/fs offload
# tiers — the 7B-1M's uncapped ~1M-token pool otherwise holds the whole working set
# and the tier read path never fires (0% external). 16G is the smallest cap that
# reliably BOOTS at the 524K window (the 14G theoretical floor sits right on vLLM's
# concurrency>=1 edge and gets rejected): it yields a ~599K-token pool (~1.14x
# headroom), well below the client's PREFIX_COUNT=96 reused set, so cold prefixes
# evict and reload. KV_CACHE_BYTES= (empty) removes the cap (derive from
# GPU_MEM_UTIL); the window's floor scales with MAX_MODEL_LEN (preflight guard warns).
KV_CACHE_BYTES="${KV_CACHE_BYTES-16G}"

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
c.pop("dual_chunk_attention_config", None)   # the DCA trigger for vLLM 0.29.0's layer_idx crash
json.dump(c, open(sys.argv[2], "w"), indent=2)
PY
else
  echo "[7b1m] reusing dense repack at ${dense}"
fi

# Container path to the repack (HF_CACHE is bind-mounted at /root/.cache/huggingface).
MODEL_CTR="/root/.cache/huggingface/${DENSE_DIR_NAME}"

# Hand off to the generic long-context cputier wrapper. ROPE_YARN=0 (the repack is
# dense native 262144 via rope_theta, no YaRN); KV/weights set so the preflight is
# accurate for the 7B (56 KB/token, ~15 GB fp16 weights).
exec env \
  HF_CACHE="$HF_CACHE" \
  MODEL="$MODEL_CTR" \
  SERVED_MODEL_NAME="$SERVED_MODEL_NAME" \
  MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  ROPE_YARN=0 \
  KV_BYTES_PER_TOKEN=$((56 * 1024)) \
  WEIGHTS_GB=15 \
  ${KV_CACHE_BYTES:+KV_CACHE_BYTES="$KV_CACHE_BYTES"} \
  "${SCRIPT_DIR}/run-serve-cputier-longctx.sh"
