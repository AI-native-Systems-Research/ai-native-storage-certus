#!/bin/bash
# Certus-SPDK — host certus-server-yaml (SPDK NVMe) + gRPC client container.
#
# Two parts, both handled here: start the host server, wait for :50051, run the
# gRPC bench client (certus-grpc-connector/run-bench.sh), then tear the server
# down (SIGTERM, then SIGKILL after 8s — SPDK ignores SIGTERM during teardown).
#
#   ./run-docker-certus-spdk.sh
#   DEVICE_PCI="0000:61:00.0 0000:62:00.0" MEM_TIER_SIZE=16G ./run-docker-certus-spdk.sh
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
LOG="${LOG:-${SCRIPT_DIR}/certus-spdk_$(stamp).log}"
SERVER_LOG="${SERVER_LOG:-${SCRIPT_DIR}/server_$(stamp).log}"

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
echo "[certus-spdk] server up on :50051 — launching gRPC client"

# run-bench.sh handles the client container (GPU, --ipc=host, HF cache, store).
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
  bash "${REPO_ROOT}/certus-grpc-connector/run-bench.sh" 2>&1 | tee "$LOG" || true
rc="${PIPESTATUS[0]}"

stop_server
exit "$rc"
