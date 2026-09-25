#!/bin/bash
# run-guidellm-cc-traces.sh — end-to-end evaluator for the 131K cc-traces
# KV-offload throughput test against the Certus-SHMQ offload backend.
#
# It orchestrates the full four-step pipeline and always tears everything down:
#
#   1. BUILD    CERTUS_PROFILE=full-optimized cargo b -r -p certus-server-yaml --features spdk
#   2. SERVER   numactl --cpunodebind=0 --membind=0 target/release/certus-server-yaml \
#                 --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 \
#                 --shm-path /dev/shm/certus-shmq --memory-tier-size 30G \
#                 --memory-tier-eviction-threshold 0.9 --store-backpressure-ms 5000 \
#                 --channels 64 --poller-base-cpu 2 --shmq-poller-cpu 6 --format
#               (backgrounded; we wait for the shmq mailbox to appear)
#   3. VLLM     apps/vllm-serve/run-serve-cc131k.sh
#               (backgrounded; we wait for the OpenAI /v1/models endpoint)
#   4. CLIENT   MAX_SECONDS=600 apps/vllm-serve/run-guidellm-cc131k.sh
#
# After the client run it extracts the two headline numbers from guidellm's JSON:
#   * Total tokens/sec (aggregate: total_token_count.successful.total_sum / duration)
#   * TTFT median (time_to_first_token_ms.successful.median, in ms and s)
#
# On ANY exit (success, failure, or Ctrl-C) it kills the vLLM container and the
# certus-server so the NVMe devices and GPUs are released.
#
# Prerequisites (same as running the steps by hand — this script does NOT do them):
#   * NVMe devices already bound to vfio-pci with 1G hugepages reserved
#     (tools/configure-bench.sh) so certus-server can claim them.
#   * The shmq vLLM image built in the /mnt/certus1 podman store.
#   * guidellm installed (auto-detected; falls back to $GUIDELLM_VENV).
#
# Everything below is env-overridable; defaults reproduce the requested run.
#
#   ./run-guidellm-cc-traces.sh                    # full run, 600s client window
#   MAX_SECONDS=120 ./run-guidellm-cc-traces.sh    # shorter measurement window
#   SKIP_BUILD=1 ./run-guidellm-cc-traces.sh       # reuse the existing binary
set -uo pipefail

# ── Paths ────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
VLLM_DIR="${VLLM_DIR:-${REPO_ROOT}/apps/vllm-serve}"
SERVE_SCRIPT="${SERVE_SCRIPT:-${VLLM_DIR}/run-serve-cc131k.sh}"
CLIENT_SCRIPT="${CLIENT_SCRIPT:-${VLLM_DIR}/run-guidellm-cc131k.sh}"

# ── Run configuration (matches the requested invocation) ───────────────────────
CERTUS_PROFILE="${CERTUS_PROFILE:-full-optimized}"
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
DEVICE_PCI_1="${DEVICE_PCI_1:-0000:61:00.0}"
DEVICE_PCI_2="${DEVICE_PCI_2:-0000:62:00.0}"
DEVICE_PCI_3="${DEVICE_PCI_3:-0000:63:00.0}"
DEVICE_PCI_4="${DEVICE_PCI_4:-0000:64:00.0}"
MEM_TIER_SIZE="${MEM_TIER_SIZE:-30G}"
EVICT_THRESHOLD="${EVICT_THRESHOLD:-0.9}"
STORE_BACKPRESSURE_MS="${STORE_BACKPRESSURE_MS:-5000}"
CHANNELS="${CHANNELS:-64}"
POLLER_BASE_CPU="${POLLER_BASE_CPU:-2}"
SHMQ_POLLER_CPU="${SHMQ_POLLER_CPU:-6}"

MAX_SECONDS="${MAX_SECONDS:-600}"
PORT="${PORT:-8000}"
TARGET="${TARGET:-http://127.0.0.1:${PORT}}"

SKIP_BUILD="${SKIP_BUILD:-0}"
SERVER_READY_TIMEOUT="${SERVER_READY_TIMEOUT:-120}"   # seconds to wait for the shmq mailbox
VLLM_READY_TIMEOUT="${VLLM_READY_TIMEOUT:-1200}"      # seconds to wait for vLLM /v1/models (model load + graph capture)

# guidellm venv fallback (the client script needs guidellm on PATH).
GUIDELLM_VENV="${GUIDELLM_VENV:-/home/dwaddington/venv_guidellm}"

# ── Output locations ───────────────────────────────────────────────────────────
STAMP="$(date +%Y%m%d_%H%M%S)"
RESULTS_DIR="${RESULTS_DIR:-${SCRIPT_DIR}/results}"
mkdir -p "$RESULTS_DIR"
SERVER_LOG="${RESULTS_DIR}/certus-server_${STAMP}.log"
VLLM_LOG="${RESULTS_DIR}/vllm-serve_${STAMP}.log"
CLIENT_LOG="${RESULTS_DIR}/guidellm-client_${STAMP}.log"
GUIDELLM_JSON="${RESULTS_DIR}/guidellm_cc-traces_${STAMP}.json"   # OUTPUT for run-guidellm.sh
SUMMARY="${RESULTS_DIR}/summary_${STAMP}.txt"

log()  { echo -e "\033[1;36m[eval]\033[0m $*"; }
warn() { echo -e "\033[1;33m[eval:warn]\033[0m $*" >&2; }
err()  { echo -e "\033[1;31m[eval:error]\033[0m $*" >&2; }

# ── Teardown (runs on any exit) ────────────────────────────────────────────────
SERVER_PID=""
VLLM_PID=""
# The shmq serve script runs its container out of the alternate /mnt/certus1 store.
PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"
SHMQ_IMAGE="${IMAGE:-localhost/certus-otel-shmq-bench}"

cleanup() {
  local rc=$?
  echo
  log "tearing down (exit code ${rc}) ..."

  # 1. Stop the vLLM container. Killing the backgrounded podman client usually
  #    triggers --rm, but reap by image explicitly so nothing is left holding the
  #    GPUs if the client detached.
  if [[ -n "$VLLM_PID" ]] && kill -0 "$VLLM_PID" 2>/dev/null; then
    kill -TERM "$VLLM_PID" 2>/dev/null || true
  fi
  local ids
  ids="$(command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" \
          ps -a --format '{{.ID}} {{.Image}}' 2>/dev/null \
          | grep -F "$SHMQ_IMAGE" | awk '{print $1}')"
  if [[ -n "$ids" ]]; then
    warn "stopping vLLM container(s): $(echo "$ids" | tr '\n' ' ')"
    echo "$ids" | xargs -r command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" rm -f >/dev/null 2>&1 || true
  fi
  [[ -n "$VLLM_PID" ]] && wait "$VLLM_PID" 2>/dev/null || true

  # 2. Stop the certus-server and wait for it to release the NVMe devices.
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

  # 3. Remove our shmq mailbox so a re-run starts clean.
  [[ -e "$SHM_PATH" ]] && rm -f "$SHM_PATH" 2>/dev/null || true

  log "teardown complete. logs in ${RESULTS_DIR}"
}
trap cleanup EXIT INT TERM

# ── Preflight ──────────────────────────────────────────────────────────────────
[[ -x "$SERVE_SCRIPT"  ]] || { err "serve script not found/executable: $SERVE_SCRIPT";  exit 1; }
[[ -x "$CLIENT_SCRIPT" ]] || { err "client script not found/executable: $CLIENT_SCRIPT"; exit 1; }
if ! command -v guidellm >/dev/null 2>&1; then
  if [[ -x "${GUIDELLM_VENV}/bin/guidellm" ]]; then
    export PATH="${GUIDELLM_VENV}/bin:${PATH}"
    log "using guidellm from ${GUIDELLM_VENV}/bin"
  else
    err "guidellm not on PATH and not found at ${GUIDELLM_VENV}/bin — install it or set GUIDELLM_VENV"
    exit 1
  fi
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
log "starting certus-server -> ${SERVER_LOG}"
numactl --cpunodebind=0 --membind=0 "$SERVER_BIN" \
	--device-pci "$DEVICE_PCI_1" --device-pci "$DEVICE_PCI_2" \
	--device-pci "$DEVICE_PCI_3" --device-pci "$DEVICE_PCI_4" \
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

# ── Step 3: start vLLM serve, wait for /v1/models ───────────────────────────────
log "starting vLLM serve -> ${VLLM_LOG}"
( cd "$VLLM_DIR" && PORT="$PORT" exec "$SERVE_SCRIPT" ) >"$VLLM_LOG" 2>&1 &
VLLM_PID=$!

log "waiting up to ${VLLM_READY_TIMEOUT}s for vLLM at ${TARGET}/v1/models (model load can take minutes) ..."
ready=0
for _ in $(seq 1 "$VLLM_READY_TIMEOUT"); do
  if ! kill -0 "$VLLM_PID" 2>/dev/null; then
    err "vLLM serve exited during startup — see ${VLLM_LOG}"; tail -n 60 "$VLLM_LOG" >&2 || true; exit 1
  fi
  if curl -fsS --max-time 5 "${TARGET}/v1/models" >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[[ "$ready" == "1" ]] || { err "vLLM never became ready — see ${VLLM_LOG}"; tail -n 60 "$VLLM_LOG" >&2 || true; exit 1; }
log "vLLM serving on ${TARGET}"

# ── Step 4: run the guidellm client ─────────────────────────────────────────────
log "running guidellm cc-traces client (MAX_SECONDS=${MAX_SECONDS}) -> ${CLIENT_LOG}"
client_rc=0
( cd "$VLLM_DIR" && MAX_SECONDS="$MAX_SECONDS" PORT="$PORT" OUTPUT="$GUIDELLM_JSON" exec "$CLIENT_SCRIPT" ) \
  2>&1 | tee "$CLIENT_LOG"
client_rc="${PIPESTATUS[0]}"
[[ "$client_rc" -eq 0 ]] || warn "guidellm client exited with code ${client_rc} (extracting whatever it wrote)"

# ── Extract headline metrics ────────────────────────────────────────────────────
log "extracting metrics from ${GUIDELLM_JSON}"
if [[ -f "$GUIDELLM_JSON" ]]; then
  python3 - "$GUIDELLM_JSON" <<'PY' | tee "$SUMMARY"
import json, sys
d = json.load(open(sys.argv[1]))
bms = d.get("benchmarks", [])
if not bms:
    print("no benchmarks in JSON"); sys.exit(0)
bm = bms[-1]                       # the throughput profile emits a single benchmark
m = bm["metrics"]
dur = bm.get("duration") or 0.0
rt = m["request_totals"]

def stat(name, cls="successful"):
    return m[name][cls]

succ = rt.get("successful", 0)
# Total tokens/sec: aggregate over the measurement window (prompt + output tokens).
tot_sum = stat("total_token_count")["total_sum"] if succ else 0.0
total_tps_agg = tot_sum / dur if dur else float("nan")
# guidellm's own aggregate throughput mean, for cross-check.
gtps = stat("tokens_per_second").get("mean", float("nan"))
out_sum = stat("output_token_count")["total_sum"] if succ else 0.0
out_tps_agg = out_sum / dur if dur else float("nan")
# TTFT median (ms).
ttft_med_ms = stat("time_to_first_token_ms").get("median", float("nan"))

print("========== cc-traces guidellm evaluation ==========")
print(f"requests: total={rt.get('total')}  successful={succ}  "
      f"incomplete={rt.get('incomplete')}  errored={rt.get('errored')}")
print(f"duration: {dur:.1f} s")
print("---------------------------------------------------")
print(f"Total tokens/sec : {total_tps_agg:,.2f} tok/s"
      f"   (total tokens {tot_sum:,.0f} / {dur:.0f}s)")
print(f"  output tok/s   : {out_tps_agg:,.2f} tok/s")
print(f"  guidellm mean  : {gtps:,.2f} tok/s   (tokens_per_second.successful.mean)")
print(f"TTFT median      : {ttft_med_ms:,.2f} ms   ({ttft_med_ms/1000.0:,.2f} s)")
print("===================================================")
PY
else
  warn "no guidellm JSON produced at ${GUIDELLM_JSON} — client likely failed; see ${CLIENT_LOG}"
fi

log "done. results in ${RESULTS_DIR}"
log "  summary : ${SUMMARY}"
log "  json    : ${GUIDELLM_JSON}"
exit "$client_rc"
