#!/bin/bash
# Certus-SPDK + Prometheus — same as run-docker-certus-spdk.sh (host
# certus-server-yaml over SPDK NVMe + gRPC bench client), but with the client's
# vLLM engine exposing Prometheus metrics on port 8000.
#
# The bench drives vLLM through the offline LLM(...) engine (no OpenAI server),
# so there is no /metrics endpoint unless the workload opens one itself. This
# variant does three things the base script does not:
#   * LOG_STATS=1        — so vLLM registers its PrometheusStatLogger metrics
#   * PROM_PORT=8000      — the workload calls start_http_server(8000)
#   * WORKLOAD_SRC=...    — bind-mounts the repo's run_multiturn_grpc_certus.py
#                           over the image copy, so the exporter works WITHOUT
#                           rebuilding the certus-grpc-bench image.
# Scrape the client from the host at:  http://localhost:8000/metrics
#
# NOTE: these are the CLIENT-side vLLM + KV-offload metrics. The SPDK/SSD-side
# counters live in the host certus-server and are read separately via its
# GetIoStats RPC (printed per round in the bench log) — they are NOT in this
# Prometheus registry.
#
#   ./run-docker-certus-spdk-prom.sh
#   PROM_PORT=9100 DEVICE_PCI="0000:61:00.0" ./run-docker-certus-spdk-prom.sh
#
# PREREQUISITES (not performed here — done by tools/configure-bench.sh, needs sudo):
#   * every device in DEVICE_PCI bound to vfio-pci (not the kernel nvme driver)
#   * 1G hugepages reserved on NUMA_NODE
# The host is assumed to already be in this state (as after a bench reconfigure).
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

# The gRPC image lives in the /mnt/certus1 podman store, not the default store.
IMAGE="${IMAGE:-localhost/certus-grpc-bench}"
PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"

DEVICE_PCI="${DEVICE_PCI:-0000:61:00.0 0000:62:00.0 0000:63:00.0}"  # vfio-pci NVMe group
MEM_TIER_SIZE="${MEM_TIER_SIZE:-32G}"
EVICT_THRESH="${EVICT_THRESH:-0.6}"
SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"
TENSOR_PARALLEL_SIZE="${TENSOR_PARALLEL_SIZE:-1}"
SERVER_WAIT="${SERVER_WAIT:-180}"                  # seconds to wait for :50051
NUMA_NODE="${NUMA_NODE:-0}"                         # pin server to the NVMe/hugepage node
SERVER_BIN="${SERVER_BIN:-${REPO_ROOT}/target/release/certus-server-yaml}"
LOG="${LOG:-${SCRIPT_DIR}/certus-spdk-prom_$(stamp).log}"
SERVER_LOG="${SERVER_LOG:-${SCRIPT_DIR}/server_$(stamp).log}"

# ── Prometheus wiring (the only substantive difference from the base script) ──
PROM_PORT="${PROM_PORT:-8000}"        # host port the client exporter is published on
LOG_STATS="${LOG_STATS:-1}"           # 1 = register vLLM metrics (empty /metrics otherwise)
# Bind-mount the repo's workload over the image copy so the exporter block takes
# effect without a rebuild. Override/blank to use the image's baked workload
# instead (only correct once the image is rebuilt with the exporter code).
WORKLOAD_SRC="${WORKLOAD_SRC:-${REPO_ROOT}/certus-grpc-connector/run_multiturn_grpc_certus.py}"

[[ -x "$SERVER_BIN" ]] || {
  echo "error: server binary not built at ${SERVER_BIN}" >&2
  echo "       build it: CERTUS_PROFILE=full cargo build --release -p certus-server-yaml" >&2
  exit 1
}
require_image "$IMAGE" --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT"

dev_flags=(); for d in $DEVICE_PCI; do dev_flags+=(--device-pci "$d"); done
numa_prefix=()
command -v numactl >/dev/null 2>&1 && numa_prefix=(numactl "--cpunodebind=${NUMA_NODE}" "--membind=${NUMA_NODE}")

SERVER_PID=""
stop_server() {
  [[ -z "$SERVER_PID" ]] && return 0
  kill -0 "$SERVER_PID" 2>/dev/null || { SERVER_PID=""; return 0; }
  echo "[certus-spdk] stopping server (pid ${SERVER_PID})"
  kill -TERM "$SERVER_PID" 2>/dev/null
  for _ in $(seq 1 8); do
    kill -0 "$SERVER_PID" 2>/dev/null || { SERVER_PID=""; return 0; }
    sleep 1
  done
  echo "[certus-spdk] server ignored SIGTERM (SPDK teardown) — escalating to SIGKILL"
  kill -9 "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null || true
  SERVER_PID=""
}
trap stop_server EXIT

echo "[certus-spdk] starting server: ${dev_flags[*]} --memory-tier-size ${MEM_TIER_SIZE} (numa ${NUMA_NODE})"
"${numa_prefix[@]}" "$SERVER_BIN" "${dev_flags[@]}" \
  --memory-tier-size "$MEM_TIER_SIZE" \
  --memory-tier-eviction-threshold "$EVICT_THRESH" \
  --listen 0.0.0.0:50051 \
  --format \
  > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

up=0
for _ in $(seq 1 "$SERVER_WAIT"); do
  kill -0 "$SERVER_PID" 2>/dev/null || break
  if (exec 3<>/dev/tcp/127.0.0.1/50051) 2>/dev/null; then exec 3>&- 3<&-; up=1; break; fi
  sleep 1
done
[[ "$up" -eq 1 ]] || {
  echo "error: server :50051 did not come up within ${SERVER_WAIT}s (see ${SERVER_LOG})" >&2
  exit 1
}
echo "[certus-spdk] server up on :50051 — launching gRPC client (prometheus :${PROM_PORT})"
echo "[certus-spdk] scrape client metrics at: http://localhost:${PROM_PORT}/metrics"

# run-bench.sh handles the client container (GPU, --ipc=host, HF cache, store).
# PROM_PORT / LOG_STATS / WORKLOAD_SRC are the added knobs it now understands.
rc=0
IMAGE="$IMAGE" \
GPU="$GPU" \
CERTUS_SERVER="host.containers.internal:50051" \
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
  bash "${REPO_ROOT}/certus-grpc-connector/run-bench.sh" 2>&1 | tee "$LOG" || true
rc="${PIPESTATUS[0]}"

stop_server
exit "$rc"
