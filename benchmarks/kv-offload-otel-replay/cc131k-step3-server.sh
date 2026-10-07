#!/bin/bash
# cc131k-step3-server.sh — STEP 3 (Certus-shmq arm ONLY): start the host
# certus-server that publishes the shmq mailbox the shmq replay client attaches
# to. The offload / cputier / nooffload arms are self-contained (one vLLM
# process) and do NOT need this — skip straight to step 4 for those.
#
# PREREQUISITES (not done here):
#   * certus-server built:   CERTUS_PROFILE=full-optimized cargo build -r \
#                              -p certus-server --features spdk
#   * NVMe bound to vfio-pci + 1 GiB hugepages:  sudo tools/configure-bench.sh
# This script runs in the FOREGROUND and holds the terminal — start it in its
# own shell/tmux, then run step 4 in another.
#
# Override via env (defaults mirror HOWTO.manually-run.md):
#   SERVER_BIN=target/release/certus-server \
#   DEVICE_PCIS="0000:61:00.0 0000:62:00.0 0000:63:00.0" \
#   SHM_PATH=/dev/shm/certus-shmq MEMORY_TIER_SIZE=13G CHANNELS=32 \
#   ./cc131k-step3-server.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

SERVER_BIN="${SERVER_BIN:-${REPO_ROOT}/target/release/certus-server}"
DEVICE_PCIS="${DEVICE_PCIS:-0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0}"
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
MEMORY_TIER_SIZE="${MEMORY_TIER_SIZE:-32G}"
EVICTION_THRESHOLD="${EVICTION_THRESHOLD:-0.9}"
STORE_BACKPRESSURE_MS="${STORE_BACKPRESSURE_MS:-5000}"
CHANNELS="${CHANNELS:-32}"
POLLER_BASE_CPU="${POLLER_BASE_CPU:-2}"
SHMQ_POLLER_CPU="${SHMQ_POLLER_CPU:-6}"
CPUNODE="${CPUNODE:-0}"

if [[ ! -x "$SERVER_BIN" ]]; then
  echo "error: certus-server binary not found/executable: $SERVER_BIN" >&2
  echo "       build it:  CERTUS_PROFILE=full-optimized cargo build -r -p certus-server --features spdk" >&2
  exit 1
fi

# Expand the space-separated PCI list into repeated --device-pci flags.
DEV_ARGS=()
for pci in $DEVICE_PCIS; do DEV_ARGS+=(--device-pci "$pci"); done

echo "[step3] server    : ${SERVER_BIN}"
echo "[step3] devices   : ${DEVICE_PCIS}"
echo "[step3] shm path  : ${SHM_PATH}  (use the same SHM_PATH in step 4 shmq arm)"
echo "[step3] mem tier  : ${MEMORY_TIER_SIZE}  (evict ${EVICTION_THRESHOLD}, channels ${CHANNELS})"
echo

exec numactl --cpunodebind="${CPUNODE}" --membind="${CPUNODE}" "$SERVER_BIN" \
  "${DEV_ARGS[@]}" \
  --shm-path "$SHM_PATH" \
  --memory-tier-size "$MEMORY_TIER_SIZE" \
  --memory-tier-eviction-threshold "$EVICTION_THRESHOLD" \
  --store-backpressure-ms "$STORE_BACKPRESSURE_MS" \
  --channels "$CHANNELS" \
  --poller-base-cpu "$POLLER_BASE_CPU" \
  --shmq-poller-cpu "$SHMQ_POLLER_CPU" \
  --format
