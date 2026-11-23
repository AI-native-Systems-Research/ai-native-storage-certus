#!/bin/bash
# wedge-watch-live.sh — attach-mode wedge watchdog used by the sweep driver.
#
# Why this exists alongside run-cputier-wedge-capture.sh: aiperf retitles its
# process to "aiperf system_controller" and drops argv, so the stock watchdog's
# "client RE + results-dir path" cmdline match can never find the client — the
# 2026-11-18/19 runs attached (or would have) but watched nothing. This variant
# matches the client by process name only.
#
# Watches EVERY arm the matrix runs (certus and cputier), one client at a time,
# until the matrix exits. The arm being watched is read from the matrix's own
# orchestrator.log ("===== ARM <name>"), so markers land in the right arm dir;
# ARM= is only a fallback label. (v1 hard-coded ARM=cputier-nointervals, so the
# 2026-11-19 certus stalls were filed under cputier, and it stopped after the
# first stall, leaving later arms unwatched.)
#
# Stall = running==0 && waiting>0 && request_success_total unchanged for
# STALL_SECONDS. On stall: optional SIGUSR2 state dumps (DEBUG_DUMP=1, needs the
# diag/sitecustomize.py hook mounted), py-spy dumps (EngineCore first) into
# <arm>/WEDGE-<time>/, a WEDGED marker, and the client is stopped so the matrix
# finishes the arm normally and the sweep continues.
#
#   ATTACH_PID=<matrix pid> RESULTS_DIR=<matrix dir> \
#     CONCURRENT_STREAMS=16 STALL_SECONDS=300 ./wedge-watch-live.sh
set -uo pipefail

ARM_FALLBACK="${ARM:-unknown-arm}"
ATTACH_PID="${ATTACH_PID:?ATTACH_PID (matrix pid) required}"
RESULTS_DIR="${RESULTS_DIR:?RESULTS_DIR required}"
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-16}"
STALL_SECONDS="${STALL_SECONDS:-300}"
POLL_SECONDS="${POLL_SECONDS:-10}"
# Anchored so it can only match the real client: an unanchored -f pattern also
# matches OTHER tools' pgrep/pkill processes whose cmdline embeds the pattern
# (the stop-rule polls with the same regex) — that race made the v1 watchdog
# false-detect "client running" during preflight and exit before the client
# ever started, missing both cap-32 stalls on 2026-11-19.
CLIENT_RE='^aiperf (profile --url|system_controller)'
PY_SPY="${PY_SPY:-$(command -v py-spy || echo "$HOME/.local/bin/py-spy")}"
WLOG="${RESULTS_DIR}/watchdog.log"
# cputier runs in the default podman store; certus (shmq) in the /mnt/certus1 store.
ALT_STORE=(--root "${PODMAN_STORE:-/mnt/certus1/podman/storage}" --runroot "${PODMAN_RUNROOT:-/mnt/certus1/podman/run}")
IMAGE_RE='certus-offload|certus-shmq-connector'

wlog(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$WLOG"; }
metric(){ awk -v m="$1" '$1 ~ "^"m"[{]" {s += $2} END {printf "%d", s}' <<<"$2"; }

current_arm(){
  local a
  a=$(grep -oE '===== ARM [^ ]+' "${RESULTS_DIR}/orchestrator.log" 2>/dev/null | tail -1 | awk '{print $3}')
  echo "${a:-$ARM_FALLBACK}"
}

# Sets PODMAN (command + store flags) and C (container id) for the serving container.
find_container(){
  local id
  PODMAN=(podman); C=""
  id=$(podman ps --format '{{.ID}} {{.Image}}' 2>/dev/null | awk -v re="$IMAGE_RE" '$2 ~ re {print $1; exit}')
  if [[ -z "$id" ]]; then
    id=$(podman "${ALT_STORE[@]}" ps --format '{{.ID}} {{.Image}}' 2>/dev/null | awk -v re="$IMAGE_RE" '$2 ~ re {print $1; exit}')
    [[ -n "$id" ]] && PODMAN=(podman "${ALT_STORE[@]}")
  fi
  C="$id"
}

capture(){
  local arm="$1" sub dir pid pids epids ep round
  sub="${RESULTS_DIR}/${arm}"; dir="${sub}/WEDGE-$(date +%H%M%S)"
  mkdir -p "$dir"
  wlog "capturing stuck engine (${arm}) -> $dir"
  ps -eo pid,ppid,etime,stat,wchan:32,args --forest > "$dir/ps.txt"
  find_container
  if [[ -z "$C" ]]; then wlog "  ERROR: no serving container found in either podman store"; else
    wlog "  container $C (${PODMAN[*]})"
    "${PODMAN[@]}" cp "$(readlink -f "$PY_SPY")" "$C:/tmp/py-spy"
    # Find the engine by its cmdline (VLLM::EngineCore) or the server, scanning /proc
    # inside the container and skipping this lookup shell. Log what was found, and if
    # nothing matched, save the container's process list so the next capture shows why.
    pids=$("${PODMAN[@]}" exec "$C" sh -c 'for p in /proc/[0-9]*; do [ "${p#/proc/}" = "$$" ] && continue; a=$(tr "\0" " " < "$p/cmdline" 2>/dev/null); case "$a" in *VLLM::Engine*|*"vllm serve"*) echo "${p#/proc/}";; esac; done' 2>>"$dir/lookup-stderr.txt" | awk '!s[$0]++')
    wlog "  engine/server pids in container: ${pids:-none}"
    if [[ -z "$pids" ]]; then
      "${PODMAN[@]}" exec "$C" sh -c 'for p in /proc/[0-9]*; do echo "${p#/proc/} $(tr "\0" " " < "$p/cmdline" 2>/dev/null)"; done' > "$dir/container-procs.txt" 2>&1
    fi
    # DEBUG_DUMP=1: ask the EngineCore's sitecustomize hook (diag/sitecustomize.py)
    # for two scheduler/connector state dumps, 20 s apart, before py-spy pauses it.
    # Only signal when the hook is mounted — SIGUSR2's default action kills the engine.
    if [[ "${DEBUG_DUMP:-0}" == "1" ]]; then
      if "${PODMAN[@]}" exec "$C" test -f /opt/certus-debug/sitecustomize.py; then
        epids=$("${PODMAN[@]}" exec "$C" sh -c 'for p in /proc/[0-9]*; do case "$(cat $p/comm 2>/dev/null)" in VLLM::Engine*) echo "${p#/proc/}";; esac; done')
        for round in 1 2; do
          for ep in $epids; do wlog "  SIGUSR2 state dump #${round} -> engine pid $ep"; "${PODMAN[@]}" exec "$C" kill -USR2 "$ep"; done
          [[ $round == 1 ]] && sleep 20
        done
        sleep 5
      else
        wlog "  DEBUG_DUMP=1 but no debug hook mounted in container; skipping SIGUSR2"
      fi
    fi
    for pid in $pids; do
      wlog "  py-spy dump container pid $pid"
      "${PODMAN[@]}" exec "$C" timeout 120 /tmp/py-spy dump --pid "$pid" > "$dir/py-spy-$pid.txt" 2>&1
      "${PODMAN[@]}" exec "$C" timeout 180 /tmp/py-spy dump --pid "$pid" --locals > "$dir/py-spy-$pid-locals.txt" 2>&1
      "${PODMAN[@]}" exec "$C" sh -c "for t in /proc/$pid/task/*; do echo \"\${t##*/} \$(cat \$t/comm) \$(cut -d' ' -f3 \$t/stat) \$(cat \$t/wchan 2>/dev/null)\"; done" \
        > "$dir/threads-$pid.txt" 2>&1
    done
  fi
  curl -s --max-time 60 http://127.0.0.1:8000/metrics > "$dir/metrics.txt" \
    || wlog "  /metrics did not answer within 60 s"
  {
    echo "arm ${arm} (${CONCURRENT_STREAMS} streams) WEDGED at $(date '+%F %T %Z'): 0 running, waiting>0, no completed"
    echo "request for ${STALL_SECONDS}s+. Captures in $(basename "$dir")/. Results from this arm are not valid."
  } > "${sub}/WEDGED"
}

stop_client(){ pkill -TERM -f "$CLIENT_RE" 2>/dev/null; }

wlog "live-watch: attach matrix pid ${ATTACH_PID}, cap ${CONCURRENT_STREAMS}, stall=${STALL_SECONDS}s -> ${RESULTS_DIR}"
[[ -x "$PY_SPY" ]] || { wlog "ERROR: py-spy not found (PY_SPY=$PY_SPY)"; exit 1; }

# One iteration per client (= per arm) until the matrix exits.
while kill -0 "$ATTACH_PID" 2>/dev/null; do
  # client not up yet (vLLM model load takes a few minutes) — wait for it
  until pgrep -f "$CLIENT_RE" >/dev/null || ! kill -0 "$ATTACH_PID" 2>/dev/null; do sleep 5; done
  kill -0 "$ATTACH_PID" 2>/dev/null || break
  arm=$(current_arm)
  wlog "client running (${arm}); watching for a stall"

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
        wlog "STALL (${arm}): running=$running waiting=$waiting completed=$done_n unchanged for ${STALL_SECONDS}s"
        capture "$arm"; captured=1; stop_client; break
      fi
    else
      stalled_since=""
    fi
    last_done=$done_n
    sleep "$POLL_SECONDS"
  done
  (( captured )) || wlog "no stall detected while the ${arm} client ran"
  # wait for this client to exit before looking for the next arm's client
  while pgrep -f "$CLIENT_RE" >/dev/null && kill -0 "$ATTACH_PID" 2>/dev/null; do sleep 5; done
done
wlog "done -> ${RESULTS_DIR} (WEDGE-* captures in <arm>/ for any arm that stalled)"
