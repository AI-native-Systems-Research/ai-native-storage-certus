#!/bin/bash
# run-aiperf-weka-iobound.sh — CC-weka trace client for the iobound matrix: replays a
# WEKA-format cc-trace with aiperf (multi-turn and subagent structure kept), back to back
# (no think time / inter-turn delays), in place of run-guidellm-gqa-iobound.sh. Thin
# wrapper over run-aiperf-weka.sh. Select it per run with
#     CLIENT_SCRIPT=run-aiperf-weka-iobound.sh ./run-gqa-iobound-matrix.sh   (or the sweep)
#
# Why aiperf, not guidellm: guidellm's mooncake replay sends every request as an
# unrelated prompt, so a cc-trace's prefix reuse never reaches vLLM (0 cache hits);
# aiperf's WEKA loader rebuilds each turn's prompt from its hash_ids, so turns of one
# conversation share their prefix KV as in the recorded sessions.
#
# The matrix calls clients with OUTPUT=<arm>/guidellm.json, CONCURRENT_STREAMS and
# MAX_SECONDS. This wrapper maps them onto aiperf: results go to <arm>/aiperf/
# (profile_export_aiperf.json), CONCURRENT_STREAMS -> --concurrency (requests in flight),
# MAX_SECONDS -> --benchmark-duration. It writes the same before/after /metrics snapshot
# as run-guidellm.sh, to the matrix's metrics path (${OUTPUT%.json}.metrics.txt), so the
# report reads both clients' runs the same way.
#
# Each turn's max_tokens is its recorded output length, and ignore_eos makes the model
# produce all of it (otherwise it stops at its own EOS and the decode load shrinks).
# Token counts come from the server's usage fields: re-tokenising the streamed text
# undercounts outputs that contain special tokens.
# Conversations must fit the served window: build a truncated copy of the trace for it
# (TRACE below is the 262K cut of cc-traces-weka-062126; see its README.txt).
#
#   CONCURRENT_STREAMS=50 MAX_SECONDS=900 OUTPUT=/tmp/x/guidellm.json ./run-aiperf-weka-iobound.sh
#   TIMING=think CONCURRENT_STREAMS=600 TRACE=.../trunc262k-out1-x3 ...  # think-time pacing;
#     CONCURRENT_STREAMS then caps in-flight requests (--prefill-concurrency) while every
#     conversation in the pool is admitted — a thinking conversation holds no slot.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

TRACE="${TRACE:-/mnt/certus1/cc-traces-weka-062126.trunc262k}"
OUTPUT="${OUTPUT:?OUTPUT not set (the matrix passes <arm>/guidellm.json)}"
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-8}"
# TIMING=think replays each turn's recorded think_time (aiperf --use-think-time-only)
# instead of firing back to back. Under think time aiperf's --concurrency is a SESSION
# slot held for a conversation's whole lifetime (think gaps included), so at
# CONCURRENT_STREAMS sessions only that many conversations would ever run. Instead we
# admit the whole pool (SESSION_CONCURRENCY) and cap in-flight REQUESTS with
# --prefill-concurrency: that slot is released at TTFT (~= completion for out=1), so a
# thinking conversation holds nothing and others process in the meantime.
TIMING="${TIMING:-asap}"            # asap | think | delaycap | intervals (see run-aiperf-weka.sh)
SESSION_CONCURRENCY="${SESSION_CONCURRENCY:-$(find "$TRACE" -maxdepth 1 -name '*.json' 2>/dev/null | wc -l)}"
MAX_SECONDS="${MAX_SECONDS:-900}"
GRACE_SECONDS="${GRACE_SECONDS:-120}"      # responses still in flight at the deadline
# The served name, not MODEL: the sweep exports MODEL as the serve scripts' checkpoint path.
MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b-1m}"
# Local copy of the served checkpoint's tokenizer (no hub download on the host).
TOKENIZER="${TOKENIZER:-/mnt/certus1/hf-cache/qwen7b1m-dense}"
PORT="${PORT:-8000}"
TARGET="${TARGET:-http://127.0.0.1:${PORT}}"
ARM_DIR="$(dirname "$OUTPUT")"

# Same counters and format as run-guidellm.sh's snapshot (the report parses these).
METRICS_OUT="${OUTPUT%.json}.metrics.txt"
METRICS_RE='prefix_cache|external|offload|kv_transfer|connector|kv_cache_usage|prompt_tokens_by_source|request_queue_time_seconds_(sum|count)|request_prefill_time_seconds_(sum|count)|time_to_first_token_seconds_(sum|count)'
snap_metrics() {  # $1 = label
  local out
  out="$(curl -fsS --max-time 30 "${TARGET}/metrics" 2>/dev/null | grep -iE "$METRICS_RE" | grep -v '^#')"
  echo "### ${1} $(date -Is)"
  [[ -n "$out" ]] && echo "$out" || echo "# (no matching counters at ${TARGET}/metrics)"
}

nconv=$(find "$TRACE" -maxdepth 1 -name '*.json' 2>/dev/null | wc -l)
if [[ "$TIMING" == "asap" ]]; then
  AIPERF_CONCURRENCY="$CONCURRENT_STREAMS"
  EXTRA_ARGS="--benchmark-grace-period ${GRACE_SECONDS} --use-server-token-count --extra-inputs {\"ignore_eos\":true}"
  echo "[ccweka] trace=${TRACE} (${nconv} conversations) timing=asap concurrency=${CONCURRENT_STREAMS} duration=${MAX_SECONDS}s grace=${GRACE_SECONDS}s"
else
  AIPERF_CONCURRENCY="$SESSION_CONCURRENCY"
  EXTRA_ARGS="--benchmark-grace-period ${GRACE_SECONDS} --use-server-token-count --extra-inputs {\"ignore_eos\":true} --prefill-concurrency ${CONCURRENT_STREAMS}"
  echo "[ccweka] trace=${TRACE} (${nconv} conversations) timing=${TIMING} sessions=${SESSION_CONCURRENCY} (all admitted) in-flight cap (prefill)=${CONCURRENT_STREAMS} duration=${MAX_SECONDS}s grace=${GRACE_SECONDS}s"
fi
echo "[ccweka] model=${MODEL_NAME} tokenizer=${TOKENIZER} max_tokens=recorded out, ignore_eos=true"
snap_metrics "BEFORE" > "$METRICS_OUT" 2>/dev/null || true

set +e
TRACE="$TRACE" TIMING="$TIMING" CONCURRENCY="$AIPERF_CONCURRENCY" BENCHMARK_DURATION="$MAX_SECONDS" \
  MODEL="$MODEL_NAME" TOKENIZER="$TOKENIZER" TARGET="$TARGET" ARTIFACT_DIR="${ARM_DIR}/aiperf" \
  AIPERF_EXTRA_ARGS="$EXTRA_ARGS" \
  "${SCRIPT_DIR}/run-aiperf-weka.sh"
rc=$?
set -e

snap_metrics "AFTER" >> "$METRICS_OUT" 2>/dev/null || true
exit "$rc"
