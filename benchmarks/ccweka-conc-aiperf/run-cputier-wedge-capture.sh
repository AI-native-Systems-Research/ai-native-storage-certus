#!/bin/bash
# run-cputier-wedge-capture.sh — re-run ONE arm of the GQA iobound matrix (default
# cputier-intervals at 1000 streams, 128G tier: the arm that wedged in
# gqa-stream-sweep-20261028_155048) under a watchdog that captures the stuck engine
# before anything is killed.
#
# Stall = vLLM reports 0 running, >0 waiting and request_success_total has not moved
# for STALL_SECONDS. On a stall it writes into the arm's dir (WEDGE-<time>/):
#   py-spy-<pid>.txt / py-spy-<pid>-locals.txt  every vLLM process (EngineCore first)
#   metrics.txt                                 /metrics with a long timeout
#   ps.txt, threads-<pid>.txt                   process tree and per-thread states
# (pids are container-namespace; py-spy is copied into the vLLM container and run
# there) then touches WEDGED and stops the guidellm client so the matrix finishes the arm
# (stops vLLM, writes report.html). With no stall it just runs the arm to completion.
#
#   ./run-cputier-wedge-capture.sh
#   STALL_SECONDS=60 ARM=cputier-nointervals ./run-cputier-wedge-capture.sh
#   ATTACH_PID=<matrix pid> RESULTS_DIR=<its dir> ./run-cputier-wedge-capture.sh  # watch a run already going
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ARM="${ARM:-cputier-intervals}"
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-1000}"
CPU_TIER_GB="${CPU_TIER_GB:-128}"
MAX_SECONDS="${MAX_SECONDS:-1000}"
STALL_SECONDS="${STALL_SECONDS:-120}"
POLL_SECONDS="${POLL_SECONDS:-10}"
CLIENT_RE="${CLIENT_RE:-(guidellm run|aiperf profile)}"   # the load generator's command line (either client)
PY_SPY="${PY_SPY:-$(command -v py-spy || echo "$HOME/.local/bin/py-spy")}"
RESULTS_DIR="${RESULTS_DIR:-/mnt/certus1/vllm-serve-matrix/gqa-wedge-capture-$(date +%Y%m%d_%H%M%S)-${ARM}-${CONCURRENT_STREAMS}streams}"
SUB="${RESULTS_DIR}/${ARM}"
WLOG="${RESULTS_DIR}/watchdog.log"

mkdir -p "$SUB"
wlog(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$WLOG"; }
metric(){ awk -v m="$1" '$1 ~ "^"m"[{]" {s += $2} END {printf "%d", s}' <<<"$2"; }

container(){ podman ps --format '{{.ID}} {{.Image}}' | awk '/certus-offload/ {print $1; exit}'; }

capture(){
  # py-spy runs INSIDE the container: from the host it cannot find the
  # containerised interpreter ("Failed to find python version").
  local dir="${SUB}/WEDGE-$(date +%H%M%S)" c pid
  mkdir -p "$dir"
  wlog "capturing stuck engine -> $dir"
  ps -eo pid,ppid,etime,stat,wchan:32,args --forest > "$dir/ps.txt"
  c=$(container)
  if [[ -z "$c" ]]; then wlog "  ERROR: no certus-offload container running"; else
    podman cp "$(readlink -f "$PY_SPY")" "$c:/tmp/py-spy"
    # EngineCore first, then the API server (container-namespace pids).
    for pid in $(podman exec "$c" sh -c "{ pgrep -f EngineCore; pgrep -f 'vllm serve'; } | awk '!s[\$0]++'"); do
      wlog "  py-spy dump container pid $pid"
      podman exec "$c" timeout 120 /tmp/py-spy dump --pid "$pid" > "$dir/py-spy-$pid.txt" 2>&1
      podman exec "$c" timeout 180 /tmp/py-spy dump --pid "$pid" --locals > "$dir/py-spy-$pid-locals.txt" 2>&1
      podman exec "$c" sh -c "for t in /proc/$pid/task/*; do echo \"\${t##*/} \$(cat \$t/comm) \$(cut -d' ' -f3 \$t/stat) \$(cat \$t/wchan 2>/dev/null)\"; done" \
        > "$dir/threads-$pid.txt" 2>&1
    done
  fi
  curl -s --max-time 60 http://127.0.0.1:8000/metrics > "$dir/metrics.txt" \
    || wlog "  /metrics did not answer within 60 s"
  {
    echo "arm ${ARM} WEDGED at $(date '+%F %T %Z'): 0 running, waiting>0, no completed"
    echo "request for ${STALL_SECONDS}s+. Captures in $(basename "$dir")/. Results from this arm are not valid."
  } > "$SUB/WEDGED"
}

stop_client(){ pkill -TERM -f "${CLIENT_RE} .*${SUB}/" 2>/dev/null; }

wlog "re-run ${ARM}: ${CONCURRENT_STREAMS} streams, ${CPU_TIER_GB}G tier, MAX_SECONDS=${MAX_SECONDS}, stall=${STALL_SECONDS}s -> ${RESULTS_DIR}"
[[ -x "$PY_SPY" ]] || { wlog "ERROR: py-spy not found (PY_SPY=$PY_SPY)"; exit 1; }

if [[ -n "${ATTACH_PID:-}" ]]; then
  # Watch a matrix run that is already going (RESULTS_DIR must be its dir).
  matrix_pid=$ATTACH_PID
  wlog "attaching to running matrix pid ${matrix_pid}"
else
  RESULTS_DIR="$RESULTS_DIR" ARMS="$ARM" CONCURRENT_STREAMS="$CONCURRENT_STREAMS" \
    CPU_TIER_GB="$CPU_TIER_GB" MAX_SECONDS="$MAX_SECONDS" \
    "${SCRIPT_DIR}/run-gqa-iobound-matrix.sh" >> "${RESULTS_DIR}/matrix.log" 2>&1 &
  matrix_pid=$!
fi

# Watch only once the client is running (vLLM model load takes a few minutes).
until pgrep -f "${CLIENT_RE} .*${SUB}/" >/dev/null || ! kill -0 "$matrix_pid" 2>/dev/null; do sleep 5; done
wlog "client running; watching for a stall"

last_done=-1 stalled_since="" captured=0
while kill -0 "$matrix_pid" 2>/dev/null && pgrep -f "${CLIENT_RE} .*${SUB}/" >/dev/null; do
  m=$(curl -s --max-time 30 http://127.0.0.1:8000/metrics) || m=""
  if [[ -n "$m" ]]; then
    running=$(metric vllm:num_requests_running "$m")
    waiting=$(metric vllm:num_requests_waiting "$m")
    done_n=$(metric vllm:request_success_total "$m")
  else
    running=0 waiting=1 done_n=$last_done   # an unresponsive server counts as no progress
  fi
  if (( running == 0 && waiting > 0 && done_n == last_done )); then
    stalled_since="${stalled_since:-$(date +%s)}"
    if (( $(date +%s) - stalled_since >= STALL_SECONDS )); then
      wlog "STALL: running=$running waiting=$waiting completed=$done_n unchanged for ${STALL_SECONDS}s"
      capture; captured=1; stop_client; break
    fi
  else
    stalled_since=""
  fi
  last_done=$done_n
  sleep "$POLL_SECONDS"
done

(( captured )) || wlog "no stall detected while the client ran"
while kill -0 "$matrix_pid" 2>/dev/null; do sleep 5; done   # `wait` only works on a child
wlog "done -> ${RESULTS_DIR} (report.html; WEDGE-* captures in ${ARM}/ if it stalled)"
