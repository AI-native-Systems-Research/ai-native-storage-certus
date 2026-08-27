#!/usr/bin/env bash
# Certus-SPDK KV-offload replay — run-to-run STABILITY harness.
#
# Repeats the canonical certus-spdk end-to-end replay (host certus-server over
# SPDK NVMe + vLLM shmq client container) N times with a FIXED config, and
# records the per-run generation throughput so run-to-run variance can be
# measured. Bypasses profile_all.sh's host-reconfigure wrapper (needs sudo) —
# the host is already vfio-bound + hugepaged, so the reconfigure is a no-op and
# this drives the identical certus phase directly.
#
# Config matches profile_all.sh's certus-spdk defaults:
#   4 NVMe devices (61-64), 13G DRAM tier (small enough to spill to NVMe),
#   CHANNELS=32, EVICT 0.6, SLAB 2 MiB, 450 convs x 12 turns, Llama-3-8B.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTDIR="${1:?usage: run_certus_stability.sh <output-dir>}"
PROGRESS="$OUTDIR/progress.log"
mkdir -p "$OUTDIR"

RUNS="${RUNS:-10}"

# --- fixed canonical certus-spdk config (exported to run-docker-certus-shmq.sh) ---
export DEVICE_PCI="0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0"
export MEM_TIER_SIZE="${MEM_TIER_SIZE:-13G}"
export EVICT_THRESH="${EVICT_THRESH:-0.6}"
export CHANNELS="${CHANNELS:-32}"
export SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"
export NUM_CONVS="${NUM_CONVS:-450}"
export MAX_ROUNDS="${MAX_ROUNDS:-0}"
export OUTPUT_TOKENS="${OUTPUT_TOKENS:-150}"   # for tok/s derivation only

echo "START $(date -Is)  runs=$RUNS  num_convs=$NUM_CONVS  max_rounds=$MAX_ROUNDS  mem_tier=$MEM_TIER_SIZE  channels=$CHANNELS  evict=$EVICT_THRESH" | tee "$PROGRESS"

for r in $(seq 1 "$RUNS"); do
  rr=$(printf "%02d" "$r")
  export LOG="$OUTDIR/run${rr}.log"
  export SERVER_LOG="$OUTDIR/server${rr}.log"
  t0=$(date +%s)
  bash "$SCRIPT_DIR/run-docker-certus-shmq.sh" >/dev/null 2>&1
  rc=$?
  t1=$(date +%s)
  wall=$(( t1 - t0 ))
  done_line=$(grep -m1 '^\[run\] DONE' "$LOG" 2>/dev/null)
  gens=$(sed -nE 's/.*generations=([0-9]+).*/\1/p' <<<"$done_line")
  elapsed=$(sed -nE 's/.*elapsed=([0-9.]+)s.*/\1/p' <<<"$done_line")
  gps=$(sed -nE 's/.*\(([0-9.]+) gen\/s\).*/\1/p' <<<"$done_line")
  echo "RUN_DONE $r/$RUNS  rc=$rc  gens=${gens:-?}  elapsed=${elapsed:-?}s  gen_per_s=${gps:-?}  wall=${wall}s" | tee -a "$PROGRESS"
  if [ "$rc" -ne 0 ] || [ -z "$gps" ]; then
    echo "WARN run$rr rc=$rc no-DONE-line (see run${rr}.log / server${rr}.log)" | tee -a "$PROGRESS"
  fi
  sleep 5
done
echo "ALL_DONE $(date -Is)" | tee -a "$PROGRESS"
