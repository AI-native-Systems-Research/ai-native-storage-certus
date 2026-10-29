#!/bin/bash
# entrypoint.sh — role dispatch for the vLLM + LMCache image.
#
#   lmcache-server <lmcache server args…>   preflight the L2 dir, then `lmcache server`
#   <anything else>                          `vllm serve <args…>`
#
# Server preflight (from env, set by run.sh): L2_DIR must be writable (with
# O_DIRECT when USE_ODIRECT=1 — tmpfs/overlay refuse it) and have DISK_GB free.
# These would otherwise surface only as dropped stores or a mid-run ENOSPC.
set -euo pipefail

log() { echo "[lmcache-entry] $*"; }
die() { echo "[lmcache-entry] FATAL: $*" >&2; exit 1; }

if [[ "${1:-}" != "lmcache-server" ]]; then
  exec vllm serve "$@"
fi
shift

L2_DIR="${L2_DIR:?L2_DIR is required for lmcache-server}"
mkdir -p "$L2_DIR" || die "cannot create $L2_DIR"
probe="$L2_DIR/.probe.$$"
if [[ "${USE_ODIRECT:-1}" == "1" ]]; then
  dd if=/dev/zero of="$probe" bs=1M count=4 oflag=direct status=none \
    || die "O_DIRECT write failed in $L2_DIR (tmpfs/overlay?) — bind an SSD filesystem or set USE_ODIRECT=0"
else
  dd if=/dev/zero of="$probe" bs=1M count=4 status=none || die "$L2_DIR is not writable"
fi
rm -f "$probe"
want_gb="${DISK_GB:-0}"
avail_gb=$(df -BG --output=avail "$L2_DIR" | tail -1 | tr -dc 0-9)
(( want_gb <= avail_gb )) || die "L2 ${want_gb}G > ${avail_gb}G free on $(df --output=target "$L2_DIR" | tail -1)"
log "lmcache $(cat /opt/lmcache/LMCACHE_VERSION): L2 $L2_DIR ${want_gb}G of ${avail_gb}G free ($(df --output=fstype "$L2_DIR" | tail -1))"

exec lmcache server "$@"
