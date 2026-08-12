#!/bin/bash
# SharedStorage — llmd_fs_backend on a host filesystem (image certus-sharedstorage-bench).
#   ./run-docker-shared.sh
#   SHARED_FS=/mnt/ss-kv DISK_DEV=md0 ./run-docker-shared.sh
#
# NOTE (vLLM 0.26.0): the sharedstorage image build currently FAILS upstream
# removed vllm.v1.kv_offload.abstract, which llmd_fs_backend imports so the
# certus-sharedstorage-bench tag is still the 0.20 image. Verify with:
#   podman run --rm --entrypoint python3 certus-sharedstorage-bench -c 'import vllm;print(vllm.__version__)'
#
# NOTE (perms): the container runs as a rootless-mapped uid; SHARED_FS and its
# shared-kv/ subdir must be writable by that uid or the run silently degrades to
# no-offload. See tools/configure-bench.sh for the RAID0/XFS + ownership setup.
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-sharedstorage-bench}"
SHARED_FS="${SHARED_FS:-/mnt/ss-kv}"
DRAM="${DRAM:-$((32 * (1 << 30)))}"                 # host-RAM staging pool (bytes)
DISK_DEV="${DISK_DEV:-$(findmnt -no SOURCE --target "$SHARED_FS" 2>/dev/null | xargs -r basename)}"
[[ -z "$DISK_DEV" ]] && DISK_DEV="md0"
LOG="${LOG:-${SCRIPT_DIR}/sharedstorage_$(stamp).log}"

require_image "$IMAGE"
mkdir -p "${SHARED_FS}/shared-kv"
run_container "$LOG" "$IMAGE" \
  -v "${SHARED_FS}:/mnt/fs-backend-bench:z" \
  -e "DRAM=${DRAM}" \
  -e "DISK_DEV=${DISK_DEV}" \
  -e "SKIP_PREFLIGHT=1"
