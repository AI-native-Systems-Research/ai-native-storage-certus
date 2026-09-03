#!/bin/bash
# Certus-SHMQ — host certus-server (SPDK NVMe) + shared-memory client container.
#
# The control transport is a /dev/shm mailbox. The two consequences:
#   * the server takes --shm-path (NOT --listen); there is no TCP port to poll,
#     so readiness = the server logging "serving" (it publishes the mailbox last).
#   * the client container shares the mailbox via --ipc=host (run-bench.sh does
#     this), which also lets the host server open the container's CUDA IPC
#     handles. There is no server address to pass — SHM_PATH IS the endpoint.
#
#   ./run-docker-certus-shmq.sh
#   NUM_CONVS=8 MAX_ROUNDS=2 ./run-docker-certus-shmq.sh          # quick smoke
#   DEVICE_PCI="0000:61:00.0 0000:62:00.0" MEM_TIER_SIZE=16G ./run-docker-certus-shmq.sh
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
LOG="${LOG:-${SCRIPT_DIR}/certus-shmq_$(stamp).log}"
SERVER_LOG="${SERVER_LOG:-${SCRIPT_DIR}/server-shmq_$(stamp).log}"

# Data-parallel fan-out: DP_SIZE>1 runs that many independent client containers
# (one per GPU in GPUS) against this ONE server / shared mailbox, each replaying
# a disjoint conversation shard. GPUS defaults to 0,1,... for DP_SIZE replicas.
# DP_SIZE=1 keeps the original single-client behaviour exactly.
DP_SIZE="${DP_SIZE:-1}"
GPUS="${GPUS:-}"
if [[ -z "$GPUS" ]]; then
  if [[ "$DP_SIZE" -gt 1 ]]; then
    GPUS="$(seq -s' ' 0 $((DP_SIZE - 1)))"   # 0 1 ... DP_SIZE-1
  else
    GPUS="$GPU"                               # single replica: inherit GPU (default "all")
  fi
fi
CONNECTOR_SRC="${CONNECTOR_SRC:-}"            # optional connector-package override (bind-mounted)

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
echo "[certus-shmq] server serving, mailbox ${SHM_PATH} — launching client"

# run-bench.sh handles the client container (GPU, --ipc=host, HF cache, store).
# No CERTUS_SERVER: the shared /dev/shm mailbox at SHM_PATH is the endpoint.
# One invocation = one replica; DP_RANK/DP_SIZE select its conversation shard and
# its disjoint channel-claim partition on the shared mailbox.
run_client() {
  local gpu="$1" dp_rank="$2" log="$3"
  IMAGE="$IMAGE" \
  GPU="$gpu" \
  DP_RANK="$dp_rank" \
  DP_SIZE="$DP_SIZE" \
  SHM_PATH="$SHM_PATH" \
  NUM_CONVS="$NUM_CONVS" \
  MAX_ROUNDS="$MAX_ROUNDS" \
  DATASET_HOST="${DATASET_HOST:-}" \
  WORKLOAD_MODE="${WORKLOAD_MODE:-batched}" \
  ACTIVE_SESSIONS="${ACTIVE_SESSIONS:-0}" \
  WORKLOAD_SRC="${WORKLOAD_SRC:-}" \
  ASYNC_SRC="${ASYNC_SRC:-}" \
  CONNECTOR_SRC="${CONNECTOR_SRC:-}" \
  MODEL="$MODEL" \
  SLAB_SIZE_BYTES="$SLAB_SIZE_BYTES" \
  TENSOR_PARALLEL_SIZE="$TENSOR_PARALLEL_SIZE" \
  ENFORCE_EAGER="${ENFORCE_EAGER:-0}" \
  HF_CACHE="$HF_CACHE" \
  PODMAN_STORE="$PODMAN_STORE" \
  PODMAN_RUNROOT="$PODMAN_RUNROOT" \
    bash "${REPO_ROOT}/certus-shmq-connector/run-bench.sh" 2>&1 | tee "$log"
  return "${PIPESTATUS[0]}"
}

rc=0
if [[ "$DP_SIZE" -le 1 ]]; then
  # Single replica: original path, original single $LOG, exact prior behaviour.
  run_client "$GPU" 0 "$LOG" || true
  rc=$?
else
  # Data-parallel: launch one client per GPU in the background, all sharing the
  # one server/mailbox, then wait for every replica before tearing the server
  # down. Per-replica logs so the two tee streams don't clobber each other.
  read -r -a gpu_arr <<< "$GPUS"
  echo "[certus-shmq] DP fan-out: DP_SIZE=${DP_SIZE} GPUS='${GPUS}'"
  declare -a pids=() logs=()
  for r in $(seq 0 $((DP_SIZE - 1))); do
    gpu="${gpu_arr[$r]}"
    rlog="${LOG%.log}.gpu${gpu}.log"
    logs[$r]="$rlog"
    echo "[certus-shmq] replica ${r}: GPU=${gpu} -> ${rlog}"
    run_client "$gpu" "$r" "$rlog" &
    pids[$r]=$!
  done
  for r in $(seq 0 $((DP_SIZE - 1))); do
    if wait "${pids[$r]}"; then :; else
      echo "[certus-shmq] replica ${r} (GPU ${gpu_arr[$r]}) exited non-zero" >&2
      rc=1
    fi
  done
fi

stop_server
exit "$rc"
