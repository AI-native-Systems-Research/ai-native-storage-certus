#!/bin/bash
# wedge-watch-live.sh — attach-mode wedge watchdog used by the sweep driver.
#
# Why this exists alongside run-cputier-wedge-capture.sh: aiperf retitles its
# process to "aiperf system_controller" and drops argv, so the stock watchdog's
# "client RE + results-dir path" cmdline match can never find the client — the
# 2026-11-18/19 runs attached (or would have) but watched nothing. This variant
# matches the client by process name only.
#
# Stall = running==0 && waiting>0 && request_success_total unchanged for
# STALL_SECONDS. On stall: py-spy dumps (EngineCore first) into ARM/WEDGE-<time>/,
# a WEDGED marker, and the client is stopped so the matrix finishes the arm
# normally and the sweep continues.
#
#   ARM=cputier-nointervals ATTACH_PID=<matrix pid> RESULTS_DIR=<matrix dir> \
#     CONCURRENT_STREAMS=16 STALL_SECONDS=300 ./wedge-watch-live.sh
set -uo pipefail

ARM="${ARM:?ARM required}"
ATTACH_PID="${ATTACH_PID:?ATTACH_PID (matrix pid) required}"
RESULTS_DIR="${RESULTS_DIR:?RESULTS_DIR required}"
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-16}"
STALL_SECONDS="${STALL_SECONDS:-300}"
POLL_SECONDS="${POLL_SECONDS:-10}"
CLIENT_RE='aiperf (profile --url|system_controller)'
PY_SPY="${PY_SPY:-$(command -v py-spy || echo "$HOME/.local/bin/py-spy")}"
SUB="${RESULTS_DIR}/${ARM}"
WLOG="${RESULTS_DIR}/watchdog.log"

mkdir -p "$SUB"
wlog(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$WLOG"; }
metric(){ awk -v m="$1" '$1 ~ "^"m"[{]" {s += $2} END {printf "%d", s}' <<<"$2"; }
container(){ podman ps --format '{{.ID}} {{.Image}}' | awk '/certus-offload/ {print $1; exit}'; }

capture(){
  local dir="${SUB}/WEDGE-$(date +%H%M%S)" c pid
  mkdir -p "$dir"
  wlog "capturing stuck engine -> $dir"
  ps -eo pid,ppid,etime,stat,wchan:32,args --forest > "$dir/ps.txt"
  c=$(container)
  if [[ -z "$c" ]]; then wlog "  ERROR: no certus-offload container running"; else
    podman cp "$(readlink -f "$PY_SPY")" "$c:/tmp/py-spy"
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
    echo "arm ${ARM} (${CONCURRENT_STREAMS} streams) WEDGED at $(date '+%F %T %Z'): 0 running, waiting>0, no completed"
    echo "request for ${STALL_SECONDS}s+. Captures in $(basename "$dir")/. Results from this arm are not valid."
  } > "$SUB/WEDGED"
}

stop_client(){ pkill -TERM -f "$CLIENT_RE" 2>/dev/null; }

wlog "live-watch ${ARM}: attach matrix pid ${ATTACH_PID}, cap ${CONCURRENT_STREAMS}, stall=${STALL_SECONDS}s -> ${RESULTS_DIR}"
[[ -x "$PY_SPY" ]] || { wlog "ERROR: py-spy not found (PY_SPY=$PY_SPY)"; exit 1; }

if ! pgrep -f "$CLIENT_RE" >/dev/null; then
  # client not up yet (vLLM model load takes a few minutes) — wait for it
  until pgrep -f "$CLIENT_RE" >/dev/null || ! kill -0 "$ATTACH_PID" 2>/dev/null; do sleep 5; done
fi
wlog "client running; watching for a stall"

last_done=-1 stalled_since="" captured=0
while kill -0 "$ATTACH_PID" 2>/dev/null && pgrep -f "$CLIENT_RE" >/dev/null; do
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
while kill -0 "$ATTACH_PID" 2>/dev/null; do sleep 5; done
wlog "done -> ${RESULTS_DIR} (WEDGE-* captures in ${ARM}/ if it stalled)"
