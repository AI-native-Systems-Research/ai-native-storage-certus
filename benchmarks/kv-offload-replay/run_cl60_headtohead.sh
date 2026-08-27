#!/usr/bin/env bash
# Closed-loop 60-session head-to-head driver: cputier-fixed then certus-shmq,
# back-to-back (they share the GPU). NUM_CONVS/ACTIVE_SESSIONS overridable.
set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

DS="${DS:-}"
if [[ -z "$DS" ]]; then
  echo "error: DS must be set to a host dataset JSON (e.g. /path/sharegpt_v3.json)" >&2
  exit 1
fi
if [[ ! -f "$DS" ]]; then
  echo "error: dataset not found: DS=$DS" >&2
  exit 1
fi
STAMP="${STAMP:-$(date +%Y%m%d_%H%M%S)}"
NUM_CONVS="${NUM_CONVS:-1000}"
ACTIVE_SESSIONS="${ACTIVE_SESSIONS:-60}"
MAX_ROUNDS="${MAX_ROUNDS:-0}"

CPUTIER_OUT="cl60-cputier-${STAMP}"
SHMQ_OUT="cl60-shmq-${STAMP}"

echo "H2H_START $(date -Is)  num_convs=${NUM_CONVS}  active_sessions=${ACTIVE_SESSIONS}  max_rounds=${MAX_ROUNDS}"
echo "H2H_START cputier -> ${CPUTIER_OUT}"
RUNS=1 NUM_CONVS="$NUM_CONVS" MAX_ROUNDS="$MAX_ROUNDS" \
  WORKLOAD_MODE=async ACTIVE_SESSIONS="$ACTIVE_SESSIONS" \
  DATASET_HOST="$DS" \
  bash ./run_cputier_patched_stability.sh "$CPUTIER_OUT"
echo "H2H_MID cputier done rc=$?  $(date -Is)"

sleep 5

echo "H2H_START shmq -> ${SHMQ_OUT}"
RUNS=1 NUM_CONVS="$NUM_CONVS" MAX_ROUNDS="$MAX_ROUNDS" \
  WORKLOAD_MODE=async ACTIVE_SESSIONS="$ACTIVE_SESSIONS" \
  DATASET_HOST="$DS" \
  bash ./run_certus_stability.sh "$SHMQ_OUT"
echo "H2H_DONE shmq done rc=$?  $(date -Is)"
echo "H2H_ALL_DONE $(date -Is)"
echo "  cputier: ${CPUTIER_OUT}/progress.log"
echo "  shmq:    ${SHMQ_OUT}/progress.log"
