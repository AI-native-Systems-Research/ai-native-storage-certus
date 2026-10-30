#!/usr/bin/env bash
# build-gdrdrv-rhcos.sh — build the gdrdrv kernel module + gdrcopy userspace tools
# on an RHCOS / OpenShift GPU-Operator node, where the host root is read-only and
# kernel modules must be built in a container against the host kernel.
#
# Why this exists: GDRCopy (Path 5) and the nvme-bar1-bench BAR1 microbench
# (Path 9) both need the gdrdrv kernel module loaded (they open /dev/gdrdrv).
# gdrdrv is NOT loaded on this node and cannot be dnf-installed (read-only OS).
# The GPU Operator's driver-toolkit container already carries the exact kernel
# build tree (/usr/src/kernels/$(uname -r)) and the nvidia driver source, so we
# build gdrdrv there, copy the .ko out, and insmod it on the host.
#
# This script does the BUILD (in-container, non-destructive) and STAGING only.
# It PRINTS the insmod commands for you to run — it does not load the module
# itself, because inserting a kernel module into a live GPU-Operator node is a
# privileged, host-wide action you should execute deliberately.
#
# Prereqs (verified on ru10 2026-10-28):
#   - crictl present; driver-toolkit + nvidia-driver containers running in the
#     nvidia-gpu-operator namespace.
#   - gdrcopy source in-repo at kernel/modules/gdrcopy/.
#
# Usage:
#   sudo benchmarks/hw-ceiling/build-gdrdrv-rhcos.sh [outdir]   # default: ./gdrdrv-build

set -euo pipefail

[[ "$(id -u)" -eq 0 ]] || { echo "run as root (sudo): needs crictl + host staging" >&2; exit 1; }

REPO_ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel 2>/dev/null || true)"
[[ -z "$REPO_ROOT" ]] && REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
GDR_SRC="$REPO_ROOT/kernel/modules/gdrcopy"
OUT="${1:-$REPO_ROOT/gdrdrv-build}"
KVER="$(uname -r)"

[[ -d "$GDR_SRC" ]] || { echo "gdrcopy source not found at $GDR_SRC" >&2; exit 1; }
mkdir -p "$OUT"

echo "=== locating the OpenShift driver-toolkit container ==="
# The driver-toolkit container carries /usr/src/kernels/$KVER (kernel headers).
DTK="$(crictl ps --name openshift-driver-toolkit-ctr -q 2>/dev/null | head -1)"
[[ -z "$DTK" ]] && DTK="$(crictl ps -q 2>/dev/null | while read -r id; do
  crictl exec "$id" test -d "/usr/src/kernels/$KVER" 2>/dev/null && { echo "$id"; break; }; done)"
[[ -z "$DTK" ]] && { echo "ERROR: no running container has /usr/src/kernels/$KVER" >&2; exit 1; }
echo "    driver-toolkit ctr: $DTK"
crictl exec "$DTK" ls "/usr/src/kernels/$KVER/Makefile" >/dev/null 2>&1 \
  && echo "    kernel build tree: OK" || { echo "ERROR: no Makefile in kernel tree" >&2; exit 1; }

echo "=== staging gdrcopy source into the container ==="
# crictl has no cp; stage via a tar stream through a host path the ctr can see.
# Simplest portable route: tar the source to stdout, untar inside the ctr.
tar -C "$GDR_SRC/.." -cf - "$(basename "$GDR_SRC")" | \
  crictl exec -i "$DTK" sh -c 'mkdir -p /tmp/gdrbuild && tar -C /tmp/gdrbuild -xf -'

echo "=== building gdrdrv.ko in the driver-toolkit container ==="
# Build only the kernel module (src/gdrdrv). KDIR points at the host kernel tree
# the toolkit provides. gdrcopy ships an nv-p2p-dummy fallback so it links even
# without nvidia's Module.symvers.
crictl exec "$DTK" sh -c "
  set -e
  cd /tmp/gdrbuild/gdrcopy
  make -C src/gdrdrv KVER='$KVER' KDIR='/usr/src/kernels/$KVER' 2>&1 | tail -20
  ls -la src/gdrdrv/gdrdrv.ko
"

echo "=== copying gdrdrv.ko out to the host ==="
crictl exec "$DTK" cat /tmp/gdrbuild/gdrcopy/src/gdrdrv/gdrdrv.ko > "$OUT/gdrdrv.ko"
echo "    staged: $OUT/gdrdrv.ko ($(stat -c %s "$OUT/gdrdrv.ko") bytes)"

echo "=== building gdrcopy userspace (libgdrapi + gdrcopy_copybw) in the container ==="
crictl exec "$DTK" sh -c "
  set -e
  cd /tmp/gdrbuild/gdrcopy
  make -C src lib 2>&1 | tail -5 || true
  make -C tests 2>&1 | tail -5 || true
  ls tests/gdrcopy_copybw 2>/dev/null && echo COPYBW_OK || echo 'copybw not built (may need CUDA in ctr)'
"
crictl exec "$DTK" cat /tmp/gdrbuild/gdrcopy/tests/gdrcopy_copybw > "$OUT/gdrcopy_copybw" 2>/dev/null || true
crictl exec "$DTK" sh -c 'cat /tmp/gdrbuild/gdrcopy/src/libgdrapi.so* 2>/dev/null' > "$OUT/libgdrapi.so" 2>/dev/null || true

echo
echo "=== BUILD DONE. To LOAD the module (host-wide; run deliberately): ==="
echo "  sudo insmod $OUT/gdrdrv.ko"
echo "  ls -l /dev/gdrdrv        # should appear"
echo "  sudo chmod a+rw /dev/gdrdrv   # so non-root / containers can open it"
echo
echo "Then GDRCopy (Path 5) and the nvme-bar1-bench BAR1 mode (Path 9) can run."
echo "To unload later:  sudo rmmod gdrdrv"
echo
echo "NOTE: gdrdrv was built against nvidia $(ls -d /run/nvidia/driver/usr/src/nvidia-* 2>/dev/null | sed 's#.*/nvidia-##'). If the"
echo "GPU Operator updates the driver, rebuild before loading."
