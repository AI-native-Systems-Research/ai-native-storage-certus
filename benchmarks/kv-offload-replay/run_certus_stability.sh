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
# Data-parallel: DP_SIZE>1 fans out that many client replicas (GPUS, one each)
# against the one server; each writes run${rr}.gpu<N>.log. Aggregate throughput
# is sum(generations) / max(elapsed) — the wall-clock envelope, never a sum of
# per-replica gen/s. DP_SIZE=1 keeps the single-log path unchanged.
export DP_SIZE="${DP_SIZE:-1}"
export GPUS="${GPUS:-}"

echo "START $(date -Is)  runs=$RUNS  num_convs=$NUM_CONVS  max_rounds=$MAX_ROUNDS  mem_tier=$MEM_TIER_SIZE  channels=$CHANNELS  evict=$EVICT_THRESH  dp_size=$DP_SIZE" | tee "$PROGRESS"

for r in $(seq 1 "$RUNS"); do
  rr=$(printf "%02d" "$r")
  export LOG="$OUTDIR/run${rr}.log"
  export SERVER_LOG="$OUTDIR/server${rr}.log"
  t0=$(date +%s)
  bash "$SCRIPT_DIR/run-docker-certus-shmq.sh" >/dev/null 2>&1
  rc=$?
  t1=$(date +%s)
  wall=$(( t1 - t0 ))
  if [ "$DP_SIZE" -le 1 ]; then
    done_line=$(grep -m1 '^\[run\] DONE' "$LOG" 2>/dev/null)
    gens=$(sed -nE 's/.*generations=([0-9]+).*/\1/p' <<<"$done_line")
    elapsed=$(sed -nE 's/.*elapsed=([0-9.]+)s.*/\1/p' <<<"$done_line")
    gps=$(sed -nE 's/.*\(([0-9.]+) gen\/s\).*/\1/p' <<<"$done_line")
    echo "RUN_DONE $r/$RUNS  rc=$rc  gens=${gens:-?}  elapsed=${elapsed:-?}s  gen_per_s=${gps:-?}  wall=${wall}s" | tee -a "$PROGRESS"
    if [ "$rc" -ne 0 ] || [ -z "$gps" ]; then
      echo "WARN run$rr rc=$rc no-DONE-line (see run${rr}.log / server${rr}.log)" | tee -a "$PROGRESS"
    fi
  else
    # Aggregate across the per-replica logs: sum generations, take max elapsed
    # (wall-clock envelope), recompute gen/s from the raw ints (not summed rates).
    sum_gens=0; max_elapsed=0; n_done=0; parts=""
    for rlog in "${LOG%.log}".gpu*.log; do
      [ -f "$rlog" ] || continue
      dl=$(grep -m1 '^\[run\] DONE' "$rlog" 2>/dev/null)
      [ -z "$dl" ] && continue
      g=$(sed -nE 's/.*generations=([0-9]+).*/\1/p' <<<"$dl")
      e=$(sed -nE 's/.*elapsed=([0-9.]+)s.*/\1/p' <<<"$dl")
      [ -z "$g" ] && continue
      n_done=$(( n_done + 1 ))
      sum_gens=$(( sum_gens + g ))
      max_elapsed=$(awk -v a="$max_elapsed" -v b="${e:-0}" 'BEGIN{print (b>a)?b:a}')
      parts="${parts}$(basename "$rlog"):g=${g},e=${e}s "
    done
    if [ "$n_done" -gt 0 ] && awk -v e="$max_elapsed" 'BEGIN{exit !(e>0)}'; then
      agg_gps=$(awk -v g="$sum_gens" -v e="$max_elapsed" 'BEGIN{printf "%.1f", g/e}')
    else
      agg_gps=""
    fi
    echo "RUN_DONE $r/$RUNS  rc=$rc  replicas=${n_done}/${DP_SIZE}  gens=${sum_gens}  elapsed=${max_elapsed}s  gen_per_s=${agg_gps:-?}  wall=${wall}s  [${parts}]" | tee -a "$PROGRESS"
    if [ "$rc" -ne 0 ] || [ "$n_done" -lt "$DP_SIZE" ] || [ -z "$agg_gps" ]; then
      echo "WARN run$rr rc=$rc replicas=${n_done}/${DP_SIZE} (see run${rr}.gpu*.log / server${rr}.log)" | tee -a "$PROGRESS"
    fi
  fi
  sleep 5
done
echo "ALL_DONE $(date -Is)" | tee -a "$PROGRESS"
