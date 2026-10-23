#!/bin/bash
# run-guidellm-mha-iobound.sh — CLIENT side of the MHA KV-IO-bound test. Pairs with
# run-serve-certus-shmq-mha-iobound.sh (Qwen1.5-7B-Chat). Thin wrapper over
# run-guidellm.sh.
#
# GOAL: drive the server so that run time is dominated by KV-cache I/O (reloading
# evicted KV from the DRAM/SSD offload tiers) rather than by GPU compute. The
# levers, all set to push toward I/O-bound:
#
#   * BIG shared prefixes (PREFIX_TOKENS=8192), MANY of them (PREFIX_COUNT=64), so
#     the reused working set is
#         64 x 8192 tok x 512 KiB/tok  =  ~256 GiB of prefix KV
#     which massively oversubscribes GPU KV (~20 GB) and any DRAM tier — so reuse
#     hits are served from the SSD tier (real NVMe reads = the I/O under test).
#   * MANY TURNS (TURNS=128): each resumed conversation reloads its whole
#     accumulated chain (prefix + all prior turns) from the tier; by the deep turns
#     that is tens of GiB of KV read per single turn.
#   * ONE OUTPUT TOKEN (OUTPUT_TOKENS=1) + small new prompt (PROMPT_TOKENS=128) so
#     each turn is essentially prefill-only — it emulates a pure prefix lookup:
#     load/recompute the accumulated context, emit a single token, done. Decode
#     (HBM-bandwidth bound, not tier I/O) is removed almost entirely, so a turn's
#     time is dominated by the KV reload. Raise OUTPUT_TOKENS to add decode back.
#   * CLOSED-LOOP concurrent streams with ~zero think time (round-robin-like), so
#     the server stays saturated and oversubscription forces continuous evict+reload.
#
# WINDOW: Qwen1.5-7B is native 32768. The defaults fit a deep conversation inside
# it: 8192 prefix + 128 turns x (128+32) = 28672 tok < 32768. Raising TURNS or the
# per-turn sizes past the window makes deep turns 400 (and the server slides them).
#
# Metrics to confirm it IS I/O-bound (subtract the BEFORE/AFTER /metrics snapshots
# run-guidellm.sh writes): vllm:kv_offload_total_bytes_total{CPU_to_GPU} (bytes
# reloaded) and kv_offload_total_time_total; a high reload-byte rate and a large
# share of time in transfer = I/O-bound. external_prefix_cache_hits/queries shows
# the tier hit rate.
#
#   ./run-guidellm-mha-iobound.sh                           # 8 streams, 1000 s, ~256 GiB working set
#   PREFIX_COUNT=128 ./run-guidellm-mha-iobound.sh          # 2x working set (more SSD traffic)
#   OUTPUT_TOKENS=32 ./run-guidellm-mha-iobound.sh          # add some decode back (less prefill-pure)
#   CONCURRENT_STREAMS=16 ./run-guidellm-mha-iobound.sh     # heavier oversubscription
#   MAX_SECONDS=600 ./run-guidellm-mha-iobound.sh           # shorter measurement window
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

CONTEXT_WINDOW="${CONTEXT_WINDOW:-32768}"   # server window (served-side); for the fit check only
PREFIX_TOKENS="${PREFIX_TOKENS:-8192}"
PREFIX_COUNT="${PREFIX_COUNT:-64}"
PROMPT_TOKENS="${PROMPT_TOKENS:-128}"
OUTPUT_TOKENS="${OUTPUT_TOKENS:-1}"         # 1 token => prefill-only turns (emulate pure prefix reuse)
TURNS="${TURNS:-128}"
THINK_S="${THINK_S:-0.001}"                 # constant think time; must be > 0 (guidellm drops <=0)
KV_PER_TOKEN_KIB="${KV_PER_TOKEN_KIB:-512}" # Qwen1.5-7B MHA, fp16 — for the working-set report only

EST_CTX=$(( PREFIX_TOKENS + TURNS * (PROMPT_TOKENS + OUTPUT_TOKENS) ))
if (( EST_CTX > CONTEXT_WINDOW )); then
  echo "[mha-iobound] NOTE: est. accumulated context ~${EST_CTX} tok > CONTEXT_WINDOW=${CONTEXT_WINDOW}; deep turns will 400 / slide" >&2
fi

# Reused prefix working set in GiB (informational): prefix_count x prefix_tokens x KV/token.
WORKING_SET_GIB=$(awk "BEGIN{printf \"%.0f\", ${PREFIX_COUNT}*${PREFIX_TOKENS}*${KV_PER_TOKEN_KIB}/1048576}")
echo "[mha-iobound] prefix=${PREFIX_TOKENS}x${PREFIX_COUNT} => ~${WORKING_SET_GIB} GiB reused KV (oversubscribes GPU+DRAM -> SSD reads)"
echo "[mha-iobound] turns=${TURNS} prompt=${PROMPT_TOKENS} output=${OUTPUT_TOKENS} think=${THINK_S}s est-accum-ctx~${EST_CTX}/${CONTEXT_WINDOW}"

# Comma form: run-guidellm.sh folds prefix_tokens/prefix_count into a prefix_buckets
# entry and passes the remaining numeric fields through to synthetic_text. No
# delay_stdev (guidellm requires it > 0 when set; without it the sampler draws
# uniform(delay_min, delay_max) = a constant THINK_S when the two are equal).
DATA_SPEC="prompt_tokens=${PROMPT_TOKENS},output_tokens=${OUTPUT_TOKENS},turns=${TURNS}"
DATA_SPEC+=",delay=${THINK_S},delay_min=${THINK_S},delay_max=${THINK_S}"
DATA_SPEC+=",prefix_tokens=${PREFIX_TOKENS},prefix_count=${PREFIX_COUNT}"

exec env \
  MODEL="${MODEL:-qwen1.5-7b}" \
  PROCESSOR="${PROCESSOR:-Qwen/Qwen1.5-7B-Chat}" \
  PORT="${PORT:-8000}" \
  RATE_TYPE="${RATE_TYPE:-concurrent}" \
  CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-8}" \
  MAX_SECONDS="${MAX_SECONDS:-1000}" \
  DATA="$DATA_SPEC" \
  "${SCRIPT_DIR}/run-guidellm.sh"
