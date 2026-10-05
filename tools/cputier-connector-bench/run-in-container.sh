#!/bin/bash
# run-in-container.sh — run bench_cputier_lifecycle.py inside a vLLM image.
#
# The benchmark imports vLLM's native kv_offload stack (TieringOffloadingSpec,
# CPUOffloadingWorker), which is not installed on the host. This wrapper runs
# it in a stock vllm/vllm-openai image with the repo bind-mounted at /certus,
# so no image build is needed. All arguments are passed to the benchmark.
#
#   ./run-in-container.sh                                   # default 4 phases
#   ./run-in-container.sh --mode all --csv bench-results/cputier.csv --tag cputier
#   ./run-in-container.sh --spec cpu                        # CPU-only (no disk tier)
#   FS_TIER_HOST=/mnt/nvme/kv-fs-tier ./run-in-container.sh # disk tier on a given fs
#   CPU_BYTES=13G ./run-in-container.sh --cpu-bytes 13G     # bigger CPU tier (shm auto-sized)
#   IMAGE=docker.io/vllm/vllm-openai:v0.31.0 ./run-in-container.sh
#
# Environment:
#   IMAGE          vLLM image (>= 0.30; the 0.26 certus-offload-fix026 image has
#                  an older kv_offload API and is not supported)
#   GPU            CDI GPU index passed to --device nvidia.com/gpu=<GPU>
#   FS_TIER_HOST   host directory backing the fs disk tier, mounted at /fs-tier.
#                  The benchmark writes into a per-run subdirectory and removes
#                  it at exit (pass --keep-fs to keep it). Must NOT be on the
#                  container's overlay storage: overlayfs rejects O_DIRECT.
#   CPU_BYTES      CPU tier size used to size the container's /dev/shm (pass
#                  the same value to --cpu-bytes). Default 4G.
#   SHM_SIZE       explicit /dev/shm size (default CPU_BYTES + 4G)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd -P)"

IMAGE="${IMAGE:-docker.io/vllm/vllm-openai:v0.30.0}"
GPU="${GPU:-0}"
FS_TIER_HOST="${FS_TIER_HOST:-${HOME}/cputier-bench-fs-tier}"

to_bytes() {
  local v="$1"
  case "${v: -1}" in
    K|k) echo $(( ${v%?} << 10 )) ;;
    M|m) echo $(( ${v%?} << 20 )) ;;
    G|g) echo $(( ${v%?} << 30 )) ;;
    T|t) echo $(( ${v%?} << 40 )) ;;
    *)   echo "$v" ;;
  esac
}
CPU_BYTES="$(to_bytes "${CPU_BYTES:-4G}")"
SHM_SIZE="${SHM_SIZE:-$(( CPU_BYTES + (4 << 30) ))}"

if ! command podman image exists "$IMAGE"; then
  echo "error: image '$IMAGE' not found in the default podman store (podman pull $IMAGE)." >&2
  exit 1
fi

mkdir -p "$FS_TIER_HOST"

ARGS=("$@")
has_fs_root=0
for a in "${ARGS[@]}"; do
  [[ "$a" == --fs-root || "$a" == --fs-root=* ]] && has_fs_root=1
done
(( has_fs_root )) || ARGS+=(--fs-root /fs-tier)

echo "[cputier-bench] ${IMAGE}  gpu=${GPU}  shm-size=${SHM_SIZE}  fs-tier=${FS_TIER_HOST} -> /fs-tier"

# label=disable: the repo and fs-tier dirs are bind-mounted without relabeling
# them for SELinux (a :z relabel would rewrite contexts across the whole repo).
exec command podman run --rm --pull=never \
  --security-opt label=disable \
  --shm-size "${SHM_SIZE}" \
  --device "nvidia.com/gpu=${GPU}" \
  -v "${REPO_ROOT}:/certus" \
  -v "${FS_TIER_HOST}:/fs-tier" \
  -w /certus \
  --entrypoint python3 \
  "$IMAGE" \
  /certus/tools/cputier-connector-bench/bench_cputier_lifecycle.py "${ARGS[@]}"
