#!/bin/bash
# run-in-container.sh — start a private LMCache MP server, run
# bench_lmcache_lifecycle.py against it in the vLLM + LMCache image, tear the
# server down. All arguments are passed to the benchmark.
#
#   ./run-in-container.sh                                   # default 4 phases
#   ./run-in-container.sh --verify                          # data round trip first
#   ./run-in-container.sh --mode all --csv bench-results/lmcache.csv --tag lmcache
#   L1_GB=8 L2_DIR=/mnt/nvme/lmcache-bench-l2 ./run-in-container.sh
#   L2=0 ./run-in-container.sh                              # L1 only (no cold phase)
#   CHUNK_SIZE=64 ./run-in-container.sh --block-bytes 512K  # match a 64-token offload block
#
# The server runs on side ports (ZMQ 5575, HTTP 8095), so it does not touch a
# deploy/lmcache stack on 5555/8080. Both containers share the GPU, the host IPC
# and network namespaces, and the host PID namespace: the server imports the
# bench's KV tensors by CUDA IPC, and on this host that fails with "invalid
# device context" unless both sides see the same PIDs (--pid host).
#
# Environment:
#   IMAGE        vLLM + LMCache image (deploy/lmcache/build.sh)
#                [localhost/vllm-lmcache-bench:vllm0.30.0]
#   GPU          CDI GPU index [0]
#   L1_GB        LMCache L1 (pinned RAM) size in GiB [4]  (cputier --cpu-bytes 4G)
#   L2           1 = fs_native L2 adapter on L2_DIR, 0 = none [1]
#   L2_DIR       host dir for L2 files; a per-run subdirectory is created and
#                removed at exit (KEEP_L2=1 keeps it) [$HOME/lmcache-bench-l2]
#   L2_GB        L2 capacity [64]
#   USE_ODIRECT  1 = O_DIRECT L2 I/O [1]; the dir must not be overlay/tmpfs
#   L2_THREADS   fs_native I/O workers [8]
#   CHUNK_SIZE   server --chunk-size in tokens [256]
#   LMC_WORKERS  server --max-workers [1]
#   ZMQ_PORT / HTTP_PORT   server ports [5575 / 8095]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd -P)"

IMAGE="${IMAGE:-localhost/vllm-lmcache-bench:vllm0.30.0}"
GPU="${GPU:-0}"
L1_GB="${L1_GB:-4}"
L2="${L2:-1}"
L2_DIR="${L2_DIR:-${HOME}/lmcache-bench-l2}"
L2_GB="${L2_GB:-64}"
USE_ODIRECT="${USE_ODIRECT:-1}"
L2_THREADS="${L2_THREADS:-8}"
CHUNK_SIZE="${CHUNK_SIZE:-256}"
LMC_WORKERS="${LMC_WORKERS:-1}"
ZMQ_PORT="${ZMQ_PORT:-5575}"
HTTP_PORT="${HTTP_PORT:-8095}"
SRV="lmcache-connector-bench-server-$$"

if ! command podman image exists "$IMAGE"; then
  echo "error: image '$IMAGE' not found — build it: deploy/lmcache/build.sh" >&2
  exit 1
fi
for p in "$ZMQ_PORT" "$HTTP_PORT"; do
  if ss -ltnH "sport = :$p" | grep -q .; then
    echo "error: port $p is in use (another bench server?); set ZMQ_PORT/HTTP_PORT" >&2
    exit 1
  fi
done

COMMON=(--pull=never --device "nvidia.com/gpu=${GPU}" --security-opt label=disable
        --ipc host --network host --pid host --ulimit memlock=-1:-1)

SRV_ARGS=(lmcache-server --host 127.0.0.1 --port "$ZMQ_PORT"
          --http-host 127.0.0.1 --http-port "$HTTP_PORT"
          --chunk-size "$CHUNK_SIZE" --l1-size-gb "$L1_GB"
          --eviction-policy LRU --max-workers "$LMC_WORKERS")
SRV_MOUNT=()
RUN_DIR=""
if [[ "$L2" == "1" ]]; then
  mkdir -p "$L2_DIR"
  RUN_DIR="$(mktemp -d "${L2_DIR}/run-XXXXXX")"
  ODIRECT=$([[ "$USE_ODIRECT" == "1" ]] && echo true || echo false)
  SRV_ARGS+=(--l2-adapter "{\"type\":\"fs_native\",\"base_path\":\"/l2\",\"use_odirect\":${ODIRECT},\"num_workers\":${L2_THREADS},\"max_capacity_gb\":${L2_GB}}")
  SRV_MOUNT=(-v "${RUN_DIR}:/l2" -e L2_DIR=/l2 -e DISK_GB="$L2_GB" -e USE_ODIRECT="$USE_ODIRECT")
else
  # The entrypoint's L2 preflight needs a writable dir even without an adapter.
  SRV_MOUNT=(-e L2_DIR=/tmp/l2 -e DISK_GB=0 -e USE_ODIRECT=0)
fi

cleanup() {
  podman rm -f "$SRV" >/dev/null 2>&1 || true
  if [[ -n "$RUN_DIR" && "${KEEP_L2:-0}" != "1" ]]; then
    rm -rf --one-file-system "$RUN_DIR"
  elif [[ -n "$RUN_DIR" ]]; then
    echo "[lmcache-bench] L2 files kept in $RUN_DIR"
  fi
}
trap cleanup EXIT

echo "[lmcache-bench] ${IMAGE}  gpu=${GPU}  L1=${L1_GB}G  L2=$([[ "$L2" == 1 ]] && echo "${RUN_DIR} ${L2_GB}G" || echo none)  chunk=${CHUNK_SIZE}"
podman run -d --name "$SRV" "${COMMON[@]}" "${SRV_MOUNT[@]}" "$IMAGE" "${SRV_ARGS[@]}" >/dev/null
for _ in $(seq 1 120); do
  curl -sf "http://127.0.0.1:${HTTP_PORT}/status" 2>/dev/null | grep -q '"is_healthy":true' && break
  if [[ "$(podman inspect -f '{{.State.Running}}' "$SRV" 2>/dev/null)" != "true" ]]; then
    echo "[lmcache-bench] server exited during startup:" >&2
    podman logs --tail 40 "$SRV" >&2
    exit 1
  fi
  sleep 1
done
curl -sf "http://127.0.0.1:${HTTP_PORT}/status" | grep -q '"is_healthy":true' \
  || { echo "[lmcache-bench] server not healthy; see podman logs $SRV" >&2; exit 1; }

set +e
podman run --rm "${COMMON[@]}" \
  -v "${REPO_ROOT}:/certus" -w /certus \
  --entrypoint python3 \
  "$IMAGE" \
  /certus/benchmarks/lmcache-connector-bench/bench_lmcache_lifecycle.py \
  --server-url "tcp://127.0.0.1:${ZMQ_PORT}" --http-url "http://127.0.0.1:${HTTP_PORT}" "$@"
rc=$?
set -e
if (( rc != 0 )); then
  echo "[lmcache-bench] bench exited $rc; last server log lines:" >&2
  podman logs --tail 20 "$SRV" 2>&1 | grep -vE "Discovered|slapd" >&2
fi
exit $rc
