#!/usr/bin/env bash
# run-spdk-nvme.sh — one-command SPDK spdk_nvme_perf read/write on ONE scratch
# NVMe drive (Paths 1-2), on a host where SPDK was built via run-spdk-build.sh.
#
# The drive is named by PCI BDF (e.g. 0000:41:00.0), consistent with --nvme and
# the NVME_BDFS convention of configure-bench.sh / profile_all.sh. It does the
# whole destructive dance safely:
#   derive the drive's block node  ->  refuse if that node (or a partition) is
#   mounted  ->  bind ONLY that BDF to vfio-pci + hugepages  ->  run perf (read,
#   then optionally write)  ->  rebind the drive back to the kernel driver.
#
# DESTRUCTIVE: with --write it overwrites the raw drive. Reads are safe.
# The drive is ALWAYS rebound to the kernel driver on exit (even on error/Ctrl-C).
#
# Usage (run as root — it needs vfio/hugepage privileges):
#   sudo benchmarks/hw-ceiling/run-spdk-nvme.sh 0000:41:00.0            # read only
#   sudo benchmarks/hw-ceiling/run-spdk-nvme.sh 0000:41:00.0 --write    # read + write (ERASES)
#
# Options:
#   --write          also run sequential write (overwrites the drive)
#   --spdk-dir DIR   where run-spdk-build.sh put things (default: ./spdk-host)
#   --sizes "A B C"  io sizes in bytes (default: "4096 65536 131072 1048576")
#   --qd N           queue depth (default 32)
#   --time N         seconds per run (default 5)

set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
  echo "error: must run as root (sudo) — it binds vfio-pci and allocates hugepages" >&2
  exit 1
fi

BDF=""
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
    -h|--help)   sed -n '2,27p' "$0"; exit 0 ;;
    --* )        echo "unknown option: $1" >&2; exit 2 ;;
    *)           BDF="$1"; shift ;;
  esac
done

[[ -z "$BDF" ]] && { echo "error: name a drive by PCI BDF, e.g. $0 0000:41:00.0" >&2; exit 2; }
BDF="${BDF,,}"
# accept short "41:00.0" too
[[ "$BDF" =~ ^[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]] && BDF="0000:${BDF}"
[[ "$BDF" =~ ^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]] || {
  echo "error: '$BDF' is not a PCI BDF (want 0000:41:00.0)" >&2; exit 2; }
[[ -d "/sys/bus/pci/devices/${BDF}" ]] || {
  echo "error: BDF '$BDF' is not a PCI device on this host" >&2; exit 2; }

PERF="$SPDK_DIR/bin/spdk_nvme_perf"
SETUP="$SPDK_DIR/scripts/setup.sh"
LIB="$SPDK_DIR/lib"
[[ -x "$PERF" ]]  || { echo "error: $PERF not found — run run-spdk-build.sh first" >&2; exit 1; }
[[ -f "$SETUP" ]] || { echo "error: $SETUP not found — run run-spdk-build.sh first" >&2; exit 1; }

# Derive the kernel block node from the BDF (for the mount-safety check only).
# The namespace block dir lives under the controller's sysfs dir and may be named
# differently from the controller under NVMe multipath (nvme3 -> nvme1n2).
DEV=""
cdir="$(ls -d "/sys/bus/pci/devices/${BDF}/nvme/"nvme* 2>/dev/null | head -1)"
if [[ -n "$cdir" ]]; then
  blk="$(ls -d "$cdir"/nvme* 2>/dev/null | grep -E '/nvme[0-9]+n[0-9]+$' | head -1)"
  [[ -n "$blk" ]] && DEV="/dev/$(basename "$blk")"
fi

# SAFETY: refuse if the drive's block node (or a partition) is mounted. If the
# drive is already on vfio-pci there is no block node — nothing to check, proceed.
if [[ -n "$DEV" ]]; then
  MOUNTED="$(lsblk -nro NAME,MOUNTPOINT "$DEV" 2>/dev/null | awk '$2!=""{print}')"
  if [[ -n "$MOUNTED" ]]; then
    echo "REFUSING: $DEV ($BDF, or a partition) is mounted:" >&2
    echo "$MOUNTED" >&2
    echo "Unmount it first, or pick a scratch drive." >&2
    exit 1
  fi
fi

echo "=== SPDK NVMe perf on $BDF  (block node: ${DEV:-<on vfio-pci>}) ==="
echo "    sizes: $SIZES   QD: $QD   time: ${TIME}s   write: $([[ $WRITE == 1 ]] && echo yes || echo no)"
[[ $WRITE == 1 ]] && echo "    WARNING: --write will ERASE this drive"

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
