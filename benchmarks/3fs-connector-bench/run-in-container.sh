#!/bin/bash
# run-in-container.sh — run bench_3fs_lifecycle.py inside a vLLM image.
#
# The benchmark imports vLLM's HF3FSKVConnector, which is not installed on the
# host. This wrapper runs it with the repo bind-mounted at /certus and the 3FS
# storage directory bound at /hf3fs. All arguments are passed to the benchmark.
#
#   ./run-in-container.sh                                   # default phases
#   ./run-in-container.sh --verify                          # data round trip first
#   ./run-in-container.sh --mode all --csv bench-results/3fs.csv --tag 3fs
#   MOCK=1 H3FS_DIR=/raid0/h3fs-mock ./run-in-container.sh  # code-path test, NOT 3FS
#
# Real 3FS needs the vLLM + 3FS USRBIO image (benchmarks/vllm-serve/build_h3fs_container.sh)
# and a live FUSE mount (deploy/3fs/run.sh, as root). The mount ROOT is bound in,
# because USRBIO locates the mount through <root>/3fs-virt, and the connector's
# data file goes in <root>/vllm, which deploy/3fs creates for you.
#
# Environment:
#   IMAGE      vLLM image [localhost/vllm-hf3fs-bench:vllm0.30.0; with MOCK=1
#              docker.io/vllm/vllm-openai:v0.30.0]
#   GPU        CDI GPU index [0]
#   H3FS_MOUNT host 3FS FUSE mountpoint [/mnt/3fs]
#   MOCK       1 = run on vLLM's file-backed mock client in the stock image, with
#              the data file under H3FS_DIR. It exercises the code path only:
#              results are not 3FS and every row is tagged hf3fs_client=mock [0]
#   H3FS_DIR   (MOCK=1) host directory for the mock data file; a per-run
#              subdirectory is created and removed at exit (KEEP_DATA=1 keeps it)
#              [/raid0/h3fs-mock]
#   CPUS       optional CPU list (e.g. 0-15,32-47) to pin the bench to, applied
#              with taskset inside the container: rootless podman has no cpuset
#              controller, and host affinity is not inherited
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd -P)"

MOCK="${MOCK:-0}"
GPU="${GPU:-0}"
H3FS_MOUNT="${H3FS_MOUNT:-/mnt/3fs}"
H3FS_DIR="${H3FS_DIR:-/raid0/h3fs-mock}"
if [[ "$MOCK" == "1" ]]; then
  IMAGE="${IMAGE:-docker.io/vllm/vllm-openai:v0.30.0}"
else
  IMAGE="${IMAGE:-localhost/vllm-hf3fs-bench:vllm0.30.0}"
fi

if ! command podman image exists "$IMAGE"; then
  echo "error: image '$IMAGE' not found in the default podman store." >&2
  [[ "$MOCK" == "1" ]] || echo "       build it: benchmarks/vllm-serve/build_h3fs_container.sh 0.30.0 (or MOCK=1 to test the code path)" >&2
  exit 1
fi

MOUNTS=() BENCH=()
RUN_DIR=""
if [[ "$MOCK" == "1" ]]; then
  mkdir -p "$H3FS_DIR"
  RUN_DIR="$(mktemp -d "${H3FS_DIR}/run-XXXXXX")"
  MOUNTS=(-v "${RUN_DIR}:/hf3fs")
  BENCH=(--storage-path /hf3fs)
  # The stock image has no USRBIO client, so the connector uses its mock. Make
  # sure a mock run is never mistaken for a 3FS run downstream.
  trap '[[ "${KEEP_DATA:-0}" == 1 ]] && echo "[3fs-bench] mock data kept in $RUN_DIR" || rm -rf --one-file-system "$RUN_DIR"' EXIT
else
  mountpoint -q "$H3FS_MOUNT" 2>/dev/null || {
    echo "error: $H3FS_MOUNT is not a mountpoint — start 3FS first (sudo deploy/3fs/run.sh)," >&2
    echo "       or MOCK=1 to test the code path without 3FS." >&2
    exit 1
  }
  [[ -d "$H3FS_MOUNT/vllm" && -w "$H3FS_MOUNT/vllm" ]] || {
    echo "error: $H3FS_MOUNT/vllm is missing or not writable (deploy/3fs creates it)" >&2
    exit 1
  }
  # The data file persists on 3FS across runs, but its page index lives in the
  # bench's private metadata server, so every run starts empty. Put each run in
  # its own file anyway, so a crashed run cannot leave pages another reads.
  RUN_DIR="$H3FS_MOUNT/vllm/bench-$$"
  mkdir -p "$RUN_DIR"
  MOUNTS=(-v "${H3FS_MOUNT}:/vllm-workspace/mnt/hf3fs")
  BENCH=(--storage-path "/vllm-workspace/mnt/hf3fs/vllm/bench-$$" --require-usrbio)
  trap 'rm -rf "$RUN_DIR"' EXIT
fi

ENTRY=(python3)
[[ -n "${CPUS:-}" ]] && ENTRY=(taskset -c "$CPUS" python3)
echo "[3fs-bench] ${IMAGE}  gpu=${GPU}  $([[ "$MOCK" == 1 ]] && echo "MOCK client, data ${RUN_DIR}" || echo "3FS ${H3FS_MOUNT}")"

# --ipc host: USRBIO hands I/O buffers to hf3fs_fuse_main through /dev/shm.
# label=disable: the repo and 3FS mount are bound without SELinux relabeling
# (relabeling a FUSE mount fails, and :z would rewrite contexts across the repo).
set +e
command podman run --rm --pull=never \
  --security-opt label=disable \
  --ipc host \
  --device "nvidia.com/gpu=${GPU}" \
  -v "${REPO_ROOT}:/certus" \
  "${MOUNTS[@]}" \
  -w /certus \
  -e VLLM_LOGGING_LEVEL="${VLLM_LOGGING_LEVEL:-WARNING}" \
  --entrypoint "${ENTRY[0]}" \
  "$IMAGE" \
  "${ENTRY[@]:1}" \
  /certus/benchmarks/3fs-connector-bench/bench_3fs_lifecycle.py "${BENCH[@]}" "$@"
rc=$?
set -e
exit $rc
