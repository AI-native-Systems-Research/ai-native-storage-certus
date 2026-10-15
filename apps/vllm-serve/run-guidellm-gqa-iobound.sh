#!/bin/bash
# run-guidellm-gqa-iobound.sh — GQA counterpart of run-guidellm-mha-iobound.sh.
# CLIENT side of the KV-IO test. Pairs with run-serve-certus-shmq-gqa-iobound.sh
# (Qwen2.5-7B-Instruct, GQA) or its cputier counterpart. Thin wrapper over
# run-guidellm.sh.
#
# SAME CLIENT WORKLOAD as the MHA arm — identical prefix/turn/output shape — so the
# only difference between the two experiments is the served model's attention type
# (MHA Qwen1.5-7B @ 512 KiB/token vs GQA Qwen2.5-7B @ 56 KiB/token). Running the
# identical levers against the GQA model is the experiment: it measures how much
# GQA's ~9x KV compression relieves the KV-I/O bottleneck.
#
# The I/O-bound levers (all set to push toward I/O-bound, same as the MHA arm):
#
#   * BIG shared prefixes (PREFIX_TOKENS=8192), MANY of them (PREFIX_COUNT=64), so
#     the reused working set is
#         64 x 8192 tok x 56 KiB/tok  =  ~28 GiB of prefix KV
#     (vs ~256 GiB on the MHA arm — GQA stores 1/9th the bytes for the same tokens).
#   * MANY TURNS (TURNS=128): each resumed conversation reloads its whole
#     accumulated chain (prefix + all prior turns) from the tier.
#   * ONE OUTPUT TOKEN (OUTPUT_TOKENS=1) + small new prompt (PROMPT_TOKENS=128) so
#     each turn is essentially prefill-only — a pure prefix lookup. Decode (HBM-
#     bandwidth bound, not tier I/O) is removed almost entirely. Raise OUTPUT_TOKENS
#     to add decode back.
#   * CLOSED-LOOP concurrent streams with ~zero think time, so the server stays
#     saturated and oversubscription forces continuous evict+reload.
#
# ── NOTE ON "I/O-BOUND" FOR GQA ───────────────────────────────────────────────────
# Because GQA's working set at these defaults is only ~28 GiB, it FITS a default
# 32 G DRAM/CPU tier — so the default run is DRAM-resident, NOT SSD-bound. That is
# the honest apples-to-apples result (same workload, 9x less KV). To reproduce the
# MHA arm's SSD-read regime at matched BYTE volume (~224 GiB working set), raise
# PREFIX_COUNT to ~512; or keep PREFIX_COUNT=64 and shrink the tier (CPU_TIER_GB on
# the serve script, same value on both backends).
#
# WINDOW: Qwen2.5-7B is native 32768 (same as the MHA arm's Qwen1.5-7B). The defaults
# fit a deep conversation inside it: 8192 prefix + 128 turns x (128+1) < 32768.
# Raising TURNS or the per-turn sizes past the window makes deep turns 400 (slide).
#
# Metrics to confirm the I/O profile (subtract the BEFORE/AFTER /metrics snapshots
# run-guidellm.sh writes): vllm:kv_offload_total_bytes_total{CPU_to_GPU} (bytes
# reloaded) and kv_offload_total_time_total; external_prefix_cache_hits/queries
# shows the tier hit rate. Expect far fewer reloaded bytes than the MHA arm here.
#
#   ./run-guidellm-gqa-iobound.sh                           # 8 streams, 1000 s, ~28 GiB working set
#   PREFIX_COUNT=512 ./run-guidellm-gqa-iobound.sh          # byte-match the MHA arm (~224 GiB -> SSD)
#   OUTPUT_TOKENS=32 ./run-guidellm-gqa-iobound.sh          # add some decode back (less prefill-pure)
#   CONCURRENT_STREAMS=16 ./run-guidellm-gqa-iobound.sh     # heavier oversubscription
#   MAX_SECONDS=600 ./run-guidellm-gqa-iobound.sh           # shorter measurement window
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

CONTEXT_WINDOW="${CONTEXT_WINDOW:-32768}"   # server window (served-side); for the fit check only
PREFIX_TOKENS="${PREFIX_TOKENS:-8192}"
PREFIX_COUNT="${PREFIX_COUNT:-64}"
PROMPT_TOKENS="${PROMPT_TOKENS:-128}"
OUTPUT_TOKENS="${OUTPUT_TOKENS:-1}"         # 1 token => prefill-only turns (emulate pure prefix reuse)
TURNS="${TURNS:-128}"
THINK_S="${THINK_S:-0.001}"                 # constant think time; must be > 0 (guidellm drops <=0)
KV_PER_TOKEN_KIB="${KV_PER_TOKEN_KIB:-56}"  # Qwen2.5-7B GQA (4 KV heads), fp16 — working-set report only

EST_CTX=$(( PREFIX_TOKENS + TURNS * (PROMPT_TOKENS + OUTPUT_TOKENS) ))
if (( EST_CTX > CONTEXT_WINDOW )); then
  echo "[gqa-iobound] NOTE: est. accumulated context ~${EST_CTX} tok > CONTEXT_WINDOW=${CONTEXT_WINDOW}; deep turns will 400 / slide" >&2
fi

# Reused prefix working set in GiB (informational): prefix_count x prefix_tokens x KV/token.
WORKING_SET_GIB=$(awk "BEGIN{printf \"%.0f\", ${PREFIX_COUNT}*${PREFIX_TOKENS}*${KV_PER_TOKEN_KIB}/1048576}")
echo "[gqa-iobound] prefix=${PREFIX_TOKENS}x${PREFIX_COUNT} => ~${WORKING_SET_GIB} GiB reused KV (GQA 56 KiB/tok; fits a 32G tier at defaults — raise PREFIX_COUNT for SSD)"
echo "[gqa-iobound] turns=${TURNS} prompt=${PROMPT_TOKENS} output=${OUTPUT_TOKENS} think=${THINK_S}s est-accum-ctx~${EST_CTX}/${CONTEXT_WINDOW}"

# Comma form: run-guidellm.sh folds prefix_tokens/prefix_count into a prefix_buckets
# entry and passes the remaining numeric fields through to synthetic_text. No
# delay_stdev (guidellm requires it > 0 when set; without it the sampler draws
# uniform(delay_min, delay_max) = a constant THINK_S when the two are equal).
DATA_SPEC="prompt_tokens=${PROMPT_TOKENS},output_tokens=${OUTPUT_TOKENS},turns=${TURNS}"
DATA_SPEC+=",delay=${THINK_S},delay_min=${THINK_S},delay_max=${THINK_S}"
DATA_SPEC+=",prefix_tokens=${PREFIX_TOKENS},prefix_count=${PREFIX_COUNT}"

exec env \
  MODEL="${MODEL:-qwen2.5-7b}" \
  PROCESSOR="${PROCESSOR:-Qwen/Qwen2.5-7B-Instruct}" \
  PORT="${PORT:-8000}" \
  RATE_TYPE="${RATE_TYPE:-concurrent}" \
  CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-8}" \
  MAX_SECONDS="${MAX_SECONDS:-1000}" \
  DATA="$DATA_SPEC" \
  "${SCRIPT_DIR}/run-guidellm.sh"
