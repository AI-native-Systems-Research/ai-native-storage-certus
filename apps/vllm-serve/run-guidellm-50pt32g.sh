#!/bin/bash
# run-guidellm-50pt32g.sh — CLIENT side of the "50pt@32G" experiment. Pairs with
# run-serve-certus-shmq-50pt32g.sh. Thin wrapper over run-guidellm.sh.
#
# Parameters come from the dram_dashboard preset
#   ~/certus/tools/kv-cache-dynamics/dram-dashboard-config-50pt@32G.yaml
# mapped onto a guidellm synthetic_text multi-turn workload:
#
#   preset key                      -> here
#   dist wildchat                   -> think time fitted from wildchat-intervals.yaml
#   turnMode ccweka                 -> depth from cc-trace-weka-062126-turns.yaml (see below)
#   prefix 20000, groups 1, shared 0-> PREFIX_TOKENS=20000, PREFIX_COUNT=1 (one global prefix)
#   prompt 364 / gen 1398           -> PROMPT_TOKENS / OUTPUT_TOKENS
#   concurrent 8                    -> CONCURRENT_STREAMS=8
#   sessions 100                    -> bounded by MAX_SECONDS (guidellm has no session count)
#   mml 500000                      -> CONTEXT_WINDOW=49152 (server-side; see the serve script)
#
# DEVIATIONS (guidellm / hardware limits):
#   * turns: guidellm takes ONE scalar depth, not a distribution. The ccweka median
#     (67) would grow to ~138K tokens, far past the 49152 window the 10G HBM allows,
#     and 400 every turn after ~16. By default TURNS is clamped to the deepest
#     conversation that fits: floor((window - prefix) / (prompt + gen)) = 16. Set
#     TURNS_CLAMP=0 to use TURNS_STAT of the ccweka distribution unclamped, or
#     TURNS=N explicitly.
#   * think time: guidellm offers only a truncated normal; wildchat has a multi-day
#     tail (p99 ≈ 7.6 h). We fit mean/stdev over the body up to DELAY_MAX_S (p90:
#     mean ≈150 s, stdev ≈128 s) and compress it 10x (DELAY_SCALE=0.1) so a run
#     covers several conversation generations inside MAX_SECONDS. DELAY_SCALE=1.0
#     for real-time think time (needs a much longer MAX_SECONDS).
#
#   ./run-guidellm-50pt32g.sh                               # 16 turns, 8 streams, 1800 s
#   TURNS_CLAMP=0 ./run-guidellm-50pt32g.sh                 # ccweka median depth (overflows the window)
#   CONTEXT_WINDOW=131072 ./run-guidellm-50pt32g.sh         # pair with the 131072-window serve variant
#   DELAY_SCALE=1.0 MAX_SECONDS=7200 ./run-guidellm-50pt32g.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

KV_DYNAMICS_DIR="${KV_DYNAMICS_DIR:-${HOME}/certus/tools/kv-cache-dynamics}"
TURN_DIST="${TURN_DIST:-${KV_DYNAMICS_DIR}/cc-trace-weka-062126-turns.yaml}"
THINKTIME_DIST="${THINKTIME_DIST:-${KV_DYNAMICS_DIR}/wildchat-intervals.yaml}"
for f in "$TURN_DIST" "$THINKTIME_DIST"; do
  [[ -f "$f" ]] || { echo "error: distribution file not found: $f" >&2; exit 1; }
done

CONTEXT_WINDOW="${CONTEXT_WINDOW:-49152}"
PREFIX_TOKENS="${PREFIX_TOKENS:-20000}"
PREFIX_COUNT="${PREFIX_COUNT:-1}"
PROMPT_TOKENS="${PROMPT_TOKENS:-364}"
OUTPUT_TOKENS="${OUTPUT_TOKENS:-1398}"
TURNS_STAT="${TURNS_STAT:-median}"
DELAY_MAX_S="${DELAY_MAX_S:-p90}"   # think-time body cap: a number, or p90|p99|max
DELAY_SCALE="${DELAY_SCALE:-0.1}"
# guidellm silently DROPS a conversation whose sampled think time is <= 0, so the
# truncated normal's floor must be strictly positive.
DELAY_MIN_S="${DELAY_MIN_S:-0.001}"

# Clamp depth to what fits the window unless the caller set TURNS or disabled it.
if [[ -z "${TURNS:-}" && "${TURNS_CLAMP:-1}" != "0" ]]; then
  TURNS=$(( (CONTEXT_WINDOW - PREFIX_TOKENS) / (PROMPT_TOKENS + OUTPUT_TOKENS) ))
  (( TURNS >= 1 )) || { echo "error: window ${CONTEXT_WINDOW} too small for prefix ${PREFIX_TOKENS}" >&2; exit 1; }
  echo "[50pt32g] TURNS clamped to ${TURNS} to fit CONTEXT_WINDOW=${CONTEXT_WINDOW}"
fi

# ── Fit turns (if unset) and the think-time truncated normal from the YAMLs ──────
# Isolated interpreter (-I): no cwd/user-site code injection.
read -r TURNS DELAY DELAY_STDEV DELAY_MIN DELAY_MAX < <(
python3 -I - "$TURN_DIST" "$THINKTIME_DIST" "$TURNS_STAT" "${TURNS:-}" "$DELAY_MAX_S" "$DELAY_SCALE" "$DELAY_MIN_S" <<'PY'
import math, sys, yaml
turn_f, think_f, turns_stat, turns, cap_s, scale, dmin = sys.argv[1:8]
if not turns.strip():
    tsum = yaml.safe_load(open(turn_f))["summary"]
    if turns_stat not in tsum:
        sys.exit(f"TURNS_STAT='{turns_stat}' not in {turn_f} summary {sorted(tsum)}")
    turns = str(max(1, round(float(tsum[turns_stat]))))
hd = yaml.safe_load(open(think_f))
hsum = hd.get("summary", {})
cap = float(hsum[f"{cap_s}_s"]) if cap_s in ("p90", "p99", "max") else float(cap_s)
# Mean/stdev over bucket geometric midpoints at or below the cap.
n = s = s2 = 0.0
for b in hd["buckets"]:
    lo, up, c = float(b["lower_s"]), float(b["upper_s"]), float(b["count"])
    mid = math.sqrt(lo * up) if lo > 0 else up / 2.0
    if mid <= cap:
        n += c; s += c * mid; s2 += c * mid * mid
if n == 0:
    sys.exit(f"no think-time buckets at or below cap {cap} in {think_f}")
mean = s / n
std = math.sqrt(max(s2 / n - mean * mean, 0.0))
k = float(scale)
print(turns, round(mean * k, 4), round(std * k, 4),
      round(max(float(dmin), 1e-6) * k, 6), round(cap * k, 4))
PY
)

EST_CTX=$(( PREFIX_TOKENS + TURNS * (PROMPT_TOKENS + OUTPUT_TOKENS) ))
if (( EST_CTX > CONTEXT_WINDOW )); then
  echo "[50pt32g] NOTE: est. accumulated context ~${EST_CTX} tok > CONTEXT_WINDOW=${CONTEXT_WINDOW}; deep turns will 400" >&2
fi
echo "[50pt32g] turns=${TURNS} prefix=${PREFIX_TOKENS}x${PREFIX_COUNT} prompt=${PROMPT_TOKENS} output=${OUTPUT_TOKENS} est-ctx~${EST_CTX}"
echo "[50pt32g] think time (s): delay=${DELAY} stdev=${DELAY_STDEV} min=${DELAY_MIN} max=${DELAY_MAX} (cap=${DELAY_MAX_S}, scale=${DELAY_SCALE})"

# Comma form: run-guidellm.sh folds prefix_tokens/prefix_count into a prefix_buckets
# entry and passes the remaining numeric fields through to synthetic_text.
DATA_SPEC="prompt_tokens=${PROMPT_TOKENS},output_tokens=${OUTPUT_TOKENS},turns=${TURNS}"
DATA_SPEC+=",delay=${DELAY},delay_stdev=${DELAY_STDEV},delay_min=${DELAY_MIN},delay_max=${DELAY_MAX}"
DATA_SPEC+=",prefix_tokens=${PREFIX_TOKENS},prefix_count=${PREFIX_COUNT}"

MODEL="${MODEL:-qwen2.5-14b}" \
PROCESSOR="${PROCESSOR:-Qwen/Qwen2.5-14B-Instruct}" \
PORT="${PORT:-8000}" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-8}" \
MAX_SECONDS="${MAX_SECONDS:-1800}" \
DATA="$DATA_SPEC" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
