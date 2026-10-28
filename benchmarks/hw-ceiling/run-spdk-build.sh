#!/usr/bin/env bash
# run-spdk-build.sh — build spdk_nvme_perf in a container and extract it + its
# shared libraries to a host directory, for Paths 1-2 on a read-only bootc host.
#
# Usage:
#   export HTTPS_PROXY=http://<proxy>:<port>     # needed for the clone + dnf
#   benchmarks/hw-ceiling/run-spdk-build.sh [outdir]      # default: ./spdk-host
#
# Result: <outdir>/bin/spdk_nvme_perf and <outdir>/lib/*.so* . Run it as:
#   LD_LIBRARY_PATH=<outdir>/lib sudo <outdir>/bin/spdk_nvme_perf ...
# or point the profiler at it (it globs for spdk_nvme_perf under deps/).
#
# Scope: spdk_nvme_perf only. The BAR1 microbench needs CUDA + gdrdrv at runtime.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null)"
[[ -z "$REPO_ROOT" ]] && REPO_ROOT="$(cd "$HERE/../.." && pwd)"
REPO_ROOT="$(cd "$REPO_ROOT" && pwd -P)"
OUTDIR="${1:-$REPO_ROOT/spdk-host}"
IMAGE="certus-spdk-build:latest"

BUILD_ARGS=()
if [[ -n "${HTTPS_PROXY:-}" ]]; then
  BUILD_ARGS+=(--build-arg "https_proxy=$HTTPS_PROXY" --build-arg "http_proxy=${HTTP_PROXY:-$HTTPS_PROXY}")
  echo "[spdk-build] proxy: $HTTPS_PROXY"
fi

echo "[spdk-build] building image (SPDK compile — this takes a while) ..."
podman build "${BUILD_ARGS[@]}" -t "$IMAGE" -f "$HERE/Containerfile.spdk" "$HERE"

echo "[spdk-build] locating spdk_nvme_perf inside the image ..."
# The perf example binary path varies by SPDK version; find it rather than guess.
PERF_PATH="$(podman run --rm "$IMAGE" -c '
  for p in /spdk-build/bin/spdk_nvme_perf /spdk/build/examples/perf /spdk/build/bin/spdk_nvme_perf; do
    [[ -x "$p" ]] && { echo "$p"; exit 0; }
  done
  # fall back: search
  find /spdk /spdk-build -type f -name "perf" -path "*nvme*" 2>/dev/null | head -1
  find /spdk /spdk-build -type f -name "spdk_nvme_perf" 2>/dev/null | head -1
' | head -1)"

if [[ -z "$PERF_PATH" ]]; then
  echo "[spdk-build] ERROR: could not find spdk_nvme_perf in the image." >&2
  echo "[spdk-build] inspect with: podman run --rm -it $IMAGE" >&2
  exit 1
fi
echo "[spdk-build] found: $PERF_PATH"

mkdir -p "$OUTDIR/bin" "$OUTDIR/lib"
# Extract the binary and every shared lib it needs (ldd), via a tar stream so we
# follow symlinks and keep real objects.
cid="$(podman create "$IMAGE")"
trap 'podman rm "$cid" >/dev/null 2>&1 || true' EXIT
podman cp "$cid:$PERF_PATH" "$OUTDIR/bin/spdk_nvme_perf"

echo "[spdk-build] extracting shared-lib dependencies ..."
# Resolve ldd inside the container, copy each non-vdso lib out.
LIBS="$(podman run --rm "$IMAGE" -c "ldd '$PERF_PATH' 2>/dev/null | awk '/=>/{print \$3} /^\\t\\// {print \$1}'" | grep -E '^/' | sort -u)"
for lib in $LIBS; do
  podman cp "$cid:$lib" "$OUTDIR/lib/" 2>/dev/null || echo "  (skip $lib)"
done

chmod +x "$OUTDIR/bin/spdk_nvme_perf"
echo "[spdk-build] done."
echo "  binary: $OUTDIR/bin/spdk_nvme_perf"
echo "  libs:   $OUTDIR/lib/ ($(ls "$OUTDIR/lib" | wc -l) files)"
echo
echo "Test it on the host:"
echo "  LD_LIBRARY_PATH='$OUTDIR/lib' sudo '$OUTDIR/bin/spdk_nvme_perf' --help | head"
