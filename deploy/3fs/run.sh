#!/bin/bash
# run.sh — start the single-node 3FS cluster container and export its mount (ROOT).
#
#   sudo ./run.sh                    # defaults below; detaches, waits until mounted
#   sudo DATA_DIR=/other/xfs ./run.sh
#   sudo STORE_DIR=/home/podman-3fs-root ./run.sh   # root's image store in this folder
#   sudo BUILD_STORE_DIR=/home/dwaddington/podman-3fs STORE_DIR=… ./run.sh  # if build.sh used STORE_DIR
#
# Why rootful + these flags:
#   --privileged      /dev/fuse, mount(2), /dev/infiniband/*, sysctl fs.aio-max-nr
#   --network host    RDMA CM + the NIC's real IP; mgmtd/meta/storage listen on host ports
#   --ipc host        USRBIO hands buffers over /dev/shm: the vLLM client's shm must be
#                     visible to hf3fs_fuse_main, so BOTH containers share the host IPC ns
#   MOUNT:rshared     the FUSE mount made inside the container propagates to the host
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
[[ $EUID -eq 0 ]] || { echo "error: run as root (sudo ./run.sh)" >&2; exit 1; }
source ./common.sh   # STORE_DIR → root's podman store (default /var/lib/containers)

IMAGE="${IMAGE:-localhost/certus-3fs-cluster:latest}"
NAME="${NAME:-certus-3fs}"
DATA_DIR="${DATA_DIR:-/cache/3fs}"       # on nvme0n1 (XFS). FDB + storage targets + logs + state
MOUNT="${MOUNT:-/mnt/3fs}"               # host mountpoint for the 3FS FUSE client
NET_IFACE="${NET_IFACE:-ens6f0np0}"      # RoCE netdev of IB_DEVICE (port ACTIVE)
IB_DEVICE="${IB_DEVICE:-mlx5_0}"
NETWORK="${NETWORK:-RDMA}"
# 3FS storage moves file data ONLY by RDMA (the server RDMA-reads/writes the client's
# registered buffer): over TCP, writes fail "update no RDMA socket" and reads "batch
# read no RDMA socket" (src/storage/service/StorageOperator.cc). There is no TCP mode.
if [[ "${NETWORK^^}" != RDMA ]]; then
  echo "error: NETWORK=$NETWORK is not supported — 3FS's data path requires RDMA." >&2
  exit 1
fi
NUM_CHAINS="${NUM_CHAINS:-16}"           # = targets on the one disk; >= vLLM file stripe
MGMTD_PORT="${MGMTD_PORT:-8100}"         # uses 8100-8102, 9100-9102 (clear of vLLM :8000)
# Owner of the KV dir $MOUNT/vllm — the user who runs the (rootless) vLLM container.
OWNER="${OWNER:-${SUDO_USER:-root}}"

# Root's store is separate from the builder's rootless store: copy the image across
# when root has none, or when the builder's image is newer (different id after a rebuild).
mapfile -t BUILD_FLAGS < <(store_flags "${BUILD_STORE_DIR:-}")
BUILT_ID=""
if [[ -n "${SUDO_USER:-}" ]]; then
  BUILT_ID=$(sudo -u "$SUDO_USER" podman "${BUILD_FLAGS[@]}" image inspect -f '{{.Id}}' "$IMAGE" 2>/dev/null || true)
fi
ROOT_ID=$("${PODMAN[@]}" image inspect -f '{{.Id}}' "$IMAGE" 2>/dev/null || true)
if [[ -z "$ROOT_ID" || ( -n "$BUILT_ID" && "$BUILT_ID" != "$ROOT_ID" ) ]]; then
  [[ -n "$BUILT_ID" ]] || { echo "error: $IMAGE not in root's store, nor in ${SUDO_USER:-<no SUDO_USER>}'s to copy from" >&2; exit 1; }
  need_space "${STORE_DIR:-/var/lib/containers}" 8 "root's copy of $IMAGE (~3.2G + staging)"
  echo "[run] copying $IMAGE from ${SUDO_USER}'s rootless store into root's store${STORE_DIR:+ ($STORE_DIR)}"
  # Stream save→load: no intermediate archive file. The user's podman gets its own
  # writable TMPDIR on a big fs (same one build.sh used); root's load uses $TMPDIR.
  if [[ -n "${BUILD_STORE_DIR:-}" ]]; then
    USER_TMP="$BUILD_STORE_DIR/tmp"
  else
    USER_TMP="$(getent passwd "$SUDO_USER" | cut -d: -f6)/.cache/certus-3fs-buildtmp"
  fi
  sudo -u "$SUDO_USER" env TMPDIR="$USER_TMP" podman "${BUILD_FLAGS[@]}" save "$IMAGE" \
    | "${PODMAN[@]}" load
fi
# admin.sh / teardown.sh find the container through the same store.
echo "${STORE_DIR:-}" > "$_3FS_SAVED"

if "${PODMAN[@]}" container exists "$NAME"; then
  echo "error: container '$NAME' already exists (running or stopped). ./teardown.sh first." >&2
  exit 1
fi

[[ -d "$(dirname "$DATA_DIR")" ]] || { echo "error: $(dirname "$DATA_DIR") missing — is nvme0n1 mounted?" >&2; exit 1; }
mkdir -p "$DATA_DIR" "$MOUNT"
# A shared bind of the mountpoint onto itself, so the container's rshared bind can
# propagate its FUSE mount back to the host namespace.
if ! findmnt -n "$MOUNT" >/dev/null; then
  mount --bind "$MOUNT" "$MOUNT"
fi
mount --make-shared "$MOUNT"

"${PODMAN[@]}" run -d --name "$NAME" --pull=never \
  --privileged --network host --ipc host \
  --ulimit memlock=-1:-1 --ulimit nofile=1048576:1048576 \
  -v "$DATA_DIR:/data:z" \
  -v "$MOUNT:/mnt/3fs:rshared" \
  -e NET_IFACE="$NET_IFACE" -e IB_DEVICE="$IB_DEVICE" -e NETWORK="$NETWORK" \
  -e NUM_CHAINS="$NUM_CHAINS" -e MGMTD_PORT="$MGMTD_PORT" \
  -e OWNER_UID="$(id -u "$OWNER")" -e OWNER_GID="$(id -g "$OWNER")" \
  "$IMAGE" >/dev/null

echo "[run] started $NAME; waiting for the mount (logs: ${PODMAN[*]} logs -f $NAME)"
for _ in $(seq 1 300); do
  if "${PODMAN[@]}" logs "$NAME" 2>&1 | grep -q '\] UP —'; then
    findmnt "$MOUNT"
    echo "[run] 3FS is up: $MOUNT (vLLM KV dir: $MOUNT/vllm, owner $OWNER). Next: ./smoke.sh"
    exit 0
  fi
  if [[ "$("${PODMAN[@]}" inspect -f '{{.State.Running}}' "$NAME")" != "true" ]]; then
    echo "[run] container exited during startup:" >&2
    "${PODMAN[@]}" logs --tail 40 "$NAME" >&2
    echo "[run] full service logs: $DATA_DIR/log/" >&2
    exit 1
  fi
  sleep 1
done
echo "[run] timed out waiting for 3FS; see: ${PODMAN[*]} logs $NAME ; $DATA_DIR/log/" >&2
exit 1
