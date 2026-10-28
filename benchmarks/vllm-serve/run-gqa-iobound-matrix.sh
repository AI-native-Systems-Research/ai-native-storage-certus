#!/bin/bash
# run-gqa-iobound-matrix.sh — GQA counterpart of run-mha-iobound-matrix.sh. Runs the
# KV-IO experiment across the 2x2 matrix {certus-shmq, cputier} x {no intervals,
# intervals} against the GQA Qwen2.5-7B-Instruct (56 KiB/token) instead of the MHA
# Qwen1.5-7B (512 KiB/token), collects every artifact into one new results dir, and
# generates an HTML comparison report (throughput + TTFT + KV-IO plots). Each arm
# (re)provisions its backend from cold and runs to completion (MAX_SECONDS per arm).
#
# GQA I/O NOTE: at the client default PREFIX_COUNT=64 the reused working set is only
# ~28 GiB (vs ~256 GiB on the MHA matrix) and fits the 32 G DRAM/CPU tier — so the
# default matrix is DRAM-resident, which is the honest same-workload MHA->GQA result.
# To reproduce the MHA matrix's SSD-read regime at matched byte volume, run with
# PREFIX_COUNT=512 (~224 GiB), or lower CPU_TIER_GB on every arm.
#
# Prereqgs already satisfied on this box (checked before writing): certus-server-yaml
# built, NVMe on vfio-pci + hugepages, both vLLM 0.30 images present, GPUs free.
# No sudo is used — the certus arm starts the server via start-certus-optimized.sh
# (numactl + the prebuilt binary), the cputier arm is self-contained.
#
#   ./run-gqa-iobound-matrix.sh                 # all 4 arms, 1000 s each (~90 min incl. model load)
#   PREFIX_COUNT=512 ./run-gqa-iobound-matrix.sh # byte-match the MHA matrix (~224 GiB -> SSD)
#   MAX_SECONDS=120 ./run-gqa-iobound-matrix.sh # quick smoke
#   ARMS="certus-nointervals cputier-nointervals" ./run-gqa-iobound-matrix.sh  # subset
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

STAMP="$(date +%Y%m%d_%H%M%S)"
RESULTS_DIR="${RESULTS_DIR:-/mnt/certus1/vllm-serve-matrix/gqa-iobound-${STAMP}}"
MAX_SECONDS="${MAX_SECONDS:-1000}"
CPU_TIER_GB="${CPU_TIER_GB:-32}"
ARMS="${ARMS:-certus-nointervals certus-intervals cputier-nointervals cputier-intervals}"
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
FS_TIER_HOST="${FS_TIER_HOST:-/raid0/kv-fs-tier}"
VLLM_READY_TIMEOUT="${VLLM_READY_TIMEOUT:-900}"   # model load (first run downloads ~15G)
LOG="${RESULTS_DIR}/orchestrator.log"

mkdir -p "$RESULTS_DIR"
log(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$LOG"; }

CERTUS_PGID=""   # certus-server process group
VLLM_PGID=""     # current vLLM container launcher process group

stop_certus(){
  [[ -n "$CERTUS_PGID" ]] || { pgrep -f 'certus-server-yaml.*--shm-path' >/dev/null || return 0; }
  log "stopping certus-server"
  [[ -n "$CERTUS_PGID" ]] && kill -TERM "-$CERTUS_PGID" 2>/dev/null
  pkill -TERM -f 'certus-server-yaml.*--shm-path' 2>/dev/null
  for _ in $(seq 1 30); do pgrep -f 'certus-server-yaml.*--shm-path' >/dev/null || break; sleep 1; done
  pkill -KILL -f 'certus-server-yaml.*--shm-path' 2>/dev/null
  rm -f "$SHM_PATH" 2>/dev/null
  CERTUS_PGID=""
}

start_certus(){
  stop_certus
  log "starting certus-server (${CPU_TIER_GB}G tier, --format)"
  setsid env MEMORY_TIER_SIZE="${CPU_TIER_GB}G" CHANNELS=32 SHM_PATH="$SHM_PATH" \
    "${SCRIPT_DIR}/start-certus-optimized.sh" >"${RESULTS_DIR}/certus-server.log" 2>&1 &
  CERTUS_PGID=$!
  for _ in $(seq 1 90); do [[ -e "$SHM_PATH" ]] && { sleep 3; log "certus-server up (mailbox ${SHM_PATH})"; return 0; }; sleep 1; done
  log "ERROR: certus-server mailbox did not appear; see certus-server.log"; return 1
}

stop_vllm(){
  [[ -n "$VLLM_PGID" ]] && kill -TERM "-$VLLM_PGID" 2>/dev/null
  # Fallback: stop any serving container in either store.
  for st in "" "--root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run"; do
    ids=$(podman $st ps -q --filter "ancestor=localhost/certus-shmq-connector:vllm0.30.0" --filter "ancestor=certus-offload:vllm0.30.0" 2>/dev/null)
    [[ -n "$ids" ]] && podman $st stop -t 5 $ids >/dev/null 2>&1
  done
  for _ in $(seq 1 60); do curl -fsS --max-time 3 http://127.0.0.1:8000/v1/models >/dev/null 2>&1 || break; sleep 1; done
  # The next arm's container binds port 8000; wait until the old one has released
  # it ("pasta ... Address already in use" otherwise).
  for _ in $(seq 1 120); do ss -Htln 'sport = :8000' 2>/dev/null | grep -q . || break; sleep 1; done
  VLLM_PGID=""
}

start_vllm(){  # $1 = serve script, $2 = subdir
  log "launching vLLM: $1"
  setsid env CPU_TIER_GB="$CPU_TIER_GB" ACK_MEM=1 ACK_DRAM=1 \
    "${SCRIPT_DIR}/$1" >"${2}/server.log" 2>&1 &
  VLLM_PGID=$!
}

wait_vllm(){  # returns 0 when /v1/models answers, 1 on timeout
  for _ in $(seq 1 "$VLLM_READY_TIMEOUT"); do
    curl -fsS --max-time 3 http://127.0.0.1:8000/v1/models >/dev/null 2>&1 && { log "vLLM ready"; return 0; }
    kill -0 "$VLLM_PGID" 2>/dev/null || { log "ERROR: vLLM launcher exited early"; return 1; }
    sleep 1
  done
  log "ERROR: vLLM not ready after ${VLLM_READY_TIMEOUT}s"; return 1
}

run_arm(){  # $1 = key
  local key="$1" sub="${RESULTS_DIR}/$1" serve client backend
  mkdir -p "$sub"
  case "$key" in
    certus-nointervals) serve=run-serve-certus-shmq-gqa-iobound.sh; client=run-guidellm-gqa-iobound.sh; backend=certus ;;
    certus-intervals)   serve=run-serve-certus-shmq-gqa-iobound.sh; client=run-guidellm-gqa-iobound-intervals.sh; backend=certus ;;
    cputier-nointervals) serve=run-serve-cputier-gqa-iobound.sh; client=run-guidellm-gqa-iobound.sh; backend=cputier ;;
    cputier-intervals)   serve=run-serve-cputier-gqa-iobound.sh; client=run-guidellm-gqa-iobound-intervals.sh; backend=cputier ;;
    *) log "unknown arm $key"; return 1 ;;
  esac

  log "===== ARM ${key} (backend=${backend}) ====="
  if [[ "$backend" == certus ]]; then
    start_certus || { touch "$sub/FAILED"; return 1; }
  else
    stop_certus
    rm -rf "${FS_TIER_HOST:?}/"* 2>/dev/null; mkdir -p "$FS_TIER_HOST"
  fi

  start_vllm "$serve" "$sub"
  if ! wait_vllm; then touch "$sub/FAILED"; stop_vllm; [[ "$backend" == certus ]] && stop_certus; return 1; fi

  log "polling + client (MAX_SECONDS=${MAX_SECONDS})"
  setsid env OUT="${sub}/kvio.csv" DURATION=$((MAX_SECONDS + 30)) \
    "${SCRIPT_DIR}/poll-kv-metrics.sh" >"${sub}/poller.log" 2>&1 &
  local poll_pgid=$!

  # Forward the workload knobs so they reach the client AND are recorded for the
  # report. Only pass those actually set (empty => the client's own default).
  env OUTPUT="${sub}/guidellm.json" MAX_SECONDS="$MAX_SECONDS" CPU_TIER_GB="$CPU_TIER_GB" \
    ${TURNS:+TURNS="$TURNS"} ${CONCURRENT_STREAMS:+CONCURRENT_STREAMS="$CONCURRENT_STREAMS"} \
    ${PREFIX_COUNT:+PREFIX_COUNT="$PREFIX_COUNT"} ${DELAY_SCALE:+DELAY_SCALE="$DELAY_SCALE"} \
    "${SCRIPT_DIR}/$client" >"${sub}/client.log" 2>&1 || log "client exited nonzero (continuing)"

  kill -TERM "-$poll_pgid" 2>/dev/null
  stop_vllm
  if [[ "$backend" == certus ]]; then
    stop_certus
    # Each Certus arm restarts the server, which overwrites certus-server.log; keep
    # this arm's copy (its final tier-events line splits its loads by source tier).
    cp "${RESULTS_DIR}/certus-server.log" "${sub}/certus-server.log" 2>/dev/null || true
  fi
  log "arm ${key} done -> ${sub}"
}

cleanup(){ log "cleanup"; stop_vllm; stop_certus; }
trap cleanup EXIT INT TERM

log "results -> ${RESULTS_DIR}  arms: ${ARMS}  MAX_SECONDS=${MAX_SECONDS}  CPU_TIER_GB=${CPU_TIER_GB}"
for arm in $ARMS; do run_arm "$arm" || log "arm ${arm} FAILED (continuing)"; done

log "generating report"
# Pass the workload knobs through so the report records them (unset => defaults).
env ${TURNS:+TURNS="$TURNS"} ${CONCURRENT_STREAMS:+CONCURRENT_STREAMS="$CONCURRENT_STREAMS"} \
    ${PREFIX_COUNT:+PREFIX_COUNT="$PREFIX_COUNT"} ${DELAY_SCALE:+DELAY_SCALE="$DELAY_SCALE"} \
    /usr/bin/python3 "${SCRIPT_DIR}/gen-gqa-iobound-report.py" "$RESULTS_DIR" 2>&1 | tee -a "$LOG"
log "DONE -> ${RESULTS_DIR}/report.html"
