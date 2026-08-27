#!/bin/bash
# CPU+FS-spill tiered KV-offload — PATCHED arm. SAME as run-docker-cputier.sh,
# but runs certus-offload-bench-fix026: the image built from Dockerfile.offload
# with --build-arg VLLM_FIX_TIERING=1, which BAKES IN the fork tiering fix
# (fix/tiering-deferred-finalize-v0.26.0 @5e20aeb5) over vLLM 0.26.0. Used to
# validate that the fix eliminates the tiering _req_state KeyError /
# EngineDeadError crash at 450 convs.
#
# The fix is baked into the image (see Dockerfile.offload + vllm-fix2/), so
# there is NO runtime bind-mount of the patched sources — the only difference
# from the as-shipped arm is which image tag is run, so any change in
# reliability/throughput is attributable to the patch alone. Build both tags
# with build_026.sh.
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-offload-bench-fix026}"
CPU_BYTES="${CPU_BYTES:-$((8 * (1 << 30)))}"
SHM_BYTES="${SHM_BYTES:-$((CPU_BYTES + 4 * (1 << 30)))}"
DISK_DIR_HOST="${DISK_DIR_HOST:-/mnt/certus1/kv-fs-tier}"
DISK_DIR_CTR="/workspace/kv-fs-tier"
DISK_READ_THREADS="${DISK_READ_THREADS:-16}"
DISK_WRITE_THREADS="${DISK_WRITE_THREADS:-16}"
DRIVER="${DRIVER:-${SCRIPT_DIR}/run_multiturn_offloading.py}"
LOG="${LOG:-${SCRIPT_DIR}/cputier_patched_$(stamp).log}"

require_image "$IMAGE"
[[ -f "$DRIVER" ]] || { echo "error: driver not found at ${DRIVER}" >&2; exit 1; }
mkdir -p "$DISK_DIR_HOST"

# Helper modules the mounted driver imports (baked image ships only the driver).
HELPER_MOUNTS=()
for _h in run_multiturn_common.py run_multiturn_sync_batched.py run_multiturn_async.py; do
  [[ -f "${SCRIPT_DIR}/${_h}" ]] && HELPER_MOUNTS+=( -v "${SCRIPT_DIR}/${_h}:/workspace/bench/${_h}:z" )
done

echo "[patched] running ${IMAGE} (tiering fix baked in; no runtime patch mount)"

run_container "$LOG" "$IMAGE" \
  --shm-size "${SHM_BYTES}" \
  -e "CPU_BYTES=${CPU_BYTES}" \
  -e "DISK_DIR=${DISK_DIR_CTR}" \
  -e "DISK_READ_THREADS=${DISK_READ_THREADS}" \
  -e "DISK_WRITE_THREADS=${DISK_WRITE_THREADS}" \
  -v "${DRIVER}:/workspace/bench/run_multiturn_offloading.py:z" \
  "${HELPER_MOUNTS[@]}" \
  -v "${DISK_DIR_HOST}:${DISK_DIR_CTR}:z"
