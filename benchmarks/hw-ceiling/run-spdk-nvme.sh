#!/usr/bin/env bash
# run-spdk-nvme.sh — one-command SPDK spdk_nvme_perf read/write on ONE scratch
# NVMe device (Paths 1-2), on a host where SPDK was built via run-spdk-build.sh.
#
# It does the whole destructive dance safely:
#   resolve the device's PCI BDF  ->  refuse if it (or any sibling namespace) is
#   mounted  ->  bind ONLY that BDF to vfio-pci + hugepages  ->  run perf (read,
#   then optionally write)  ->  rebind the device back to the kernel driver.
#
# DESTRUCTIVE: with --write it overwrites the raw device. Reads are safe.
# The device is ALWAYS rebound to the kernel driver on exit (even on error/Ctrl-C).
#
# Usage (run as root — it needs vfio/hugepage privileges):
#   sudo benchmarks/hw-ceiling/run-spdk-nvme.sh /dev/nvme1n2            # read only
#   sudo benchmarks/hw-ceiling/run-spdk-nvme.sh /dev/nvme1n2 --write    # read + write (ERASES)
#
# Options:
#   --write          also run sequential write (overwrites the device)
#   --spdk-dir DIR   where run-spdk-build.sh put things (default: ./spdk-host)
#   --sizes "A B C"  io sizes in bytes (default: "4096 65536 131072 1048576")
#   --qd N           queue depth (default 32)
#   --time N         seconds per run (default 5)

set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
  echo "error: must run as root (sudo) — it binds vfio-pci and allocates hugepages" >&2
  exit 1
fi

DEV=""
WRITE=0
SPDK_DIR="./spdk-host"
SIZES="4096 65536 131072 1048576"
QD=32
TIME=5
while [[ $# -gt 0 ]]; do
  case "$1" in
    --write)     WRITE=1; shift ;;
    --spdk-dir)  SPDK_DIR="$2"; shift 2 ;;
    --sizes)     SIZES="$2"; shift 2 ;;
    --qd)        QD="$2"; shift 2 ;;
    --time)      TIME="$2"; shift 2 ;;
    -h|--help)   sed -n '2,26p' "$0"; exit 0 ;;
    /dev/*)      DEV="$1"; shift ;;
    *)           echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

[[ -z "$DEV" ]] && { echo "error: name a device, e.g. $0 /dev/nvme1n2" >&2; exit 2; }
[[ -b "$DEV" ]] || { echo "error: $DEV is not a block device" >&2; exit 2; }

PERF="$SPDK_DIR/bin/spdk_nvme_perf"
SETUP="$SPDK_DIR/scripts/setup.sh"
LIB="$SPDK_DIR/lib"
[[ -x "$PERF" ]]  || { echo "error: $PERF not found — run run-spdk-build.sh first" >&2; exit 1; }
[[ -f "$SETUP" ]] || { echo "error: $SETUP not found — run run-spdk-build.sh first" >&2; exit 1; }

# Resolve the PCI BDF for this namespace. The sysfs path contains the full PCI
# chain, e.g. .../0000:40:00.0/0000:41:00.0/nvme/nvme3 — the device's own BDF is
# the LAST pci address segment in that path. basename does not work here (it is
# "nvme3" under multipath), so extract the last "xxxx:xx:xx.x" from the realpath.
devpath="$(readlink -f "/sys/class/block/$(basename "$DEV")/device" 2>/dev/null)"
BDF="$(printf '%s\n' "$devpath" | grep -oE '[0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-9a-fA-F]' | tail -1)"
[[ "$BDF" =~ ^[0-9a-fA-F]{4}: ]] || { echo "error: could not resolve PCI BDF for $DEV (path: ${devpath:-none})" >&2; exit 1; }

# SAFETY: refuse if the device or any of its partitions is mounted.
MOUNTED="$(lsblk -nro NAME,MOUNTPOINT "$DEV" 2>/dev/null | awk '$2!=""{print}')"
if [[ -n "$MOUNTED" ]]; then
  echo "REFUSING: $DEV (or a partition) is mounted:" >&2
  echo "$MOUNTED" >&2
  echo "Unmount it first, or pick a scratch device." >&2
  exit 1
fi

echo "=== SPDK NVMe perf on $DEV  (BDF $BDF) ==="
echo "    sizes: $SIZES   QD: $QD   time: ${TIME}s   write: $([[ $WRITE == 1 ]] && echo yes || echo no)"
[[ $WRITE == 1 ]] && echo "    WARNING: --write will ERASE $DEV"

# Always rebind back to the kernel driver on exit, however we leave.
cleanup() {
  echo "=== rebinding $BDF back to the kernel NVMe driver ==="
  bash "$SETUP" reset >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

echo "=== binding ONLY $BDF to vfio-pci + hugepages ==="
HUGEMEM=8192 PCI_ALLOWED="$BDF" bash "$SETUP"

run_one() {  # $1 = read|write
  local wl="$1" o
  echo "--- $wl ---"
  for o in $SIZES; do
    echo "# $wl  io_size=$o  qd=$QD"
    LD_LIBRARY_PATH="$LIB" "$PERF" -r "trtype:PCIe traddr:$BDF" \
      -q "$QD" -o "$o" -w "$wl" -t "$TIME" 2>&1 | awk '/^Total/{print "  Total:",$0}'
  done
}

run_one read
[[ $WRITE == 1 ]] && run_one write

echo "=== done (device will be rebound to kernel driver now) ==="
