#!/bin/bash
# evaluator-connector-lifecycle-64sess.sh — end-to-end evaluator for the Python
# connector-lifecycle benchmark against a Certus-SHMQ server built with the
# `full-optimized` profile.
#
# It runs the exact client invocation:
#
#     python tools/certus-connector-bench/bench_connector_lifecycle.py \
#         --num-sessions 64 --min-duration 3
#
# orchestrating a three-step pipeline and always tearing everything down:
#
#   1. BUILD    CERTUS_PROFILE=full-optimized cargo build -r \
#                 -p certus-server-yaml --features spdk
#   2. SERVER   numactl --cpunodebind=0 --membind=0 target/release/certus-server-yaml \
#                 --device-pci <4 NVMe> --shm-path /dev/shm/certus-shmq \
#                 --memory-tier-size 32G --memory-tier-eviction-threshold 0.9 \
#                 --store-backpressure-ms 5000 --channels 32 \
#                 --poller-base-cpu 2 --shmq-poller-cpu 6 --format
#               (backgrounded; we wait for the shmq mailbox to appear)
#   3. CLIENT   python3 tools/certus-connector-bench/bench_connector_lifecycle.py \
#                 --num-sessions 64 --min-duration 3 \
#                 --shm-path /dev/shm/certus-shmq --csv <results>.csv --tag full-optimized
#
# The connector bench talks to the server over the shmq mailbox exactly as the
# vLLM shmq connector does (BLAKE2b key hashing, per-block regions,
# ThreadPoolExecutor dispatch, lookup cache). It needs a CUDA GPU (imports torch)
# and finds the `certus_shmq_connector` module via its own sys.path insert, so
# the system python3 (torch pre-installed) is sufficient.
#
# On ANY exit (success, failure, or Ctrl-C) it kills the certus-server and
# removes the mailbox so the NVMe devices are released and a re-run starts clean.
#
# Prerequisites (same as running the steps by hand — this script does NOT do them):
#   * NVMe devices already bound to vfio-pci with 1G hugepages reserved
#     (sudo tools/configure-bench.sh) so certus-server can claim them.
#   * A CUDA GPU visible to the python used (torch).
#
# Everything below is env-overridable; defaults reproduce the requested run.
#
#   ./evaluator-connector-lifecycle-64sess.sh                 # full run
#   SKIP_BUILD=1 ./evaluator-connector-lifecycle-64sess.sh    # reuse the binary
#   NUM_SESSIONS=32 MIN_DURATION=5 ./evaluator-connector-lifecycle-64sess.sh
set -uo pipefail

# ── Paths ────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
BENCH_SCRIPT="${BENCH_SCRIPT:-${REPO_ROOT}/tools/certus-connector-bench/bench_connector_lifecycle.py}"

# ── Run configuration ──────────────────────────────────────────────────────────
CERTUS_PROFILE="${CERTUS_PROFILE:-full-optimized}"
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
DEVICE_PCIS="${DEVICE_PCIS:-0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0}"
MEM_TIER_SIZE="${MEM_TIER_SIZE:-32G}"
EVICT_THRESHOLD="${EVICT_THRESHOLD:-0.9}"
STORE_BACKPRESSURE_MS="${STORE_BACKPRESSURE_MS:-5000}"
CHANNELS="${CHANNELS:-32}"
POLLER_BASE_CPU="${POLLER_BASE_CPU:-2}"
SHMQ_POLLER_CPU="${SHMQ_POLLER_CPU:-6}"
CPUNODE="${CPUNODE:-0}"

# Client invocation (the requested command).
NUM_SESSIONS="${NUM_SESSIONS:-64}"
MIN_DURATION="${MIN_DURATION:-3}"
BENCH_PYTHON="${BENCH_PYTHON:-python3}"

SKIP_BUILD="${SKIP_BUILD:-0}"
SERVER_READY_TIMEOUT="${SERVER_READY_TIMEOUT:-120}"   # seconds to wait for the shmq mailbox

# ── Output locations ───────────────────────────────────────────────────────────
STAMP="$(date +%Y%m%d_%H%M%S)"
RESULTS_DIR="${RESULTS_DIR:-${SCRIPT_DIR}/results}"
mkdir -p "$RESULTS_DIR"
SERVER_LOG="${RESULTS_DIR}/certus-server_connector-lifecycle_${STAMP}.log"
CLIENT_LOG="${RESULTS_DIR}/connector-lifecycle_${STAMP}.log"
BENCH_CSV="${BENCH_CSV:-${RESULTS_DIR}/connector-lifecycle_${STAMP}.csv}"

log()  { echo -e "\033[1;36m[eval]\033[0m $*"; }
warn() { echo -e "\033[1;33m[eval:warn]\033[0m $*" >&2; }
err()  { echo -e "\033[1;31m[eval:error]\033[0m $*" >&2; }

# ── Teardown (runs on any exit) ────────────────────────────────────────────────
SERVER_PID=""

cleanup() {
  local rc=$?
  echo
  log "tearing down (exit code ${rc}) ..."

  # Stop the certus-server and wait for it to release the NVMe devices.
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    warn "stopping certus-server (pid ${SERVER_PID})"
    kill -TERM "$SERVER_PID" 2>/dev/null || true
    for _ in $(seq 1 20); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
    kill -KILL "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  # Belt-and-suspenders: reap any stray server bound to our mailbox.
  pkill -TERM -f "certus-server-yaml.*${SHM_PATH}" 2>/dev/null || true
  sleep 1
  pkill -KILL -f "certus-server-yaml.*${SHM_PATH}" 2>/dev/null || true

  # Remove our shmq mailbox so a re-run starts clean.
  [[ -e "$SHM_PATH" ]] && rm -f "$SHM_PATH" 2>/dev/null || true

  log "teardown complete. logs in ${RESULTS_DIR}"
}
trap cleanup EXIT INT TERM

# ── Preflight ──────────────────────────────────────────────────────────────────
[[ -f "$BENCH_SCRIPT" ]] || { err "bench script not found: $BENCH_SCRIPT"; exit 1; }
if ! "$BENCH_PYTHON" -c "import torch" >/dev/null 2>&1; then
  err "'$BENCH_PYTHON' cannot import torch — the connector bench needs torch/CUDA."
  err "set BENCH_PYTHON to an interpreter that has it (e.g. BENCH_PYTHON=/path/to/venv/bin/python)."
  exit 1
fi

# ── Step 1: build certus-server-yaml ────────────────────────────────────────────
cd "$REPO_ROOT"
if [[ "$SKIP_BUILD" == "1" ]]; then
  log "SKIP_BUILD=1 — reusing existing target/release/certus-server-yaml"
else
  log "building certus-server-yaml (CERTUS_PROFILE=${CERTUS_PROFILE}, --features spdk)"
  if ! CERTUS_PROFILE="$CERTUS_PROFILE" cargo build -r -p certus-server-yaml --features spdk; then
    err "build failed"; exit 1
  fi
fi
SERVER_BIN="${REPO_ROOT}/target/release/certus-server-yaml"
[[ -x "$SERVER_BIN" ]] || { err "server binary missing after build: $SERVER_BIN"; exit 1; }

# ── Step 2: start certus-server, wait for the shmq mailbox ──────────────────────
[[ -e "$SHM_PATH" ]] && { warn "stale mailbox ${SHM_PATH} present — removing"; rm -f "$SHM_PATH" 2>/dev/null || true; }

# Expand the space-separated PCI list into repeated --device-pci flags.
DEV_ARGS=()
for pci in $DEVICE_PCIS; do DEV_ARGS+=(--device-pci "$pci"); done

log "starting certus-server -> ${SERVER_LOG}"
log "  devices: ${DEVICE_PCIS}"
log "  mem tier: ${MEM_TIER_SIZE} (evict ${EVICT_THRESHOLD}, channels ${CHANNELS})"
numactl --cpunodebind="${CPUNODE}" --membind="${CPUNODE}" "$SERVER_BIN" \
	"${DEV_ARGS[@]}" \
	--shm-path "$SHM_PATH" \
	--memory-tier-size "$MEM_TIER_SIZE" \
	--memory-tier-eviction-threshold "$EVICT_THRESHOLD" \
	--store-backpressure-ms "$STORE_BACKPRESSURE_MS" \
	--channels "$CHANNELS" \
	--poller-base-cpu "$POLLER_BASE_CPU" \
	--shmq-poller-cpu "$SHMQ_POLLER_CPU" \
	--format \
	>"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

log "waiting up to ${SERVER_READY_TIMEOUT}s for mailbox ${SHM_PATH} ..."
ready=0
for _ in $(seq 1 "$SERVER_READY_TIMEOUT"); do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    err "certus-server exited during startup — see ${SERVER_LOG}"; tail -n 40 "$SERVER_LOG" >&2 || true; exit 1
  fi
  [[ -e "$SHM_PATH" ]] && { ready=1; break; }
  sleep 1
done
[[ "$ready" == "1" ]] || { err "mailbox ${SHM_PATH} never appeared — see ${SERVER_LOG}"; tail -n 40 "$SERVER_LOG" >&2 || true; exit 1; }
log "certus-server up (pid ${SERVER_PID}); mailbox ready"

# ── Step 3: run the connector-lifecycle bench ───────────────────────────────────
log "running connector-lifecycle bench (--num-sessions ${NUM_SESSIONS} --min-duration ${MIN_DURATION}) -> ${CLIENT_LOG}"
client_rc=0
"$BENCH_PYTHON" "$BENCH_SCRIPT" \
    --num-sessions "$NUM_SESSIONS" \
    --min-duration "$MIN_DURATION" \
    --shm-path "$SHM_PATH" \
    --csv "$BENCH_CSV" \
    --tag "$CERTUS_PROFILE" \
  2>&1 | tee "$CLIENT_LOG"
client_rc="${PIPESTATUS[0]}"
[[ "$client_rc" -eq 0 ]] || warn "connector bench exited with code ${client_rc}"

log "done. results in ${RESULTS_DIR}"
log "  client log : ${CLIENT_LOG}"
log "  csv        : ${BENCH_CSV}"
log "  server log : ${SERVER_LOG}"
exit "$client_rc"
