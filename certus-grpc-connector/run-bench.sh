#!/bin/bash
# run-bench.sh — launch the certus-grpc-connector workload container against a
# separately-running certus-server.
#
# Thin wrapper over `podman run` that wires the flags this workload needs:
#   * GPU passthrough  (--device nvidia.com/gpu=$GPU via CDI)
#   * --ipc=host       (the host certus-server must open the CUDA IPC handles
#                       this container's vLLM process exports — requires a
#                       shared IPC namespace)
#   * HF cache mount   (avoid re-downloading the model each run)
#   * CERTUS_SERVER    (where the host server is listening)
#
# Preflight-checks the GPU prerequisite and points at setup-host.sh if missing,
# so a missing toolkit is a clear message rather than a cryptic libcuda error.
#
# Usage:
#   ./run-bench.sh
#   GPU=0 NUM_CONVS=450 ./run-bench.sh
#
# Env (all optional; defaults shown):
#   IMAGE=certus-grpc-bench     container image tag
#   GPU=all                     GPU selector (all | 0 | 0,1 | <uuid>)
#   CERTUS_SERVER=host.containers.internal:50051  (host-gateway; NOT localhost —
#                               that is the container's own loopback)
#   NUM_CONVS=450  MODEL=NousResearch/Meta-Llama-3-8B  SLAB_SIZE_BYTES=2097152
#   HF_CACHE=$HOME/.cache/huggingface
#   HF_TOKEN=<token>            passed through if set
#   PODMAN_STORE / PODMAN_RUNROOT   override rootless storage location (this
#                               host builds into /mnt/certus1 — see below)
set -euo pipefail

# Fully-qualified so rootless podman doesn't hit short-name resolution (which
# can't prompt without a TTY). Override IMAGE to point elsewhere.
IMAGE="${IMAGE:-localhost/certus-grpc-bench}"
GPU="${GPU:-all}"
# Default to podman's host-gateway name, NOT localhost: inside the container
# "localhost" is the container's own loopback, so the host-side certus-server is
# unreachable there. host.containers.internal resolves to the host from within a
# rootless container (server must listen on 0.0.0.0, which it does). Override
# with an explicit IP if this name doesn't resolve on an older podman.
CERTUS_SERVER="${CERTUS_SERVER:-host.containers.internal:50051}"
# Optional podman network mode. Empty (default) = rootless bridge, reach the host
# via host.containers.internal. Set PODMAN_NETWORK=host to share the host net
# namespace so the client can dial localhost:50051 over loopback (no userspace
# proxy). Pair with CERTUS_SERVER=localhost:50051.
PODMAN_NETWORK="${PODMAN_NETWORK:-}"
NUM_CONVS="${NUM_CONVS:-450}"
MODEL="${MODEL:-NousResearch/Meta-Llama-3-8B}"
SLAB_SIZE_BYTES="${SLAB_SIZE_BYTES:-2097152}"
TENSOR_PARALLEL_SIZE="${TENSOR_PARALLEL_SIZE:-1}"
HF_CACHE="${HF_CACHE:-$HOME/.cache/huggingface}"

# PROM_PORT — if set, publish the container's Prometheus exporter on that host
# port and tell the workload to start it (needs LOG_STATS=1 so vLLM actually
# registers metrics). Unset by default: no port published, behaviour unchanged.
PROM_PORT="${PROM_PORT:-}"
# LOG_STATS — forwarded to the workload (was previously NOT passed through, so
# `LOG_STATS=1 ./run-bench.sh` had no effect inside the container).
LOG_STATS="${LOG_STATS:-}"
# WORKLOAD_SRC — optional host path to run_multiturn_grpc_certus.py, bind-mounted
# over the copy baked into the image. Lets a change to the workload (e.g. the
# Prometheus exporter block) take effect WITHOUT rebuilding the image.
WORKLOAD_SRC="${WORKLOAD_SRC:-}"

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

         sudo ./certus-grpc-connector/setup-host.sh
EOF
    exit 1
fi

if ! podman image exists "${IMAGE}"; then
    cat >&2 <<EOF
error: image '${IMAGE}' not found in the podman store
       (${PODMAN_STORE}).
       Build it first:

         podman ${store_flags[*]} build \\
           -f certus-grpc-connector/Dockerfile -t ${IMAGE} .
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
    cache_mount+=(-v "${HF_CACHE}:/root/.cache/huggingface")
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
        workload_mount+=(-v "${WORKLOAD_SRC}:/workspace/certus-grpc-connector/run_multiturn_grpc_certus.py:z,ro")
        echo "[run-bench] workload override: ${WORKLOAD_SRC}" >&2
    else
        echo "warning: WORKLOAD_SRC=${WORKLOAD_SRC} not found — using image's baked workload." >&2
    fi
fi

echo "[run-bench] image=${IMAGE} gpu=${GPU} server=${CERTUS_SERVER}"
echo "[run-bench] num_convs=${NUM_CONVS} model=${MODEL} tensor_parallel_size=${TENSOR_PARALLEL_SIZE}"

# NOTE: `exec` bypasses the podman() shell function, so the store flags must be
# passed explicitly here — otherwise `run` hits the DEFAULT store (where the
# image isn't) and fails with "image not known" even though the preflight (which
# goes through the function) found it in the custom store.

net_flags=()
[[ -n "$PODMAN_NETWORK" ]] && net_flags+=(--network="$PODMAN_NETWORK")

exec command podman "${store_flags[@]}" run --rm \
    --pull=never \
    "${net_flags[@]}" \
    --device "nvidia.com/gpu=${GPU}" \
    --ipc=host \
    "${prom_flags[@]}" \
    "${logstats_env[@]}" \
    "${workload_mount[@]}" \
    "${cache_mount[@]}" \
    "${hf_env[@]}" \
    -e "CERTUS_SERVER=${CERTUS_SERVER}" \
    -e "NUM_CONVS=${NUM_CONVS}" \
    -e "MODEL=${MODEL}" \
    -e "TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE}"\
    -e "SLAB_SIZE_BYTES=${SLAB_SIZE_BYTES}" \
    "${IMAGE}"
