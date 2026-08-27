#!/usr/bin/env bash
# vLLM-native CPU+FS-spill (Tiered) KV-offload replay — PATCHED arm.
#
# Identical to run_cputier_stability.sh in every config dimension, EXCEPT it
# invokes run-docker-cputier-patched.sh, which bind-mounts the fix #2
# (deferred finished-request finalization) patch over the image's vLLM 0.26.0
# files. Purpose: validate locally that the patch eliminates the tiering
# _req_state KeyError / EngineDeadError crash that made the as-shipped arm
# complete 0/N at 450 convs.
#
# Same matched config as the certus-spdk stability sweep and the as-shipped
# cputier arm: NUM_CONVS=450 x 12 rounds, OUTPUT_TOKENS=150, MAX_NUM_SEQS=64,
# MAX_MODEL_LEN=8192, Llama-3-8B, float16, ENFORCE_EAGER=0, CPU_BYTES=13G,
# fs spill to /mnt/certus1. Only the three patched vLLM files differ, so any
# change in reliability/throughput is attributable to the patch alone.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTDIR="${1:?usage: run_cputier_patched_stability.sh <output-dir>}"
PROGRESS="$OUTDIR/progress.log"
mkdir -p "$OUTDIR"

RUNS="${RUNS:-10}"

# --- fixed canonical tiered-cpu-fs config (exported to run-docker-cputier-patched.sh) ---
# Patched arm: certus-offload-bench-fix026 (build_026.sh builds it with
# --build-arg VLLM_FIX_TIERING=1) has the fork tiering fix baked in, so no
# runtime bind-mount is needed. Config is otherwise identical to the stock arm.
export IMAGE="${IMAGE:-certus-offload-bench-fix026}"
export CPU_BYTES="${CPU_BYTES:-$((13 * (1 << 30)))}"   # 13G CPU primary == certus DRAM tier
export DISK_DIR_HOST="${DISK_DIR_HOST:-/mnt/certus1/kv-fs-tier}"
export DISK_READ_THREADS="${DISK_READ_THREADS:-16}"
export DISK_WRITE_THREADS="${DISK_WRITE_THREADS:-16}"
export NUM_CONVS="${NUM_CONVS:-450}"
export MAX_ROUNDS="${MAX_ROUNDS:-0}"
export OUTPUT_TOKENS="${OUTPUT_TOKENS:-150}"
export MAX_NUM_SEQS="${MAX_NUM_SEQS:-64}"
export MAX_MODEL_LEN="${MAX_MODEL_LEN:-8192}"

echo "START $(date -Is)  PATCHED(fix#2)  runs=$RUNS  num_convs=$NUM_CONVS  max_rounds=$MAX_ROUNDS  cpu_tier=$((CPU_BYTES/(1<<30)))G  fs_tier=$DISK_DIR_HOST  output_tokens=$OUTPUT_TOKENS" | tee "$PROGRESS"

for r in $(seq 1 "$RUNS"); do
  rr=$(printf "%02d" "$r")
  export LOG="$OUTDIR/run${rr}.log"
  rm -rf "${DISK_DIR_HOST:?}/"* 2>/dev/null
  t0=$(date +%s)
  bash "$SCRIPT_DIR/run-docker-cputier-patched.sh" >/dev/null 2>&1
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
    lastround=$(grep -oE '^\[run\] round [0-9]+' "$LOG" 2>/dev/null | grep -oE '[0-9]+' | tail -1)
    keyerr=$(grep -m1 -E '_req_state|KeyError|EngineDeadError' "$LOG" 2>/dev/null | head -c 80)
    echo "RUN_DONE $r/$RUNS  rc=$rc  status=CRASH  died_round=${lastround:-?}  gen_per_s=?  wall=${wall}s  err=[${keyerr:-none}]" | tee -a "$PROGRESS"
    echo "WARN run$rr CRASH rc=$rc died_round=${lastround:-?} (see run${rr}.log)" | tee -a "$PROGRESS"
  fi
  sleep 5
done
echo "ALL_DONE $(date -Is)" | tee -a "$PROGRESS"
