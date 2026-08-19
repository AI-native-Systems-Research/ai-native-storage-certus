#!/bin/bash
# run-bench.sh — launch the certus-shmq-connector workload container against a
# separately-running certus-server.
#
# Thin wrapper over `podman run` that wires the flags this workload needs:
#   * GPU passthrough  (--device nvidia.com/gpu=$GPU via CDI)
#   * --ipc=host       — does DOUBLE duty for the shmq connector:
#                        (1) the host server must open the CUDA IPC handles this
#                            container's vLLM process exports, and
#                        (2) it shares the host /dev/shm, so the container sees
#                            the server's mailbox file at SHM_PATH. There is no
#                            network transport, so there is no server address to
#                            pass — the shared /dev/shm path IS the endpoint.
#   * HF cache mount   (avoid re-downloading the model each run)
#   * SHM_PATH         (the mailbox file the host server created; a /dev/shm path)
#
# Preflight-checks the GPU prerequisite and points at setup-host.sh if missing,
# so a missing toolkit is a clear message rather than a cryptic libcuda error.
#
# Usage:
#   ./run-bench.sh
#   GPU=0 NUM_CONVS=450 ./run-bench.sh
#
# Env (all optional; defaults shown):
#   IMAGE=certus-shmq-bench     container image tag
#   GPU=all                     GPU selector (all | 0 | 0,1 | <uuid>)
#   SHM_PATH=/dev/shm/certus-shmq  mailbox file (shared into the container via
#                               --ipc=host; must match the server's --shm-path)
#   NUM_CONVS=450  MODEL=ibm-granite/granite-4.1-8b  SLAB_SIZE_BYTES=2097152
#   HF_CACHE=$HOME/.cache/huggingface
#   HF_TOKEN=<token>            passed through if set
#   PODMAN_STORE / PODMAN_RUNROOT   override rootless storage location (this
#                               host builds into /mnt/certus1 — see below)
set -euo pipefail

# Fully-qualified so rootless podman doesn't hit short-name resolution (which
# can't prompt without a TTY). Override IMAGE to point elsewhere.
IMAGE="${IMAGE:-localhost/certus-shmq-bench}"
GPU="${GPU:-all}"
# The mailbox file. Under --ipc=host the host /dev/shm is shared into the
# container, so the SAME path is valid on both sides — no host-gateway address
# like the gRPC connector needed. Must match the server's --shm-path.
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
NUM_CONVS="${NUM_CONVS:-450}"
MAX_ROUNDS="${MAX_ROUNDS:-0}"   # 0 = replay all turns; N caps at N rounds/turns
MODEL="${MODEL:-ibm-granite/granite-4.1-8b}"
SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"
TENSOR_PARALLEL_SIZE="${TENSOR_PARALLEL_SIZE:-1}"
HF_CACHE="${HF_CACHE:-$HOME/.cache/huggingface}"

# PROM_PORT — if set, publish the container's Prometheus exporter on that host
# port and tell the workload to start it (needs LOG_STATS=1 so vLLM actually
# registers metrics). Unset by default: no port published, behaviour unchanged.
PROM_PORT="${PROM_PORT:-}"
# LOG_STATS — forwarded to the workload.
LOG_STATS="${LOG_STATS:-}"
# WORKLOAD_SRC — optional host path to run_multiturn_shmq_certus.py, bind-mounted
# over the copy baked into the image. Lets a change to the workload take effect
# WITHOUT rebuilding the image.
WORKLOAD_SRC="${WORKLOAD_SRC:-}"
# CONNECTOR_SRC — optional host path to the certus_shmq_connector package dir,
# bind-mounted over the copy installed in the image (runs local edits without a
# rebuild). Unset by default (uses the image's baked connector); declared here so
# the `-n` test at the mount block below is safe under `set -u`.
CONNECTOR_SRC="${CONNECTOR_SRC:-}"

# This host keeps the (large) image on the /mnt/certus1 filesystem, so podman
# needs explicit store paths. Override or unset for a default install.
PODMAN_STORE="${PODMAN_STORE:-/mnt/certus1/podman/storage}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-/mnt/certus1/podman/run}"
store_flags=()
[[ -n "${PODMAN_STORE}" ]] && store_flags+=(--root "${PODMAN_STORE}")
[[ -n "${PODMAN_RUNROOT}" ]] && store_flags+=(--runroot "${PODMAN_RUNROOT}")

podman() { command podman "${store_flags[@]}" "$@"; }

# ── Preflight: GPU prerequisite ──
if [[ ! -f /etc/cdi/nvidia.yaml ]]; then
    cat >&2 <<EOF
error: no CDI spec at /etc/cdi/nvidia.yaml — the NVIDIA container runtime is not
       set up, so podman cannot pass a GPU into the container (vLLM needs one).
       Run the one-time host setup first:

         sudo ./certus-shmq-connector/setup-host.sh
EOF
    exit 1
fi

# ── Preflight: mailbox present on the host ──
# The server creates SHM_PATH; if it isn't there, the container's entrypoint
# would just spin until WAIT_SECS. Fail fast with a clear message instead.
if [[ ! -e "${SHM_PATH}" ]]; then
    cat >&2 <<EOF
error: mailbox file '${SHM_PATH}' not present on the host — is certus-server
       running with --shm-path ${SHM_PATH}? Start it first:

         target/release/certus-server --device-pci <bdf> \\
           --memory-tier-size <N>G --shm-path ${SHM_PATH} --format
EOF
    exit 1
fi

if ! podman image exists "${IMAGE}"; then
    cat >&2 <<EOF
error: image '${IMAGE}' not found in the podman store
       (${PODMAN_STORE}).
       Build it first:

         podman ${store_flags[*]} build \\
           -f certus-shmq-connector/Dockerfile -t ${IMAGE} .
EOF
    exit 1
fi

# Resolve the tag to an image ID and run by ID. With a custom --root store,
# rootless podman can spuriously report "image not known" at `run` time for a
# tagged name that `image exists` accepts (name-resolution races against
# <none> dangling images). Running by ID sidesteps name resolution entirely.
IMAGE_ID="$(podman image inspect "${IMAGE}" --format '{{.Id}}' 2>/dev/null)"
[[ -n "${IMAGE_ID}" ]] && IMAGE="${IMAGE_ID}"

# ── HF token passthrough (only if set) ──
hf_env=()
[[ -n "${HF_TOKEN:-}" ]] && hf_env+=(-e "HF_TOKEN=${HF_TOKEN}")

# ── HF cache mount (only if the dir exists) ──
cache_mount=()
if [[ -d "${HF_CACHE}" ]]; then
    # :z lets rootless podman relabel the cache to a shared container SELinux
    # context. Without it, a cache on a freshly-formatted/relabeled fs is
    # unlabeled_t and the container is denied (EPERM statting CACHEDIR.TAG).
    cache_mount+=(-v "${HF_CACHE}:/root/.cache/huggingface:z")
else
    echo "warning: HF cache dir ${HF_CACHE} missing — model will download fresh." >&2
fi

# ── Prometheus exporter passthrough (only if PROM_PORT set) ──
prom_flags=()
if [[ -n "${PROM_PORT}" ]]; then
    prom_flags+=(-p "${PROM_PORT}:${PROM_PORT}" -e "PROM_PORT=${PROM_PORT}")
    echo "[run-bench] prometheus exporter -> http://<host>:${PROM_PORT}/metrics" >&2
fi

# ── LOG_STATS passthrough (only if set) ──
logstats_env=()
[[ -n "${LOG_STATS}" ]] && logstats_env+=(-e "LOG_STATS=${LOG_STATS}")

# ── Workload override mount (only if WORKLOAD_SRC set) ──
# The image's ENV WORKLOAD points at this in-container path; mounting over it
# swaps in a host copy without a rebuild. :z relabels for rootless SELinux, ro
# keeps it read-only.
workload_mount=()
if [[ -n "${WORKLOAD_SRC}" ]]; then
    if [[ -f "${WORKLOAD_SRC}" ]]; then
        workload_mount+=(-v "${WORKLOAD_SRC}:/workspace/certus-shmq-connector/run_multiturn_shmq_certus.py:z,ro")
        echo "[run-bench] workload override: ${WORKLOAD_SRC}" >&2
    else
        echo "warning: WORKLOAD_SRC=${WORKLOAD_SRC} not found — using image's baked workload." >&2
    fi
fi

# ── Connector package override mount (only if CONNECTOR_SRC set) ──
# The image installs the connector as `pip install -e /workspace/certus-shmq-connector`,
# so the importable package resolves to .../certus_shmq_connector. Mounting a host
# copy of that dir over it runs local edits (e.g. manager.py changes) without an
# image rebuild — same :z,ro rationale as the workload mount above.
connector_mount=()
if [[ -n "${CONNECTOR_SRC}" ]]; then
    if [[ -d "${CONNECTOR_SRC}" ]]; then
        connector_mount+=(-v "${CONNECTOR_SRC}:/workspace/certus-shmq-connector/certus_shmq_connector:z,ro")
        echo "[run-bench] connector override: ${CONNECTOR_SRC}" >&2
    else
        echo "warning: CONNECTOR_SRC=${CONNECTOR_SRC} not found — using image's baked connector." >&2
    fi
fi

echo "[run-bench] image=${IMAGE} gpu=${GPU} shm_path=${SHM_PATH}"
echo "[run-bench] num_convs=${NUM_CONVS} model=${MODEL} tensor_parallel_size=${TENSOR_PARALLEL_SIZE}"

# NOTE: `exec` bypasses the podman() shell function, so the store flags must be
# passed explicitly here — otherwise `run` hits the DEFAULT store (where the
# image isn't) and fails with "image not known" even though the preflight (which
# goes through the function) found it in the custom store.

exec command podman "${store_flags[@]}" run --rm \
    --pull=never \
    --device "nvidia.com/gpu=${GPU}" \
    --ipc=host \
    "${prom_flags[@]}" \
    "${logstats_env[@]}" \
    "${workload_mount[@]}" \
    "${connector_mount[@]}" \
    "${cache_mount[@]}" \
    "${hf_env[@]}" \
    -e "SHM_PATH=${SHM_PATH}" \
    -e "NUM_CONVS=${NUM_CONVS}" \
    -e "MAX_ROUNDS=${MAX_ROUNDS}" \
    -e "MODEL=${MODEL}" \
    -e "TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE}"\
    -e "SLAB_SIZE_BYTES=${SLAB_SIZE_BYTES}" \
    -e "ENFORCE_EAGER=${ENFORCE_EAGER:-0}" \
    -e "WORKLOAD_MODE=${WORKLOAD_MODE:-batched}" \
    -e "DTYPE=${DTYPE:-float16}" \
    "${IMAGE}"
