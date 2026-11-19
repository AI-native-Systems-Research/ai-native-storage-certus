#!/bin/bash
# Standalone stop-rule watcher for the low-concurrency think sweep. The in-driver
# watcher raced the preflight (matrix not up yet at t=0 -> "matrix gone" exit), so
# this restart adds a wait-for-matrix phase. Args: <cap> <logfile> [matrix-regex]
cap="${1:?cap}"; log="${2:?logfile}"
MATRIX_RE="${3:-^/bin/bash /home/dwaddington/ai-native-storage-certus/benchmarks/vllm-serve/run-gqa-iobound-matrix\.sh}"
# NB: aiperf's main process starts as "aiperf profile --url ..." and retitles itself
# to "aiperf system_controller" once running — match both.
AIPERF_RE='aiperf (profile --url|system_controller)'
exec >> "$log" 2>&1
echo "[$(date '+%T')] stop-rule v2: cap=${cap} (waiting for matrix/client to appear)"
# Wait up to 45 min for the matrix harness (preflight/probe/build precede it).
seen=""
for _ in $(seq 1 180); do
  seen=$(pgrep -f "$MATRIX_RE" | head -1)
  [[ -n "$seen" ]] && break
  sleep 15
done
[[ -z "$seen" ]] && { echo "[$(date '+%T')] stop-rule v2: no matrix appeared in 45 min; exiting"; exit 1; }
echo "[$(date '+%T')] stop-rule v2: matrix up (pid ${seen}); monitoring live < ${cap} for 600s after 900s ramp"
pid="" prev="" below="" start=0
while pgrep -f "$MATRIX_RE" >/dev/null; do
  pid=$(pgrep -f "$AIPERF_RE" | head -1)
  if [[ -z "$pid" ]]; then sleep 15; prev=""; below=""; continue; fi
  if [[ "$pid" != "$prev" ]]; then
    prev=$pid; start=$(date +%s); below=""
    echo "[$(date '+%T')] new client pid ${pid} (cap=${cap})"
  fi
  now=$(date +%s)
  # NB: metric lines carry labels ("vllm:num_requests_running{engine=...} 1.0"), so $1
  # is name+labels; match the name followed by '{' to exclude waiting_by_reason lines.
  live=$(curl -fsS --max-time 5 http://127.0.0.1:8000/metrics 2>/dev/null \
    | awk '$1 ~ /^vllm:num_requests_running\{/{r=$2} $1 ~ /^vllm:num_requests_waiting\{/{w=$2} END{if(r!=""||w!="")printf "%d",r+w}')
  if [[ -n "$live" ]]; then
    echo "[$(date '+%T')] elapsed=$((now-start))s live=${live} cap=${cap}"
    if (( now - start > 900 && live < cap )); then
      if [[ -n "$below" ]]; then
        if (( now - below >= 600 )); then
          echo "[$(date '+%T')] STOP-RULE: live=${live} < cap=${cap} for >=600s -> interrupting client ${pid}"
          kill -INT "$pid"; sleep 90
          pgrep -f "$AIPERF_RE" >/dev/null && pkill -KILL -f "$AIPERF_RE"
          prev=""; below=""
        fi
      else below=$now; fi
    else below=""; fi
  fi
  sleep 15
done
echo "[$(date '+%T')] matrix gone; stop-rule v2 exit"
