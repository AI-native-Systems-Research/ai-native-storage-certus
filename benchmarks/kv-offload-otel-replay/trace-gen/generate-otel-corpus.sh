#!/bin/bash
# generate-otel-corpus.sh — build the kv-offload-otel-replay corpus from an
# inference-perf conversation_replay workload, end to end.
#
# Two stages (see this dir's section in ../README.md for the full explanation):
#   1. gen_conversation_trace.py  <config.yaml> <trace.json> [num]
#        Runs inference-perf's ConversationReplayDataGenerator and dumps the
#        workload PLAN (per-turn input/output token lengths + tool-call delays).
#        Reproducible from (seed, config); random filler TEXT is skipped here.
#   2. trace_to_otel.py  <trace.json> <out_dir> --num N --cap C --model M
#        Materialises one OTel trace file per conversation (Qwen-tokenised filler
#        text, assistant markers substituted at replay) under a sliding context
#        window, producing the directory the OTel drivers/containers replay.
#
# The defaults below reproduce the shipped /mnt/certus1/inference-perf-syn-data/
# otel_1k corpus (conversation_replay_50turn.yaml, seed 42, 1000 convs, 120k cap,
# Qwen2.5-7B-Instruct). Override any of them via env:
#
#   OUT_DIR=/mnt/certus1/inference-perf-syn-data/otel_1k \
#   NUM_CONVS=1000 CAP=120000 MODEL=Qwen/Qwen2.5-7B-Instruct \
#   CONFIG=conversation_replay_50turn.yaml  ./generate-otel-corpus.sh
#
# Requires the inference-perf package importable (python3.12) and the model's
# tokenizer available to transformers (HF cache or network). The corpus is large
# (~22 GB for 1000 convs at a 120k cap) — write it to /mnt/certus1, not the repo.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PY="${PY:-python3.12}"

CONFIG="${CONFIG:-${SCRIPT_DIR}/conversation_replay_50turn.yaml}"
NUM_CONVS="${NUM_CONVS:-1000}"
CAP="${CAP:-120000}"
MODEL="${MODEL:-Qwen/Qwen2.5-7B-Instruct}"
OUT_DIR="${OUT_DIR:-/mnt/certus1/inference-perf-syn-data/otel_${NUM_CONVS}}"
# Intermediate plan trace (compact JSON). Kept so a re-run of stage 2 (e.g. a
# different --cap) does not need to re-run inference-perf.
TRACE_JSON="${TRACE_JSON:-${OUT_DIR%/}.plan.json}"

echo "[gen] config      : ${CONFIG}"
echo "[gen] num convs   : ${NUM_CONVS}"
echo "[gen] context cap : ${CAP}"
echo "[gen] model       : ${MODEL}"
echo "[gen] plan trace  : ${TRACE_JSON}"
echo "[gen] corpus out  : ${OUT_DIR}"
echo

echo "[gen] stage 1/2: inference-perf conversation_replay -> plan trace"
"$PY" "${SCRIPT_DIR}/gen_conversation_trace.py" "$CONFIG" "$TRACE_JSON" "$NUM_CONVS"
echo

echo "[gen] stage 2/2: plan trace -> per-conversation OTel corpus"
"$PY" "${SCRIPT_DIR}/trace_to_otel.py" "$TRACE_JSON" "$OUT_DIR" \
    --num "$NUM_CONVS" --cap "$CAP" --model "$MODEL"
echo

echo "[gen] done. corpus: ${OUT_DIR} ($(ls "$OUT_DIR" | wc -l) files)"
echo "[gen] point a run at it with:  OTEL_HOST=${OUT_DIR} ./run-docker-otel-shmq-prom.sh"
