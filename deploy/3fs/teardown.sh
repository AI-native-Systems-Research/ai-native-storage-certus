#!/bin/bash
# teardown.sh — stop the 3FS cluster container and unmount (ROOT).
#
#   sudo ./teardown.sh           # stop + remove container; data in DATA_DIR is KEPT
#                                #   (the next run.sh resumes the same cluster)
#   sudo ./teardown.sh --wipe    # ALSO delete DATA_DIR (FDB, targets, token) — asks first
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "error: run as root (sudo ./teardown.sh)" >&2; exit 1; }
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"   # same STORE_DIR run.sh used (saved)

NAME="${NAME:-certus-3fs}"
DATA_DIR="${DATA_DIR:-/cache/3fs}"
MOUNT="${MOUNT:-/mnt/3fs}"

if "${PODMAN[@]}" container exists "$NAME"; then
  "${PODMAN[@]}" stop -t 60 "$NAME" >/dev/null || true
  "${PODMAN[@]}" rm -f "$NAME" >/dev/null
  echo "[teardown] removed container $NAME"
fi

# The FUSE mount, then our shared self-bind underneath it.
while findmnt -n "$MOUNT" >/dev/null; do
  umount "$MOUNT" 2>/dev/null || umount -l "$MOUNT"
done
echo "[teardown] $MOUNT unmounted"

if [[ "${1:-}" == "--wipe" ]]; then
  [[ -d "$DATA_DIR/state" || -d "$DATA_DIR/fdb" ]] \
    || { echo "error: $DATA_DIR does not look like a 3FS data dir; refusing to delete" >&2; exit 1; }
  du -sh "$DATA_DIR"
  read -r -p "[teardown] permanently delete $DATA_DIR ? [y/N] " ans
  [[ "$ans" == [yY] ]] || { echo "[teardown] kept $DATA_DIR"; exit 0; }
  rm -rf --one-file-system "$DATA_DIR"
  echo "[teardown] wiped $DATA_DIR"
fi
