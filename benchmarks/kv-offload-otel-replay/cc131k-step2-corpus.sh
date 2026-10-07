#!/bin/bash
# cc131k-step2-corpus.sh — STEP 2 of running kv-offload-otel-replay on cc131k.
#
# Materialise the per-conversation OTel corpus from the plan trace produced by
# step 1, using the shared stage-2 builder (trace-gen/trace_to_otel.py). One
# conv_*.json per conversation, with Qwen-tokenised filler text sized to the
# recorded token counts and a sliding context window capped at --cap.
#
# SIZE WARNING: each span embeds its full message history, so the corpus grows
# fast (the shipped otel_1k was ~22 GB for 1000 convs; cc131k prompts are
# larger). Start with a bounded NUM_CONVS and check `du -sh` before scaling.
# The plan has 2612 conversations total.
#
# Override via env:
#   PLAN=/mnt/certus1/inference-perf-syn-data/cc131k.plan.json \
#   OTEL_HOST=/mnt/certus1/inference-perf-syn-data/otel_cc131k \
#   NUM_CONVS=200 CAP=120000 MODEL=Qwen/Qwen2.5-14B-Instruct SEED=7 VALIDATE=1 \
#   ./cc131k-step2-corpus.sh
set -euo pipefail

# numpy/transformers/inference-perf are installed for python3.12 (in the user
# site). Default to it, and do NOT use `python -I`: isolated mode drops the user
# site-packages dir, hiding those deps.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PY="${PY:-python3.12}"

PLAN="${PLAN:-/mnt/certus1/inference-perf-syn-data/cc131k.plan.json}"
OTEL_HOST="${OTEL_HOST:-/mnt/certus1/inference-perf-syn-data/otel_cc131k}"
NUM_CONVS="${NUM_CONVS:-200}"            # bounded by default; up to 2612
CAP="${CAP:-120000}"                     # accounted-prompt cap (< 131072 window)
# Match the tokenizer to the model you will SERVE in step 4 (cc131k = 14B).
MODEL="${MODEL:-Qwen/Qwen2.5-14B-Instruct}"
SEED="${SEED:-7}"
VALIDATE="${VALIDATE:-1}"                # 1 = round-trip first file through loader
HF_CACHE="${HF_CACHE:-/mnt/certus1/hf-cache}"

if [[ ! -f "$PLAN" ]]; then
  echo "error: plan trace not found: $PLAN" >&2
  echo "       run ./cc131k-step1-reconstruct.sh first." >&2
  exit 1
fi

export HF_HOME="${HF_CACHE}"             # keep the tokenizer download off /home

ARGS=("$PLAN" "$OTEL_HOST" --num "$NUM_CONVS" --cap "$CAP" --model "$MODEL" --seed "$SEED")
[[ "$VALIDATE" == "1" ]] && ARGS+=(--validate)

echo "[step2] plan      : ${PLAN}"
echo "[step2] corpus out: ${OTEL_HOST}"
echo "[step2] num convs : ${NUM_CONVS}  (cap ${CAP}, model ${MODEL})"
echo "[step2] hf cache  : ${HF_CACHE}"
echo

"$PY" "${SCRIPT_DIR}/trace-gen/trace_to_otel.py" "${ARGS[@]}"

echo
echo "[step2] corpus size: $(du -sh "$OTEL_HOST" | cut -f1)  ($(ls "$OTEL_HOST" | wc -l) files)"
echo "[step2] done. next:"
echo "          (shmq arm) start the server:  ./cc131k-step3-server.sh"
echo "          then run an arm:  OTEL_HOST=${OTEL_HOST} NUM_CONVS=${NUM_CONVS} MODEL=${MODEL} ARM=shmq ./cc131k-step4-run.sh"
