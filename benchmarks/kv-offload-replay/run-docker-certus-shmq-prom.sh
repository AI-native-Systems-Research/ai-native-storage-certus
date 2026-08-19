#!/bin/bash
# Certus-SHMQ + Prometheus — same as run-docker-certus-shmq.sh (host
# certus-server over SPDK NVMe + /dev/shm mailbox client), but with the
# client's vLLM engine exposing Prometheus metrics on port 8000.
#
# The bench drives vLLM through the offline LLM(...) engine (no OpenAI server),
# so there is no /metrics endpoint unless the workload opens one itself. This
# variant does three things the base script does not:
#   * LOG_STATS=1        — so vLLM registers its PrometheusStatLogger metrics
#   * PROM_PORT=8000      — the workload calls start_http_server(8000)
#   * WORKLOAD_SRC=...    — bind-mounts the repo's run_multiturn_shmq_certus.py
#                           over the image copy, so the exporter works WITHOUT
#                           rebuilding the certus-shmq-bench image.
# Scrape the client from the host at:  http://localhost:8000/metrics
#
# NOTE: these are the CLIENT-side vLLM + KV-offload metrics. The SPDK/SSD-side
# counters live in the host certus-server and are NOT in this registry.
#
#   ./run-docker-certus-shmq-prom.sh
#   PROM_PORT=9100 DEVICE_PCI="0000:61:00.0" ./run-docker-certus-shmq-prom.sh
#
# PREREQUISITES (not performed here — needs sudo; see
# certus-shmq-connector/setup-host.sh or tools/configure-bench.sh):
#   * every device in DEVICE_PCI bound to vfio-pci (not the kernel nvme driver)
#   * 1G hugepages reserved on NUMA_NODE
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

# The shmq image lives in the /mnt/certus1 podman store, not the default store.
IMAGE="${IMAGE:-localhost/certus-shmq-bench}"
PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"

DEVICE_PCI="${DEVICE_PCI:-0000:61:00.0 0000:62:00.0 0000:63:00.0}"  # vfio-pci NVMe group
MEM_TIER_SIZE="${MEM_TIER_SIZE:-32G}"              # single spdk_zmalloc — keep <= ~32G
EVICT_THRESH="${EVICT_THRESH:-0.9}"
SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"
TENSOR_PARALLEL_SIZE="${TENSOR_PARALLEL_SIZE:-1}"
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"       # mailbox file (shared into container)
CHANNELS="${CHANNELS:-8}"                          # max in-flight requests = worker threads
POLLER_BASE_CPU="${POLLER_BASE_CPU:-2}"            # NVMe SPDK pollers -> base+N (empty to skip)
SHMQ_POLLER_CPU="${SHMQ_POLLER_CPU:-6}"            # shm-queue busy-poll core (empty to skip)
SERVER_WAIT="${SERVER_WAIT:-180}"                  # seconds to wait for "serving"
NUMA_NODE="${NUMA_NODE:-0}"                         # pin server to the NVMe/hugepage node
SERVER_BIN="${SERVER_BIN:-${REPO_ROOT}/target/release/certus-server}"
LOG="${LOG:-${SCRIPT_DIR}/certus-shmq-prom_$(stamp).log}"
SERVER_LOG="${SERVER_LOG:-${SCRIPT_DIR}/server-shmq_$(stamp).log}"

# ── Prometheus wiring (the only substantive difference from the base script) ──
PROM_PORT="${PROM_PORT:-8000}"        # host port the client exporter is published on
LOG_STATS="${LOG_STATS:-1}"           # 1 = register vLLM metrics (empty /metrics otherwise)
# Bind-mount the repo's workload over the image copy so the exporter block takes
# effect without a rebuild. Override/blank to use the image's baked workload
# instead (only correct once the image is rebuilt with the exporter code).
WORKLOAD_SRC="${WORKLOAD_SRC:-${REPO_ROOT}/certus-shmq-connector/run_multiturn_shmq_certus.py}"

[[ -x "$SERVER_BIN" ]] || {
  echo "error: server binary not built at ${SERVER_BIN}" >&2
  echo "       build it: cargo build --release -p certus-server" >&2
  exit 1
}
require_image "$IMAGE" --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT"

dev_flags=(); for d in $DEVICE_PCI; do dev_flags+=(--device-pci "$d"); done
numa_prefix=()
command -v numactl >/dev/null 2>&1 && numa_prefix=(numactl "--cpunodebind=${NUMA_NODE}" "--membind=${NUMA_NODE}")

# Optional explicit poller pinning (keeps the busy-poll shmq core off the NVMe
# poller cores). Only appended when set — leave empty to let SPDK choose.
poller_flags=()
[[ -n "$POLLER_BASE_CPU" ]] && poller_flags+=(--poller-base-cpu "$POLLER_BASE_CPU")
[[ -n "$SHMQ_POLLER_CPU" ]] && poller_flags+=(--shmq-poller-cpu "$SHMQ_POLLER_CPU")

SERVER_PID=""
stop_server() {
  [[ -z "$SERVER_PID" ]] && return 0
  kill -0 "$SERVER_PID" 2>/dev/null || { SERVER_PID=""; return 0; }
  echo "[certus-shmq] stopping server (pid ${SERVER_PID})"
  kill -TERM "$SERVER_PID" 2>/dev/null
  for _ in $(seq 1 8); do
    kill -0 "$SERVER_PID" 2>/dev/null || { SERVER_PID=""; return 0; }
    sleep 1
  done
  echo "[certus-shmq] server ignored SIGTERM (SPDK teardown) — escalating to SIGKILL"
  kill -9 "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null || true
  SERVER_PID=""
}
trap stop_server EXIT

# Start from a clean mailbox so a stale file from a crashed prior run can't fool
# the client preflight (the server truncates/recreates it anyway).
rm -f "$SHM_PATH"

echo "[certus-shmq] starting server: ${dev_flags[*]} --memory-tier-size ${MEM_TIER_SIZE} shm=${SHM_PATH} channels=${CHANNELS} (numa ${NUMA_NODE})"
"${numa_prefix[@]}" "$SERVER_BIN" "${dev_flags[@]}" \
  --memory-tier-size "$MEM_TIER_SIZE" \
  --memory-tier-eviction-threshold "$EVICT_THRESH" \
  --shm-path "$SHM_PATH" \
  --channels "$CHANNELS" \
  "${poller_flags[@]}" \
  --format \
  > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

# Readiness: no TCP port. The server builds the whole stack, then publishes the
# mailbox and logs "serving" LAST — so that line means the mailbox is live.
up=0
for _ in $(seq 1 "$SERVER_WAIT"); do
  kill -0 "$SERVER_PID" 2>/dev/null || break
  if grep -q "serving" "$SERVER_LOG" 2>/dev/null; then up=1; break; fi
  sleep 1
done
[[ "$up" -eq 1 ]] || {
  echo "error: server did not reach 'serving' within ${SERVER_WAIT}s (see ${SERVER_LOG})" >&2
  exit 1
}
echo "[certus-shmq] server serving, mailbox ${SHM_PATH} — launching client (prometheus :${PROM_PORT})"
echo "[certus-shmq] scrape client metrics at: http://localhost:${PROM_PORT}/metrics"

# run-bench.sh handles the client container (GPU, --ipc=host, HF cache, store).
# No CERTUS_SERVER: the shared /dev/shm mailbox at SHM_PATH is the endpoint.
# PROM_PORT / LOG_STATS / WORKLOAD_SRC are the added knobs it understands.
rc=0
IMAGE="$IMAGE" \
GPU="$GPU" \
SHM_PATH="$SHM_PATH" \
NUM_CONVS="$NUM_CONVS" \
MAX_ROUNDS="$MAX_ROUNDS" \
MODEL="$MODEL" \
SLAB_SIZE_BYTES="$SLAB_SIZE_BYTES" \
TENSOR_PARALLEL_SIZE="$TENSOR_PARALLEL_SIZE" \
HF_CACHE="$HF_CACHE" \
PODMAN_STORE="$PODMAN_STORE" \
PODMAN_RUNROOT="$PODMAN_RUNROOT" \
PROM_PORT="$PROM_PORT" \
LOG_STATS="$LOG_STATS" \
WORKLOAD_SRC="$WORKLOAD_SRC" \
  bash "${REPO_ROOT}/certus-shmq-connector/run-bench.sh" 2>&1 | tee "$LOG" || true
rc="${PIPESTATUS[0]}"

stop_server
exit "$rc"
