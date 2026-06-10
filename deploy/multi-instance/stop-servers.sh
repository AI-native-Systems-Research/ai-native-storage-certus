#!/bin/bash
#
# stop-servers.sh - Gracefully stop a multi-instance certus-server deployment.
#
# Sends SIGTERM to the certus-server processes (so they flush and shut down
# cleanly), waits, escalates to SIGKILL if needed, then kills the tmux session.
#
# Usage:
#   ./stop-servers.sh [-s SESSION] [--keep-logs]
#
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/config.sh"

KEEP_LOGS=1
while [[ $# -gt 0 ]]; do
    case "$1" in
        -s) SESSION="$2"; shift 2 ;;
        --keep-logs) KEEP_LOGS=1; shift ;;
        --purge-logs) KEEP_LOGS=0; shift ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

# --- Graceful termination ----------------------------------------------------
# Identify servers by executable name (see server_pids in config.sh) so we never
# accidentally signal the calling shell or an unrelated process.
pids="$(server_pids || true)"
if [[ -n "$pids" ]]; then
    log "Sending SIGTERM to certus-server process(es): $(echo "$pids" | tr '\n' ' ')"
    kill -TERM $pids 2>/dev/null || true
    for _ in $(seq 1 20); do
        [[ -z "$(server_pids || true)" ]] && break
        sleep 0.5
    done
    pids="$(server_pids || true)"
    if [[ -n "$pids" ]]; then
        warn "some servers still alive; sending SIGKILL"
        kill -KILL $pids 2>/dev/null || true
    fi
else
    log "No running certus-server processes found."
fi

# --- Tear down tmux session --------------------------------------------------
if tmux has-session -t "$SESSION" 2>/dev/null; then
    log "Killing tmux session '$SESSION'"
    tmux kill-session -t "$SESSION" || true
else
    log "No tmux session '$SESSION'."
fi

# --- Optional log cleanup ----------------------------------------------------
if [[ "$KEEP_LOGS" == 0 && -d "$RUN_DIR" ]]; then
    log "Removing run directory $RUN_DIR"
    rm -rf "$RUN_DIR"
else
    log "Logs retained in $RUN_DIR (use --purge-logs to delete)"
fi

log "Done."
