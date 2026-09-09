#!/bin/bash
# kv-offload-otel-replay on Certus-SHMQ — shared-memory client container against
# a host certus-server (SPDK NVMe). The OTel-corpus counterpart of
# run-docker-certus-shmq.sh, but CLIENT-ONLY: it assumes a certus-server is
# ALREADY running and has published the mailbox at SHM_PATH. It does NOT start or
# manage the server (that needs sudo / vfio-pci / hugepages) — start it exactly
# as for the ShareGPT tool:
#
#   * bind the NVMe device(s) to vfio-pci + reserve 1G hugepages
#     (certus-shmq-connector/setup-host.sh or tools/configure-bench.sh), then
#   * run target/release/certus-server --shm-path /dev/shm/certus-shmq ...
#     (run-docker-certus-shmq.sh shows the full server invocation), OR just use
#     run-docker-certus-shmq.sh to bring the server up and swap its client run
#     line for this script.
#
#   ./run-docker-otel-shmq.sh
#   NUM_CONVS=8 TIME_SCALE=0 ./run-docker-otel-shmq.sh          # quick smoke
#   SHM_PATH=/dev/shm/certus-shmq SLAB_SIZE_BYTES=2097152 ./run-docker-otel-shmq.sh
#
# The client shares the mailbox via --ipc=host (which also lets the host server
# open this container's CUDA IPC handles). There is no server address — SHM_PATH
# IS the endpoint. The ~22 GB corpus is bind-mounted read-only (OTEL_HOST) by
# run-docker-common-otel.sh.
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common-otel.sh"

# The shmq image lives in the /mnt/certus1 podman store, not the default store
# (mirrors run-docker-certus-shmq.sh).
IMAGE="${IMAGE:-localhost/certus-otel-shmq-bench}"
PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"
STORE_FLAGS=(--root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT")

SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"        # mailbox file (shared in)
SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"       # offload block size
WAIT_SECS="${WAIT_SECS:-180}"                       # mailbox-ready wait
LOG="${LOG:-${SCRIPT_DIR}/otel_shmq_$(stamp).log}"

require_image "$IMAGE" "${STORE_FLAGS[@]}"

if [[ ! -e "$SHM_PATH" ]]; then
  echo "warning: mailbox ${SHM_PATH} does not exist yet — is certus-server running?" >&2
  echo "         the client waits up to ${WAIT_SECS}s for it (see header for server start)." >&2
fi

# Client run: --ipc=host to share the mailbox + expose CUDA IPC handles; the
# shmq-specific env on top of COMMON_RUN_ARGS. --pull=never against the alt store.
echo "[run] ${IMAGE}  ->  ${LOG}"
command podman "${STORE_FLAGS[@]}" run --rm --pull=never \
  --ipc=host \
  "${COMMON_RUN_ARGS[@]}" \
  -e "SHM_PATH=${SHM_PATH}" \
  -e "SLAB_SIZE_BYTES=${SLAB_SIZE_BYTES}" \
  -e "WAIT_SECS=${WAIT_SECS}" \
  "$IMAGE" 2>&1 | tee "$LOG"
exit "${PIPESTATUS[0]}"
