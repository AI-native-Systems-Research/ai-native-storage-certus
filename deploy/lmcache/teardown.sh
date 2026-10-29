#!/bin/bash
# teardown.sh — stop the vLLM and LMCache server containers (rootless).
#
#   ./teardown.sh           # stop + remove both; L2 chunk files in DISK_DIR/l2 are kept
#                           #   (unreachable after a restart — run.sh clears them)
#   ./teardown.sh --wipe    # ALSO delete DISK_DIR/l2
set -euo pipefail

NAME="${NAME:-certus-lmcache}"
DISK_DIR="${DISK_DIR:-/cache/lmcache}"

for c in "${NAME}-vllm" "${NAME}-server"; do
  if podman container exists "$c"; then
    podman stop -t 30 "$c" >/dev/null || true
    podman rm -f "$c" >/dev/null 2>&1 || true
    echo "[teardown] removed container $c"
  fi
done

if [[ "${1:-}" == "--wipe" && -d "$DISK_DIR/l2" ]]; then
  du -sh "$DISK_DIR/l2"
  rm -rf --one-file-system "$DISK_DIR/l2"
  echo "[teardown] deleted $DISK_DIR/l2"
fi
