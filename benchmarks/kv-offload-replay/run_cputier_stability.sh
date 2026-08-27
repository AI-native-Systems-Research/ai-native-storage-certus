#!/usr/bin/env bash
# vLLM-native CPU+FS-spill (Tiered) KV-offload replay — run-to-run STABILITY /
# RELIABILITY harness, as the B arm of a head-to-head vs certus-spdk.
#
# Repeats the canonical tiered-cpu-fs end-to-end replay (vLLM 0.26
# OffloadingConnector -> TieringOffloadingSpec: CPU primary tier + "fs" disk
# secondary tier) N times with a FIXED config MATCHED to the certus-spdk
# stability sweep, and records per-run generation throughput + completion.
#
# Config matched to run_certus_stability.sh's certus arm:
#   NUM_CONVS=450 x 12 rounds, OUTPUT_TOKENS=150, MAX_NUM_SEQS=64,
#   MAX_MODEL_LEN=8192, Llama-3-8B, float16, graphs on (ENFORCE_EAGER=0).
#   Hot tier: CPU_BYTES=13G  == certus 13G DRAM tier (equal-tier comparison).
#   Cold tier: fs spill to /mnt/certus1 (nvme8, Kioxia XFS) — certus's SPDK
#   over 61-64 cannot share a device with a kernel fs, so the disk hardware
#   necessarily differs; this is a software-stack comparison.
#
# gen/s is defined identically to the certus arm: generations / replay-wall
# (the driver's "[run] done. wall=..s" is the replay loop only, excl. model load).
#
# Crash handling: the tiered backend intermittently hits an upstream vLLM 0.26
# bug (tiering/manager.py _req_state KeyError -> EngineDeadError) whose
# probability grows with context depth. A run with no "[run] done." line is
# recorded as CRASH with its round-of-death; crashes are kept as reliability data
# (completion rate), NOT retried.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTDIR="${1:?usage: run_cputier_stability.sh <output-dir>}"
PROGRESS="$OUTDIR/progress.log"
mkdir -p "$OUTDIR"

RUNS="${RUNS:-10}"

# --- fixed canonical tiered-cpu-fs config (exported to run-docker-cputier.sh) ---
# Stock certus-offload-bench (built by build_026.sh from Dockerfile.offload):
# vLLM 0.26 + the tiering framework, tiering activated at run time by DISK_DIR /
# SECONDARY_TIER. This is the AS-SHIPPED arm — it reproduces the upstream
# _req_state KeyError crash. The patched arm uses certus-offload-bench-fix026
# (see run_cputier_patched_stability.sh).
export IMAGE="${IMAGE:-certus-offload-bench}"
export CPU_BYTES="${CPU_BYTES:-$((13 * (1 << 30)))}"   # 13G CPU primary == certus DRAM tier
export DISK_DIR_HOST="${DISK_DIR_HOST:-/mnt/certus1/kv-fs-tier}"
export DISK_READ_THREADS="${DISK_READ_THREADS:-16}"
export DISK_WRITE_THREADS="${DISK_WRITE_THREADS:-16}"
export NUM_CONVS="${NUM_CONVS:-450}"
export MAX_ROUNDS="${MAX_ROUNDS:-0}"
export OUTPUT_TOKENS="${OUTPUT_TOKENS:-150}"   # match certus arm (common default is 150)
export MAX_NUM_SEQS="${MAX_NUM_SEQS:-64}"
export MAX_MODEL_LEN="${MAX_MODEL_LEN:-8192}"

echo "START $(date -Is)  runs=$RUNS  num_convs=$NUM_CONVS  max_rounds=$MAX_ROUNDS  cpu_tier=$((CPU_BYTES/(1<<30)))G  fs_tier=$DISK_DIR_HOST  output_tokens=$OUTPUT_TOKENS" | tee "$PROGRESS"

for r in $(seq 1 "$RUNS"); do
  rr=$(printf "%02d" "$r")
  export LOG="$OUTDIR/run${rr}.log"
  # Fresh fs tier each run — certus starts a fresh server + fresh tiers per run,
  # so clearing the spill dir keeps external-prefix-cache carryover out of the
  # per-run measurement (matches certus's no-carryover semantics).
  rm -rf "${DISK_DIR_HOST:?}/"* 2>/dev/null
  t0=$(date +%s)
  bash "$SCRIPT_DIR/run-docker-cputier.sh" >/dev/null 2>&1
  rc=$?
  t1=$(date +%s)
  wall=$(( t1 - t0 ))

  done_line=$(grep -m1 '^\[run\] done\.' "$LOG" 2>/dev/null)
  if [ -n "$done_line" ]; then
    gens=$(sed -nE 's/.*generations=([0-9]+).*/\1/p' <<<"$done_line")
    replay=$(sed -nE 's/.*wall=([0-9.]+)s.*/\1/p' <<<"$done_line")
    rounds=$(sed -nE 's/.*rounds=([0-9]+).*/\1/p' <<<"$done_line")
    gps=$(awk -v g="${gens:-0}" -v w="${replay:-0}" 'BEGIN{ if(w>0) printf "%.2f", g/w; else print "?" }')
    echo "RUN_DONE $r/$RUNS  rc=$rc  status=OK  gens=${gens:-?}  replay=${replay:-?}s  rounds=${rounds:-?}  gen_per_s=${gps:-?}  wall=${wall}s" | tee -a "$PROGRESS"
  else
    # Crash / no completion — record round-of-death + whether it's the known bug.
    lastround=$(grep -oE '^\[run\] round [0-9]+' "$LOG" 2>/dev/null | grep -oE '[0-9]+' | tail -1)
    keyerr=$(grep -m1 -E '_req_state|KeyError|EngineDeadError' "$LOG" 2>/dev/null | head -c 80)
    echo "RUN_DONE $r/$RUNS  rc=$rc  status=CRASH  died_round=${lastround:-?}  gen_per_s=?  wall=${wall}s  err=[${keyerr:-none}]" | tee -a "$PROGRESS"
    echo "WARN run$rr CRASH rc=$rc died_round=${lastround:-?} (see run${rr}.log)" | tee -a "$PROGRESS"
  fi
  sleep 5
done
echo "ALL_DONE $(date -Is)" | tee -a "$PROGRESS"
