#!/bin/bash
# evaluator-guidellm-multimodal-x2-A100.sh — end-to-end evaluator for the
# synthetic MULTIMODAL (text+image) KV-offload test on 2x A100, extracting the
# two headline numbers: aggregate throughput (tokens/sec) and TTFT.
#
# This is the multimodal sibling of evaluator-guidellm-cc-traces-x2-A100.sh. It
# stands up a vision-language server (Qwen3-VL-32B-FP8, tensor-parallel across
# BOTH A100s), drives it with a synthetic text+image chat workload via guidellm,
# extracts throughput + TTFT from guidellm's JSON, and ALWAYS tears the stack
# down (containers + certus-server) so the GPUs and NVMe devices are released.
#
# Two offload backends are supported, selected with BACKEND=:
#
#   BACKEND=certus  (default) — Certus-SHMQ offload connector.
#     Full pipeline:
#       1. BUILD    CERTUS_PROFILE=full-optimized cargo build -r \
#                     -p certus-server-yaml --features spdk
#       2. SERVER   numactl --cpunodebind=0 --membind=0 certus-server-yaml \
#                     --device-pci ... x4 --shm-path /dev/shm/certus-shmq \
#                     --memory-tier-size 30G ... --format   (backgrounded)
#       3. VLLM     apps/vllm-serve/run-serve-certus-shmq-multimodal-2gpu.sh
#       4. CLIENT   apps/vllm-serve/run-guidellm-synthetic-multimodal.sh
#     (shmq image lives in the /mnt/certus1 podman store; an external
#      certus-server publishes the mailbox at SHM_PATH.)
#
#   BACKEND=cputier — vLLM-native CPU+fs tiering (OffloadingConnector). Skips
#     steps 1-2 entirely: cputier is self-contained (no certus-server, no
#     mailbox, default podman store, image certus-offload-bench-fix026).
#       3. VLLM     apps/vllm-serve/run-serve-cputier-multimodal-2gpu.sh
#       4. CLIENT   apps/vllm-serve/run-guidellm-synthetic-multimodal.sh
#
# Run the two back to back (BACKEND=certus then BACKEND=cputier) to compare the
# offload backends over an identical synthetic multimodal workload.
#
# After the client run it extracts, from guidellm's JSON (the concurrent profile
# emits a single benchmark):
#   * Total tokens/sec (aggregate: total_token_count.successful.total_sum / duration)
#   * output tokens/sec (output_token_count.successful.total_sum / duration)
#   * TTFT median AND mean (time_to_first_token_ms.successful.{median,mean}, ms + s)
#
# Prerequisites (same as running the steps by hand — this script does NOT do them):
#   * The multimodal serve image(s) built: certus-otel-shmq-bench in the
#     /mnt/certus1 store (certus), and/or certus-offload-bench-fix026 in the
#     default store (cputier).
#   * BACKEND=certus only: NVMe devices bound to vfio-pci with 1G hugepages
#     reserved (tools/configure-bench.sh) so certus-server can claim them.
#   * guidellm with the vision extra on the host (pip install 'guidellm[vision]').
#     WITHOUT Pillow the guidellm scheduler DEADLOCKS at worker startup — the
#     client script fails fast with an actionable message, which this evaluator
#     surfaces. Auto-detected on PATH; falls back to $GUIDELLM_VENV.
#   * Both A100s free (the model is served tensor-parallel across both).
#
# Everything below is env-overridable; defaults reproduce the standard run.
#
#   ./evaluator-guidellm-multimodal-x2-A100.sh                     # certus, 32 streams, 1080p, 600s
#   BACKEND=cputier ./evaluator-guidellm-multimodal-x2-A100.sh     # native CPU+fs tiering backend
#   MAX_SECONDS=120 ./evaluator-guidellm-multimodal-x2-A100.sh     # shorter measurement window
#   CONCURRENT_STREAMS=8 RESOLUTION=720p ./evaluator-guidellm-multimodal-x2-A100.sh
#   IMAGES_PER_REQUEST=2 MM_IMAGES=2 ./evaluator-guidellm-multimodal-x2-A100.sh   # 2 images/prompt
#   SKIP_BUILD=1 ./evaluator-guidellm-multimodal-x2-A100.sh        # (certus) reuse existing binary
set -uo pipefail

# ── Backend selection ────────────────────────────────────────────────────────
BACKEND="${BACKEND:-certus}"
case "$BACKEND" in
  certus|cputier) ;;
  *) echo "error: BACKEND must be 'certus' or 'cputier' (got '${BACKEND}')" >&2; exit 2 ;;
esac

# ── Paths ────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
VLLM_DIR="${VLLM_DIR:-${REPO_ROOT}/apps/vllm-serve}"
CLIENT_SCRIPT="${CLIENT_SCRIPT:-${VLLM_DIR}/run-guidellm-synthetic-multimodal.sh}"
if [[ "$BACKEND" == "certus" ]]; then
  SERVE_SCRIPT="${SERVE_SCRIPT:-${VLLM_DIR}/run-serve-certus-shmq-multimodal-2gpu.sh}"
else
  SERVE_SCRIPT="${SERVE_SCRIPT:-${VLLM_DIR}/run-serve-cputier-multimodal-2gpu.sh}"
fi

# ── certus-server config (BACKEND=certus only; matches the cc-traces evaluator) ─
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
SKIP_BUILD="${SKIP_BUILD:-0}"

# ── Multimodal workload knobs (passed through to the serve + client wrappers) ──
# These are consumed by run-serve-*-multimodal-2gpu.sh and
# run-guidellm-synthetic-multimodal.sh via ${VAR:-default}, so exporting them
# here overrides their defaults.
RATE_TYPE="${RATE_TYPE:-concurrent}"           # closed-loop by default
CONCURRENT_STREAMS="${CONCURRENT_STREAMS:-32}" # fixed in-flight streams
RESOLUTION="${RESOLUTION:-1080p}"              # synthetic image size
IMAGES_PER_REQUEST="${IMAGES_PER_REQUEST:-1}"  # images per chat request (client)
MM_IMAGES="${MM_IMAGES:-2}"                    # server --limit-mm-per-prompt cap
MAX_SECONDS="${MAX_SECONDS:-600}"

# ── Endpoint ─────────────────────────────────────────────────────────────────
PORT="${PORT:-8000}"
TARGET="${TARGET:-http://127.0.0.1:${PORT}}"

# ── Readiness timeouts ───────────────────────────────────────────────────────
SERVER_READY_TIMEOUT="${SERVER_READY_TIMEOUT:-120}"   # seconds to wait for the shmq mailbox
VLLM_READY_TIMEOUT="${VLLM_READY_TIMEOUT:-1800}"      # 32B-FP8 TP=2 load + graph capture can take a while

# guidellm venv fallback (the client needs guidellm[vision] on PATH).
GUIDELLM_VENV="${GUIDELLM_VENV:-/home/dwaddington/venv_guidellm}"

# ── Output locations ─────────────────────────────────────────────────────────
STAMP="$(date +%Y%m%d_%H%M%S)"
RESULTS_DIR="${RESULTS_DIR:-${SCRIPT_DIR}/results}"
mkdir -p "$RESULTS_DIR"
SERVER_LOG="${RESULTS_DIR}/certus-server_mm_${BACKEND}_${STAMP}.log"
VLLM_LOG="${RESULTS_DIR}/vllm-serve_mm_${BACKEND}_${STAMP}.log"
CLIENT_LOG="${RESULTS_DIR}/guidellm-client_mm_${BACKEND}_${STAMP}.log"
GUIDELLM_JSON="${RESULTS_DIR}/guidellm_multimodal_${BACKEND}_${STAMP}.json"   # OUTPUT for the client
SUMMARY="${RESULTS_DIR}/summary_mm_${BACKEND}_${STAMP}.txt"

log()  { echo -e "\033[1;36m[eval]\033[0m $*"; }
warn() { echo -e "\033[1;33m[eval:warn]\033[0m $*" >&2; }
err()  { echo -e "\033[1;31m[eval:error]\033[0m $*" >&2; }

# ── Backend-specific container store + image (for teardown reaping) ────────────
if [[ "$BACKEND" == "certus" ]]; then
  # The shmq serve script runs its container out of the alternate /mnt/certus1 store.
  PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
  PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"
  VLLM_IMAGE="${IMAGE:-localhost/certus-otel-shmq-bench}"
  STORE_FLAGS=(--root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT")
else
  # cputier uses the DEFAULT podman store and the fix026 image.
  VLLM_IMAGE="${IMAGE:-certus-offload-bench-fix026}"
  STORE_FLAGS=()
fi

# ── Teardown (runs on any exit) ────────────────────────────────────────────────
SERVER_PID=""
VLLM_PID=""
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
  ids="$(command podman "${STORE_FLAGS[@]}" \
          ps -a --format '{{.ID}} {{.Image}}' 2>/dev/null \
          | grep -F "$VLLM_IMAGE" | awk '{print $1}')"
  if [[ -n "$ids" ]]; then
    warn "stopping vLLM container(s): $(echo "$ids" | tr '\n' ' ')"
    echo "$ids" | xargs -r command podman "${STORE_FLAGS[@]}" rm -f >/dev/null 2>&1 || true
  fi
  [[ -n "$VLLM_PID" ]] && wait "$VLLM_PID" 2>/dev/null || true

  # 2. (certus only) Stop the certus-server and wait for it to release the NVMe.
  if [[ "$BACKEND" == "certus" ]]; then
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
  fi

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
# Multimodal needs guidellm's vision extra (Pillow) or the scheduler deadlocks at
# worker startup. Check with the interpreter next to guidellm (its venv).
GUIDELLM_PY="$(dirname "$(command -v guidellm)")/python3"
[[ -x "$GUIDELLM_PY" ]] || GUIDELLM_PY="python3"
if ! "$GUIDELLM_PY" -c 'import PIL' >/dev/null 2>&1; then
  err "guidellm[vision] (Pillow) not installed in the guidellm env — multimodal runs DEADLOCK without it"
  err "  -> pip install 'guidellm[vision]'   (in $(dirname "$(command -v guidellm)"))"
  exit 1
fi

log "backend=${BACKEND}  serve=${SERVE_SCRIPT##*/}  client=${CLIENT_SCRIPT##*/}"
log "workload: rate-type=${RATE_TYPE} streams=${CONCURRENT_STREAMS} resolution=${RESOLUTION} images/req=${IMAGES_PER_REQUEST} max-seconds=${MAX_SECONDS}"

# ── Step 1+2 (certus only): build + start certus-server, wait for the mailbox ──
if [[ "$BACKEND" == "certus" ]]; then
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
else
  log "BACKEND=cputier — self-contained (no certus-server, no build)"
fi

# ── Step 3: start the multimodal vLLM serve, wait for /v1/models ────────────────
log "starting multimodal vLLM serve -> ${VLLM_LOG}"
( cd "$VLLM_DIR" && \
  PORT="$PORT" MM_IMAGES="$MM_IMAGES" \
  exec "$SERVE_SCRIPT" ) >"$VLLM_LOG" 2>&1 &
VLLM_PID=$!

log "waiting up to ${VLLM_READY_TIMEOUT}s for vLLM at ${TARGET}/v1/models (32B-FP8 TP=2 load can take minutes) ..."
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

# ── Step 4: run the multimodal guidellm client ──────────────────────────────────
log "running multimodal guidellm client -> ${CLIENT_LOG}"
client_rc=0
( cd "$VLLM_DIR" && \
  PORT="$PORT" \
  RATE_TYPE="$RATE_TYPE" \
  CONCURRENT_STREAMS="$CONCURRENT_STREAMS" \
  RESOLUTION="$RESOLUTION" \
  IMAGES_PER_REQUEST="$IMAGES_PER_REQUEST" \
  MAX_SECONDS="$MAX_SECONDS" \
  OUTPUT="$GUIDELLM_JSON" \
  exec "$CLIENT_SCRIPT" ) 2>&1 | tee "$CLIENT_LOG"
client_rc="${PIPESTATUS[0]}"
[[ "$client_rc" -eq 0 ]] || warn "guidellm client exited with code ${client_rc} (extracting whatever it wrote)"

# ── Extract headline metrics (throughput + TTFT) ────────────────────────────────
log "extracting metrics from ${GUIDELLM_JSON}"
if [[ -f "$GUIDELLM_JSON" ]]; then
  BACKEND="$BACKEND" RESOLUTION="$RESOLUTION" CONCURRENT_STREAMS="$CONCURRENT_STREAMS" \
  IMAGES_PER_REQUEST="$IMAGES_PER_REQUEST" \
  python3 - "$GUIDELLM_JSON" <<'PY' | tee "$SUMMARY"
import json, os, sys
d = json.load(open(sys.argv[1]))
bms = d.get("benchmarks", [])
if not bms:
    print("no benchmarks in JSON"); sys.exit(0)
bm = bms[-1]                       # the concurrent profile emits a single benchmark
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
# TTFT (ms) — report both median and mean.
ttft_med_ms = stat("time_to_first_token_ms").get("median", float("nan"))
ttft_mean_ms = stat("time_to_first_token_ms").get("mean", float("nan"))

be = os.environ.get("BACKEND", "?")
print(f"========== multimodal guidellm evaluation ({be}) ==========")
print(f"workload: resolution={os.environ.get('RESOLUTION')}  "
      f"streams={os.environ.get('CONCURRENT_STREAMS')}  "
      f"images/req={os.environ.get('IMAGES_PER_REQUEST')}")
print(f"requests: total={rt.get('total')}  successful={succ}  "
      f"incomplete={rt.get('incomplete')}  errored={rt.get('errored')}")
print(f"duration: {dur:.1f} s")
print("-----------------------------------------------------------")
print(f"Total tokens/sec : {total_tps_agg:,.2f} tok/s"
      f"   (total tokens {tot_sum:,.0f} / {dur:.0f}s)")
print(f"  output tok/s   : {out_tps_agg:,.2f} tok/s")
print(f"  guidellm mean  : {gtps:,.2f} tok/s   (tokens_per_second.successful.mean)")
print(f"TTFT median      : {ttft_med_ms:,.2f} ms   ({ttft_med_ms/1000.0:,.3f} s)")
print(f"TTFT mean        : {ttft_mean_ms:,.2f} ms   ({ttft_mean_ms/1000.0:,.3f} s)")
print("===========================================================")
PY
else
  warn "no guidellm JSON produced at ${GUIDELLM_JSON} — client likely failed; see ${CLIENT_LOG}"
fi

log "done. results in ${RESULTS_DIR}"
log "  summary : ${SUMMARY}"
log "  json    : ${GUIDELLM_JSON}"
exit "$client_rc"
