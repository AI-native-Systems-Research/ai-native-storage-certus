#!/bin/bash
# run-guidellm-mha-iobound-intervals.sh — CLIENT side of the MHA KV-IO-bound test,
# but with intra-conversation THINK TIME driven by a real inter-arrival
# DISTRIBUTION from tools/kv-cache-dynamics, instead of the constant ~0 think time
# of run-guidellm-mha-iobound.sh. Same server (run-serve-certus-shmq-mha-iobound.sh
# or the cputier counterpart). Thin wrapper over run-guidellm.sh.
#
# WHY A DISTRIBUTION: the interval YAMLs (…-intervals.yaml) record the real gap
# between consecutive requests WITHIN a conversation (human think time + next-turn
# latency). guidellm applies the fitted gap as the requeue delay between a
# conversation's turns, so each of the 8 concurrent streams pauses a sampled amount
# between turns. During that pause the OTHER streams churn the cache and evict this
# session's KV — so when it resumes it must RELOAD its whole chain from the offload
# tier. That eviction-during-think-time is the realistic driver of offload-tier
# reload I/O (vs. the saturated round-robin, which keeps every session hot).
#
# TRADE-OFF vs the constant-think variant: think time adds GPU idle (less wall-clock
# is spent computing), but it raises the RELOAD VOLUME per resumed turn. So this is
# the more realistic I/O-driver; the constant variant maximizes raw throughput.
#
# ── How the distribution maps onto guidellm (same fit as run-guidellm-50pt32g.sh) ─
# guidellm offers only a truncated normal (delay/stdev/min/max), so we fit it to the
# BODY of the interval histogram up to DELAY_MAX_S (the raw dist has a multi-day
# tail that would wreck a normal fit). DELAY_SCALE compresses think time so a run
# covers many turns in MAX_SECONDS (wildchat body mean ≈150 s; x0.1 ≈ 15 s/turn).
#
# INTERVAL_DIST (env or positional $1; default wildchat-intervals.yaml). Any
# …-intervals.yaml from tools/kv-cache-dynamics works:
#   ./run-guidellm-mha-iobound-intervals.sh                               # wildchat, p90 body, x0.1
#   ./run-guidellm-mha-iobound-intervals.sh cc-trace-weka-062126-intervals.yaml
#   INTERVAL_DIST=.../burstgpt-3-intervals.yaml ./run-guidellm-mha-iobound-intervals.sh
#   DELAY_SCALE=1.0 MAX_SECONDS=3600 ./run-guidellm-mha-iobound-intervals.sh   # real-time think
#   DELAY_MAX_S=p99 ./run-guidellm-mha-iobound-intervals.sh                    # wider think-time body
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

KV_DYNAMICS_DIR="${KV_DYNAMICS_DIR:-${HOME}/certus/tools/kv-cache-dynamics}"
INTERVAL_DIST="${INTERVAL_DIST:-${1:-${KV_DYNAMICS_DIR}/wildchat-intervals.yaml}}"
[[ -f "$INTERVAL_DIST" ]] || { echo "error: interval distribution not found: $INTERVAL_DIST" >&2; exit 1; }

# ── Workload shape: identical to run-guidellm-mha-iobound.sh ──────────────────────
CONTEXT_WINDOW="${CONTEXT_WINDOW:-32768}"
PREFIX_TOKENS="${PREFIX_TOKENS:-8192}"
PREFIX_COUNT="${PREFIX_COUNT:-64}"
PROMPT_TOKENS="${PROMPT_TOKENS:-128}"
OUTPUT_TOKENS="${OUTPUT_TOKENS:-1}"          # prefill-only turns (emulate pure prefix reuse)
TURNS="${TURNS:-128}"
KV_PER_TOKEN_KIB="${KV_PER_TOKEN_KIB:-512}"  # Qwen1.5-7B MHA, fp16 — working-set report only

# ── Think-time fit knobs ──────────────────────────────────────────────────────────
DELAY_MAX_S="${DELAY_MAX_S:-p90}"            # body cap: a number, or p90|p99|max
DELAY_SCALE="${DELAY_SCALE:-0.1}"            # multiply all think times (0.1 = 10x faster)
DELAY_MIN_S="${DELAY_MIN_S:-0.001}"          # positive floor (guidellm drops <=0)

EST_CTX=$(( PREFIX_TOKENS + TURNS * (PROMPT_TOKENS + OUTPUT_TOKENS) ))
(( EST_CTX <= CONTEXT_WINDOW )) || echo "[mha-iobound] NOTE: est. ctx ~${EST_CTX} > window ${CONTEXT_WINDOW}; deep turns 400/slide" >&2

# Fit a truncated normal to the body of the interval histogram (isolated python).
read -r DELAY DELAY_STDEV DELAY_MIN DELAY_MAX < <(
python3 -I - "$INTERVAL_DIST" "$DELAY_MAX_S" "$DELAY_SCALE" "$DELAY_MIN_S" <<'PY'
import math, sys, yaml
think_f, cap_s, scale, dmin = sys.argv[1:5]
hd = yaml.safe_load(open(think_f))
buckets = hd["buckets"]
lo_k = "lower_s" if "lower_s" in buckets[0] else "lower"
up_k = "upper_s" if "upper_s" in buckets[0] else "upper"
hsum = hd.get("summary", {})
def sv(*names):
    for n in names:
        if n in hsum: return float(hsum[n])
    return None
cap = {"p90": sv("p90_s","p90"), "p99": sv("p99_s","p99"), "max": sv("max_s","max")}.get(cap_s)
if cap_s not in ("p90","p99","max"): cap = float(cap_s)
if cap is None: sys.exit(f"DELAY_MAX_S='{cap_s}' has no summary value in {think_f}")
n=s=s2=0.0
for b in buckets:
    lo,up,c = float(b[lo_k]), float(b[up_k]), float(b["count"])
    mid = math.sqrt(lo*up) if lo>0 else up/2.0
    if mid<=cap: n+=c; s+=c*mid; s2+=c*mid*mid
if n==0: sys.exit(f"no think-time buckets at or below cap {cap} in {think_f}")
mean=s/n; std=math.sqrt(max(s2/n-mean*mean,0.0)); k=float(scale)
print(round(mean*k,4), round(std*k,4), round(max(float(dmin),1e-6)*k,6), round(cap*k,4))
PY
)

echo "[mha-iobound] interval dist : ${INTERVAL_DIST}"
WORKING_SET_GIB=$(awk "BEGIN{printf \"%.0f\", ${PREFIX_COUNT}*${PREFIX_TOKENS}*${KV_PER_TOKEN_KIB}/1048576}")
echo "[mha-iobound] prefix=${PREFIX_TOKENS}x${PREFIX_COUNT} => ~${WORKING_SET_GIB} GiB reused KV; turns=${TURNS} prompt=${PROMPT_TOKENS} output=${OUTPUT_TOKENS} est-ctx~${EST_CTX}/${CONTEXT_WINDOW}"
echo "[mha-iobound] think time (s): delay=${DELAY} stdev=${DELAY_STDEV} min=${DELAY_MIN} max=${DELAY_MAX} (cap=${DELAY_MAX_S}, scale=${DELAY_SCALE})"

# Comma form: run-guidellm.sh folds prefix_tokens/prefix_count into prefix_buckets.
# Emit delay_stdev only when > 0 (guidellm requires it strictly positive if set).
DATA_SPEC="prompt_tokens=${PROMPT_TOKENS},output_tokens=${OUTPUT_TOKENS},turns=${TURNS}"
DATA_SPEC+=",delay=${DELAY}"
awk -v s="$DELAY_STDEV" 'BEGIN{exit !(s+0>0)}' && DATA_SPEC+=",delay_stdev=${DELAY_STDEV}"
DATA_SPEC+=",delay_min=${DELAY_MIN},delay_max=${DELAY_MAX}"
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
