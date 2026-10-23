#!/bin/bash
# run-guidellm-synthetic-longctx.sh — CLIENT side of a distribution-driven synthetic
# KV-offload test; defaults to the syfi-coding long-context (1M window) coding-agent
# workload, driven by turn + think-time DISTRIBUTION FILES.
#
# Delivers a shared-prefix synthetic_text workload whose conversation shape is
# taken from two bucketed distribution YAMLs (the format emitted by
# ~/certus/tools/kv-cache-dynamics/*_dist.py):
#
#   * TURN_DIST      — turns-per-conversation distribution  (…-turns.yaml)
#   * THINKTIME_DIST — intra-conversation inter-arrival / "think time" distribution
#                      in seconds (…-intervals.yaml)
#
# Defaults to the uw-syfi/TraceLab syfi-coding distributions, but ANY pair of
# compatible YAMLs can be passed — as env vars or as the first two positional
# arguments:
#
#   ./run-guidellm-synthetic-longctx.sh                                   # syfi-coding defaults
#   ./run-guidellm-synthetic-longctx.sh my-turns.yaml my-intervals.yaml   # other distributions
#   TURN_DIST=a.yaml THINKTIME_DIST=b.yaml ./run-guidellm-synthetic-longctx.sh
#
# ── How a bucketed distribution maps onto guidellm ──────────────────────────────
# guidellm 0.7's synthetic_text models a conversation with a FIXED scalar `turns`
# and a truncated-normal think time (`delay`/`delay_stdev`/`delay_min`/`delay_max`).
# It cannot ingest an arbitrary bucketed distribution, so we fit its parameters to
# the YAMLs at runtime:
#   * turns  = one representative statistic of TURN_DIST (TURNS_STAT, default
#              median; the full min/mean/median/p90/p99 is printed so you can sweep).
#   * delay  = mean & stdev of THINKTIME_DIST restricted to the body
#              [0, DELAY_MAX_S]. The raw distribution has a multi-day return tail
#              (syfi max ≈ 2.7e6 s ≈ 31 days) whose variance would wreck a normal
#              fit and stall the benchmark, so by default we cap at the trace p99
#              (≈1493 s, ~99.2% coverage) and fit the body. DELAY_SCALE compresses
#              think time for faster runs (e.g. DELAY_SCALE=0.1).
# Because `turns` is a single scalar, sweep it (TURNS=16 median, 82 mean, 145 p90,
# 1130 p99) to cover the depth distribution rather than expecting one run to.
#
# ── Context window & shared prefix ──────────────────────────────────────────────
# CONTEXT_WINDOW=1000000: this workload targets a 1,000,000-token server window so
# the deep tail of syfi conversations accumulates history without HTTP 400s. The
# window is a SERVER property — this client cannot set it. Bring up a server that
# serves max_model_len=1000000 first, e.g. a 1M-native model (Qwen2.5-*-1M) or
#   MAX_MODEL_LEN=1000000 ./run-serve-cputier.sh    # YaRN factor ≈30 (quality cost)
# and set MODEL/PROCESSOR here to match its --served-model-name.
#
# PREFIX_TOKENS=20000: a 20,000-token shared system prefix (system prompt + tool
# definitions) prepended to every conversation and cached once — the reuse the
# KV-offload read path exists to exploit. Emitted as a single prefix bucket.
#
# Prerequisites: a server already serving a 1M window on ${TARGET}, and guidellm
# installed on the host (see run-guidellm.sh). Examples:
#   ./run-guidellm-synthetic-longctx.sh                                   # median-depth, p99-capped think time
#   TURNS=82 ./run-guidellm-synthetic-longctx.sh                          # mean-depth conversations
#   DELAY_SCALE=0.1 ./run-guidellm-synthetic-longctx.sh                   # compress think time 10x
#   DELAY_MAX_S=p90 ./run-guidellm-synthetic-longctx.sh                   # tighter think-time body
#   CONCURRENT_STREAMS=4 MAX_SECONDS=300 ./run-guidellm-synthetic-longctx.sh
#   PROMPT_TOKENS=512 OUTPUT_TOKENS=512 ./run-guidellm-synthetic-longctx.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── Distribution files (env or positional $1/$2; defaults = syfi-coding) ─────────
KV_DYNAMICS_DIR="${KV_DYNAMICS_DIR:-${HOME}/certus/tools/kv-cache-dynamics}"
TURN_DIST="${TURN_DIST:-${1:-${KV_DYNAMICS_DIR}/syfi-coding-turns.yaml}}"
# THINKTIME_DIST aliases INTERVAL_DIST (the YAMLs are named *-intervals.yaml).
THINKTIME_DIST="${THINKTIME_DIST:-${INTERVAL_DIST:-${2:-${KV_DYNAMICS_DIR}/syfi-coding-intervals.yaml}}}"

for f in "$TURN_DIST" "$THINKTIME_DIST"; do
  [[ -f "$f" ]] || { echo "error: distribution file not found: $f" >&2; exit 1; }
done

# ── Workload shape knobs ─────────────────────────────────────────────────────────
CONTEXT_WINDOW="${CONTEXT_WINDOW:-524288}"   # target server window (served-side; see header)
PREFIX_TOKENS="${PREFIX_TOKENS:-20000}"       # shared system prefix, cached once per group
# Default to 96 distinct 20K prefixes (~1.92M tokens of REUSED prefix). This is the
# tier-stress default sized for the 7B-1M server (~1M-token GPU KV at the 524K
# window): 1.92M oversubscribes it ~1.9x, so cold prefixes evict to the CPU/fs
# offload tiers and reload on reuse (external prefix-cache hits). Lower values fit
# in the 7B's big KV pool and barely touch the tiers (32 -> 640K < 1M = ~0% external);
# for the 14B server (~221K KV) even 32 oversubscribes. PREFIX_COUNT=1 = no tier traffic.
PREFIX_COUNT="${PREFIX_COUNT:-96}"            # distinct shared-prefix groups
# TURNS fixed at 12 by default (the tier-stress config). Set TURNS= (empty) to
# instead sample the conversation depth from TURN_DIST via TURNS_STAT.
TURNS="${TURNS-12}"
TURNS_STAT="${TURNS_STAT:-median}"            # TURN_DIST statistic used when TURNS is empty
DELAY_MAX_S="${DELAY_MAX_S:-p99}"             # think-time body cap: a number, or p90|p99|max
DELAY_SCALE="${DELAY_SCALE:-1.0}"             # multiply all think times (0.1 = 10x faster)
# guidellm's requeue_delay must be STRICTLY > 0 (Gt(gt=0)); a sampled think time
# of exactly 0 makes it silently DROP the whole conversation row. guidellm also
# only offers a symmetric truncated normal, which cannot represent this positive
# heavy-tailed think time without pushing mass to/below 0 — that sub-floor mass
# clamps onto delay_min. So delay_min is floored strictly positive here.
DELAY_MIN_S="${DELAY_MIN_S:-0.001}"           # positive think-time floor (must be > 0)
# The distributions carry turn COUNT and think TIME only, not per-turn token sizes.
# These coding-agent defaults (new prompt tokens + prior reply length per turn)
# match the cc trace analysis; override to reshape per-turn growth.
PROMPT_TOKENS="${PROMPT_TOKENS:-364}"
OUTPUT_TOKENS="${OUTPUT_TOKENS:-1398}"

# ── Derive guidellm params from the distribution files ───────────────────────────
# Isolated interpreter (-I): no cwd/user-site code injection; stdlib yaml only.
read -r SYFI_TURNS SYFI_DELAY SYFI_DELAY_STDEV SYFI_DELAY_MIN SYFI_DELAY_MAX SYFI_DELAY_FLOORPCT < <(
python3 -I - "$TURN_DIST" "$THINKTIME_DIST" "$TURNS_STAT" "${TURNS:-}" "$DELAY_MAX_S" "$DELAY_SCALE" "$DELAY_MIN_S" <<'PY'
import math, sys, yaml
from statistics import NormalDist
turn_f, think_f, turns_stat, turns_override, delay_cap, delay_scale, delay_min_s = sys.argv[1:8]

def load(p):
    with open(p) as fh:
        return yaml.safe_load(fh)

# ── turns: one representative scalar from the turn distribution summary ──────────
td = load(turn_f)
tsum = td.get("summary", {})
if turns_override.strip():
    turns = int(float(turns_override))
else:
    if turns_stat not in tsum:
        sys.exit(f"TURNS_STAT='{turns_stat}' not in {turn_f} summary {sorted(tsum)}")
    turns = int(round(float(tsum[turns_stat])))
turns = max(1, turns)

# ── think time: fit a truncated normal to the body of the interval distribution ──
hd = load(think_f)
buckets = hd["buckets"]
# interval YAMLs key bucket edges as lower_s/upper_s; turn-style use lower/upper.
lo_k = "lower_s" if "lower_s" in buckets[0] else "lower"
up_k = "upper_s" if "upper_s" in buckets[0] else "upper"
hsum = hd.get("summary", {})

def sumv(*names):
    for n in names:
        if n in hsum:
            return float(hsum[n])
    return None

# Resolve the body cap: a bare number, or a percentile/max keyword.
kw = {"p90": sumv("p90_s", "p90"), "p99": sumv("p99_s", "p99"),
      "max": sumv("max_s", "max")}
if delay_cap in kw:
    cap = kw[delay_cap]
    if cap is None:
        sys.exit(f"DELAY_MAX_S='{delay_cap}' has no summary value in {think_f}")
else:
    cap = float(delay_cap)

# Empirical mean/stdev over bucket geometric midpoints with midpoint <= cap.
n = s = s2 = 0.0
for b in buckets:
    lo, up, c = float(b[lo_k]), float(b[up_k]), float(b["count"])
    mid = math.sqrt(lo * up) if lo > 0 else up / 2.0
    if mid > cap:
        continue
    n += c; s += c * mid; s2 += c * mid * mid
if n == 0:
    sys.exit(f"no think-time buckets at or below cap {cap} in {think_f}")
mean = s / n
stdev = math.sqrt(max(s2 / n - mean * mean, 0.0))

scale = float(delay_scale)
delay      = round(mean * scale, 4)
delay_std  = round(stdev * scale, 4)
# guidellm requires requeue_delay > 0: floor delay_min strictly positive, else
# every conversation whose sampled delay lands <= 0 is silently dropped.
delay_min  = round(max(float(delay_min_s), 1e-6) * scale, 6)
delay_max  = round(cap * scale, 4)

# How much of the fitted (symmetric) normal falls at/below the floor and thus
# collapses onto delay_min — a measure of the normal-vs-heavy-tail mismatch.
floor_pct = round(NormalDist(delay, delay_std).cdf(delay_min) * 100, 1) if delay_std > 0 else 0.0

print(turns, delay, delay_std, delay_min, delay_max, floor_pct)
PY
)

# ── Context-window sanity check (informational) ──────────────────────────────────
# Rough accumulated context after the conversation: prefix + turns*(prompt+gen).
EST_CTX=$(( PREFIX_TOKENS + SYFI_TURNS * (PROMPT_TOKENS + OUTPUT_TOKENS) ))
if (( EST_CTX > CONTEXT_WINDOW )); then
  echo "[syfi] NOTE: est. accumulated context ~${EST_CTX} tok > CONTEXT_WINDOW=${CONTEXT_WINDOW}" >&2
  echo "[syfi]       deep conversations will slide/overflow; raise the server window or lower TURNS." >&2
fi

echo "[syfi] turn dist      : ${TURN_DIST}"
echo "[syfi] thinktime dist : ${THINKTIME_DIST}"
echo "[syfi] turns=${SYFI_TURNS} (${TURNS:+override}${TURNS:-stat=${TURNS_STAT}})  prefix=${PREFIX_TOKENS}x${PREFIX_COUNT}  ctx-window=${CONTEXT_WINDOW}"
echo "[syfi] think time (s) : delay=${SYFI_DELAY} stdev=${SYFI_DELAY_STDEV} min=${SYFI_DELAY_MIN} max=${SYFI_DELAY_MAX} (cap=${DELAY_MAX_S}, scale=${DELAY_SCALE})"
# guidellm's symmetric normal can't match a heavy-tailed think time; whatever mass
# sits <= delay_min collapses onto the floor. Surface it (was silently dropping rows).
awk -v p="${SYFI_DELAY_FLOORPCT}" 'BEGIN{ if (p+0 >= 10) printf "[syfi] NOTE: ~%.0f%% of the fitted think-time mass collapses onto delay_min=%s (symmetric-normal vs heavy-tail limit); raise DELAY_MIN_S or lower DELAY_MAX_S to reshape\n", p, "'"${SYFI_DELAY_MIN}"'" }'
echo "[syfi] per-turn tokens: prompt=${PROMPT_TOKENS} output=${OUTPUT_TOKENS}  est-accum-ctx~${EST_CTX}"

# ── Assemble the synthetic_text DATA spec and hand off to the engine ─────────────
# Comma form (no leading kind=) so run-guidellm.sh's build_data_json folds
# prefix_tokens/prefix_count into a prefix_buckets entry and passes the numeric
# turns/delay* fields straight through to a synthetic_text JSON config.
DATA_SPEC="prompt_tokens=${PROMPT_TOKENS},output_tokens=${OUTPUT_TOKENS},turns=${SYFI_TURNS}"
DATA_SPEC+=",delay=${SYFI_DELAY},delay_stdev=${SYFI_DELAY_STDEV},delay_min=${SYFI_DELAY_MIN},delay_max=${SYFI_DELAY_MAX}"
DATA_SPEC+=",prefix_tokens=${PREFIX_TOKENS},prefix_count=${PREFIX_COUNT}"

MODEL="${MODEL:-qwen2.5-7b-1m}" \
PROCESSOR="${PROCESSOR:-Qwen/Qwen2.5-7B-Instruct-1M}" \
PORT="${PORT:-8000}" \
RATE_TYPE="${RATE_TYPE:-concurrent}" \
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-8}" \
MAX_SECONDS="${MAX_SECONDS:-1200}" \
DATA="$DATA_SPEC" \
  exec "${SCRIPT_DIR}/run-guidellm.sh"
