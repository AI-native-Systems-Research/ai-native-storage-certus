#!/bin/bash
# cc131k-step1-reconstruct.sh — STEP 1 of running kv-offload-otel-replay on the
# cc131k mooncake trace.
#
# The cc131k trace is a FLAT mooncake request log (one request per line, no
# conversation structure — only hash_ids encode KV-prefix sharing). The OTel
# corpus builder (step 2) needs a conversation_replay PLAN. This step runs the
# mooncake->plan reconstruction (trace-gen/mooncake_to_plan.py), chaining rows
# whose hash_ids extend a growing prefix into multi-turn conversations so the
# within-conversation KV reuse survives into the corpus.
#
# Override via env:
#   MOONCAKE=/mnt/certus1/cc-traces-weka-062126.mooncake-131k.jsonl \
#   PLAN=/mnt/certus1/inference-perf-syn-data/cc131k.plan.json \
#   MAX_ROWS= STATS_ONLY=0  ./cc131k-step1-reconstruct.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# python3.12 to match the corpus builder (step 2) and the trace-gen shebangs.
PY="${PY:-python3.12}"

MOONCAKE="${MOONCAKE:-/mnt/certus1/cc-traces-weka-062126.mooncake-131k.jsonl}"
PLAN="${PLAN:-/mnt/certus1/inference-perf-syn-data/cc131k.plan.json}"
MAX_ROWS="${MAX_ROWS:-}"      # empty = all rows; set N for a quick test
STATS_ONLY="${STATS_ONLY:-0}" # 1 = print reconstruction stats and write nothing

if [[ ! -f "$MOONCAKE" ]]; then
  echo "error: mooncake trace not found: $MOONCAKE" >&2
  exit 1
fi
mkdir -p "$(dirname "$PLAN")"

ARGS=("$MOONCAKE" "$PLAN")
[[ -n "$MAX_ROWS" ]] && ARGS+=(--max-rows "$MAX_ROWS")
[[ "$STATS_ONLY" == "1" ]] && ARGS+=(--stats-only)

echo "[step1] mooncake  : ${MOONCAKE}"
echo "[step1] plan out  : ${PLAN}${STATS_ONLY:+ (stats-only: ${STATS_ONLY})}"
echo "[step1] max rows  : ${MAX_ROWS:-all}"
echo

"$PY" "${SCRIPT_DIR}/trace-gen/mooncake_to_plan.py" "${ARGS[@]}"

if [[ "$STATS_ONLY" != "1" ]]; then
  echo
  echo "[step1] done. next:  PLAN=${PLAN} ./cc131k-step2-corpus.sh"
fi
