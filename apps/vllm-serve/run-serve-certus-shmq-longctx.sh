#!/bin/bash
# run-serve-certus-shmq-longctx.sh — SERVER side of the long-context KV-offload test,
# Certus-SHMQ backend. Pairs with run-guidellm-synthetic-longctx.sh.
#
# The Certus-SHMQ counterpart of run-serve-cputier-longctx.sh: same model, endpoint,
# window and tier-stress defaults, but the KV offload tier is the EXTERNAL
# certus-server (CUDA IPC over a /dev/shm mailbox) instead of vLLM's in-process
# CPU+fs tiering. Thin wrapper over run-serve-certus-shmq.sh.
#
# PREREQUISITE (unlike cputier, this is NOT self-contained): a host certus-server
# must already be running and publishing the mailbox at SHM_PATH before you start
# this. See run-serve-certus-shmq.sh's header for the full server bring-up (vfio-pci
# + hugepages, then target/release/certus-server --shm-path /dev/shm/certus-shmq …).
# SLAB_SIZE_BYTES MUST match the certus-server's block size.
#
# DEFAULT WINDOW is 131072 — the largest that reliably boots on this box's 2x
# A100-40G (a true 1,000,000 window needs ~197 GB of GPU KV for one sequence and
# is rejected at startup here; see the KV preflight and the note below). Request a
# larger window explicitly; the preflight warns if it won't fit.
#
#   ./run-serve-certus-shmq-longctx.sh                  # 14B at 131072, 2x A100, 4G GPU-KV cap -> SHMQ tier
#   MAX_MODEL_LEN=200000 ./run-serve-certus-shmq-longctx.sh    # push the window (watch the KV preflight)
#   MAX_MODEL_LEN=1000000 ./run-serve-certus-shmq-longctx.sh   # true 1M — only on hardware with ~197 GB GPU KV
#   KV_CACHE_BYTES= ./run-serve-certus-shmq-longctx.sh         # no GPU-KV cap (derive from GPU_MEM_UTIL)
#   USE_1M_MODEL=1 ./run-serve-certus-shmq-longctx.sh          # native Qwen2.5-14B-Instruct-1M (no YaRN; downloads)
#   SHM_PATH=/dev/shm/certus-shmq SLAB_SIZE_BYTES=2097152 ./run-serve-certus-shmq-longctx.sh
#
# ── READ THIS: a true 1M window does NOT fit 2x A100-40G with the 14B ───────────
# vLLM requires one max_model_len sequence's KV to fit in GPU KV cache (the SHMQ
# OffloadingConnector extends cross-request REUSE — evicted prefixes demote to the
# certus-server tier and promote back — it does NOT shrink a single live sequence).
# For Qwen2.5-14B the KV footprint is ~192 KB/token, so:
#
#     1,000,000 tokens  ->  ~197 GB of ACTIVE GPU KV for ONE sequence
#
# Two A100-40G at TP=2 give ~48 GB of KV after the ~29 GB of fp16 weights, i.e.
# room for only ~240K tokens. The preflight below estimates your box's ceiling and
# WARNS when the requested window exceeds it; left as 1,000,000 on this hardware,
# vLLM will reject startup with "max seq len larger than KV cache". Your realistic
# options:
#   1. Lower MAX_MODEL_LEN to what fits (the preflight prints the number) — this is
#      the only path that serves on 2x A100-40G today. The 1M client workload still
#      exercises the SHMQ tier; deep conversations just slide at the window.
#   2. Bring enough GPU KV for ~197 GB (e.g. 8x A100-80G / H100s) and keep 1M.
#   3. USE_1M_MODEL=1 for the native Qwen2.5-*-1M checkpoint — but note dual-chunk
#      attention still STORES full KV, so it needs the same ~197 GB; it buys long-
#      context quality (no factor-30 YaRN), not single-GPU-box feasibility.
#
# YaRN caveat (stock model path): a 1M window on the native-32K 14B auto-enables
# YaRN at factor ~30.5 — far beyond Qwen's validated factor-4 (131K) recipe, so
# long-context quality degrades. Fine for KV-offload throughput/latency work;
# not for measuring model correctness. USE_1M_MODEL=1 avoids the YaRN stretch.
#
# NOTE on the connector image: the base defaults to the 0.28 connector; 0.29 is
# avoided for SHMQ (its LBHNC KV layout breaks the offload store path under load,
# leaving external hits ~0). Override IMAGE= only if you know your build is clean.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── Model (stock 14B + YaRN, or the native -1M checkpoint) ───────────────────────
if [[ "${USE_1M_MODEL:-0}" != "0" ]]; then
  MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct-1M}"
  ROPE_YARN="${ROPE_YARN:-0}"        # -1M config is natively 1M; no YaRN stretch
  echo "[serve-longctx] using native 1M checkpoint ${MODEL} (ROPE_YARN=${ROPE_YARN}); not in cache -> first run downloads"
else
  MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct}"   # native 32K; YaRN stretched to the window below
fi
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-14b}"   # matches run-guidellm-synthetic-longctx.sh default MODEL
# Default to the largest window that boots on 2x A100-40G (see header). Override up
# to your GPU KV ceiling (the preflight prints it) or to 1000000 on bigger hardware.
MAX_MODEL_LEN="${MAX_MODEL_LEN:-131072}"
# The 14B weights live in $HOME/.cache/huggingface on this box; the base serve
# script defaults HF_CACHE to /mnt/certus1/hf-cache (where they are NOT), which
# would trigger a 29 GB re-download. Point at the local copy by default.
HF_CACHE="${HF_CACHE:-$HOME/.cache/huggingface}"

# ── Sharding / GPU ───────────────────────────────────────────────────────────────
# TP=2: a single large sequence's KV + the 14B weights do not fit one 40G A100;
# shard across both so the KV pool is as large as this box can offer. GPU=all makes
# both devices visible (sharding itself needs the TP flag).
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}"
GPU="${GPU:-all}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.9}"
# OPTIONAL hard cap on the GPU KV cache, applied PER WORKER (overrides
# GPU_MEM_UTIL for KV sizing). UNSET by default: the full ~20G/worker pool boots at
# 131072 and the SHMQ offload tier is driven by the client's PREFIX_COUNT=32 (the
# proven 22.6%-external-hit path). A cap only shrinks GPU KV for extra spill — but
# vLLM still requires ONE max_model_len sequence to fit in GPU, so at 131072 (TP=2)
# the floor is ~12G/worker; a 4G/8G cap does NOT boot there (it needs a smaller
# window — 4G->~43K, 8G->~87K tokens). The preflight below catches this. To cap at
# 131072 use 12G-20G/worker; for an aggressive 4G/8G cap also lower MAX_MODEL_LEN.
KV_CACHE_BYTES="${KV_CACHE_BYTES-}"

# ── Certus-SHMQ offload tier (external certus-server) ────────────────────────────
# Unlike cputier there is no in-process CPU/fs tier to size here — the offload tier
# IS the host certus-server reached over the mailbox. These must match the server
# it is already running with; left unset they take the base script's defaults.
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"   # offload block size — MUST match certus-server

# ── Chunked prefill ───────────────────────────────────────────────────────────────
# A million-token prompt cannot be prefilled in one batch; bound the prefill chunk
# so activation memory stays flat (vLLM slices the prompt across many steps).
# Overridable; the base appends EXTRA_SERVE_ARGS last.
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:---max-num-batched-tokens 8192}"

# ── Preflight: does one max_model_len sequence's KV fit in GPU KV cache? ──────────
# Mirrors vLLM's own startup check so the failure mode is explained up front rather
# than surfacing as an opaque engine abort. Non-fatal (your GPUs/model may differ);
# set ACK_KV=1 to silence. KV/token and weight size are the 14B defaults — override
# KV_BYTES_PER_TOKEN / WEIGHTS_GB for another model.
KV_BYTES_PER_TOKEN="${KV_BYTES_PER_TOKEN:-$((192 * 1024))}"   # 14B: 48L x 8 kv-heads x 128 x 2(KV) x 2B
WEIGHTS_GB="${WEIGHTS_GB:-29}"                                # ~14.7B params at fp16
if [[ "${ACK_KV:-0}" == "0" ]] && command -v nvidia-smi >/dev/null 2>&1; then
  gpu_total_mib="$(nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits 2>/dev/null | awk '{s+=$1} END{print s+0}')"
  if [[ -n "$gpu_total_mib" && "$gpu_total_mib" -gt 0 ]]; then
    python3 -I - "$gpu_total_mib" "$GPU_MEM_UTIL" "$WEIGHTS_GB" "$KV_BYTES_PER_TOKEN" "$MAX_MODEL_LEN" "$TENSOR_PARALLEL" "${KV_CACHE_BYTES:-}" <<'PY'
import sys, math
gpu_mib, util, weights_gb, kv_per_tok, max_len, tp, kv_cap = sys.argv[1:8]
kv_per_tok = int(kv_per_tok); max_len = int(max_len); tp = int(tp)
total = float(gpu_mib) * (1 << 20)                 # bytes across all visible GPUs
usable = total * float(util)
kv_bytes = usable - float(weights_gb) * (1 << 30)  # KV pool after weights (TP shards both)
fit_tokens = int(kv_bytes / kv_per_tok) if kv_bytes > 0 else 0
need_gb = max_len * kv_per_tok / (1 << 30)
err = sys.stderr
print(f"[serve-longctx] GPU KV ceiling: ~{fit_tokens:,} tokens "
      f"(~{kv_bytes/(1<<30):.0f}G KV after {weights_gb}G weights, util {util})", file=err)
if max_len > fit_tokens:
    print(f"[serve-longctx] WARNING: requested MAX_MODEL_LEN={max_len:,} needs ~{need_gb:.0f}G of GPU KV "
          f"for ONE sequence but only ~{fit_tokens:,} tokens fit — vLLM will REJECT startup.", file=err)
    print(f"[serve-longctx]          -> set MAX_MODEL_LEN={fit_tokens} to serve on this box, "
          f"or add GPUs / use fewer-KV hardware. See the header.", file=err)

# Optional KV cap check: KV_CACHE_BYTES is PER WORKER, so the pool is cap x TP.
# vLLM still needs one max_model_len sequence to fit, so cap_tokens must be >= max_len.
def _parse(s):
    s = s.strip()
    if not s:
        return None
    u = s[-1].upper()
    m = {"G": 1 << 30, "M": 1 << 20, "K": 1 << 10}.get(u)
    return float(s[:-1]) * m if m else float(s)
cap = _parse(kv_cap)
if cap is not None:
    cap_tokens = int(cap * tp / kv_per_tok)
    floor_gb = math.ceil(max_len * kv_per_tok / tp / (1 << 30))
    if cap_tokens < max_len:
        print(f"[serve-longctx] WARNING: KV_CACHE_BYTES caps GPU KV to ~{cap_tokens:,} tokens "
              f"(per-worker x TP={tp}) < MAX_MODEL_LEN={max_len:,} — vLLM will REJECT startup.", file=err)
        print(f"[serve-longctx]          -> raise KV_CACHE_BYTES to >= {floor_gb}G (per worker), "
              f"or set MAX_MODEL_LEN <= {cap_tokens}.", file=err)
PY
  fi
fi

echo "[serve-longctx] serving ${MODEL} as '${SERVED_MODEL_NAME}' at max_model_len=${MAX_MODEL_LEN} (TP=${TENSOR_PARALLEL}) via SHMQ ${SHM_PATH}"

# Hand off via `env` so the optional vars (KV_CACHE_BYTES, ROPE_YARN) can be
# conditionally included: as `env` ARGS a `${VAR:+VAR=val}` expansion either passes
# the assignment or vanishes — unlike a shell assignment prefix, where the expansion
# would be mis-parsed as a command. Empty KV_CACHE_BYTES => omitted => base derives
# KV size from GPU_MEM_UTIL; absent ROPE_YARN => base default (YaRN on).
exec env \
  MODEL="$MODEL" \
  SERVED_MODEL_NAME="$SERVED_MODEL_NAME" \
  MAX_MODEL_LEN="$MAX_MODEL_LEN" \
  TENSOR_PARALLEL="$TENSOR_PARALLEL" \
  GPU="$GPU" \
  GPU_MEM_UTIL="$GPU_MEM_UTIL" \
  HF_CACHE="$HF_CACHE" \
  SHM_PATH="$SHM_PATH" \
  SLAB_SIZE_BYTES="$SLAB_SIZE_BYTES" \
  EXTRA_SERVE_ARGS="$EXTRA_SERVE_ARGS" \
  ${KV_CACHE_BYTES:+KV_CACHE_BYTES="$KV_CACHE_BYTES"} \
  ${ROPE_YARN:+ROPE_YARN="$ROPE_YARN"} \
  "${SCRIPT_DIR}/run-serve-certus-shmq.sh"
