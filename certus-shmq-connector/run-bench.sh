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

# Repo root, resolved from this script's own location (certus-shmq-connector/..),
# so the full-corpus mount below works whether launched directly or by the
# orchestrator, without an extra env.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Fully-qualified so rootless podman doesn't hit short-name resolution (which
# can't prompt without a TTY). Override IMAGE to point elsewhere.
IMAGE="${IMAGE:-localhost/certus-shmq-bench}"
GPU="${GPU:-all}"
# The mailbox file. Under --ipc=host the host /dev/shm is shared into the
# container, so the SAME path is valid on both sides — no host-gateway address
# like the gRPC connector needed. Must match the server's --shm-path.
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
NUM_CONVS="${NUM_CONVS:-}"   # empty = default by turn config (450 only for 12/12, whole corpus otherwise)
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
# TRACE_OFFLOAD — when set (non-"0"), the driver wraps the connector in the
# generic TracingConnector, which writes offloading_trace_<pid>.jsonl. The
# container is --rm, so the trace dir must be a host mount: TRACE_OUT (host dir,
# default ./shmq-trace) is bind-mounted in and TRACE_DIR points the tracer at it.
TRACE_OFFLOAD="${TRACE_OFFLOAD:-}"
TRACE_OUT="${TRACE_OUT:-${SCRIPT_DIR}/shmq-trace}"
# WORKLOAD_SRC — optional host path to run_multiturn_shmq_certus.py, bind-mounted
# over the copy baked into the image. Lets a change to the workload take effect
# WITHOUT rebuilding the image.
WORKLOAD_SRC="${WORKLOAD_SRC:-}"
# CONNECTOR_SRC — optional host path to the certus_shmq_connector package dir,
# bind-mounted over the copy installed in the image (runs local edits without a
# rebuild). Unset by default (uses the image's baked connector); declared here so
# the `-n` test at the mount block below is safe under `set -u`.
CONNECTOR_SRC="${CONNECTOR_SRC:-}"
# WORKLOAD_NAME — optional named dataset workload forwarded to the driver as the
# WORKLOAD_NAME env (see run_multiturn_common.resolve_workload). Empty = the
# image's baked DATASET_PATH (the 450x12 set). SHAREGPT_MIN_TURNS/MAX_TURNS pick
# the sharegpt config: 12/12 is exactly the baked DATASET_PATH (a no-op);
# min-turns 2 = the full corpus, mounted from the host (see the passthrough
# block below) since it is not baked into the image (1 = legacy alias for 2).
WORKLOAD_NAME="${WORKLOAD_NAME:-}"
SHAREGPT_MIN_TURNS="${SHAREGPT_MIN_TURNS:-}"
SHAREGPT_MAX_TURNS="${SHAREGPT_MAX_TURNS:-}"
# long-doc-qa (WORKLOAD_NAME=long-doc-qa) shape knobs, forwarded to the driver.
# Empty = the workload's baked defaults; only read when WORKLOAD_NAME=long-doc-qa.
LONGDOC_DOC_TOKENS="${LONGDOC_DOC_TOKENS:-}"
LONGDOC_QUESTIONS="${LONGDOC_QUESTIONS:-}"
LONGDOC_NUM_DOCS="${LONGDOC_NUM_DOCS:-}"
LONGDOC_SEED="${LONGDOC_SEED:-}"
# The turn bounds only apply to the sharegpt workload, so setting either without
# WORKLOAD_NAME implies it — otherwise the passthrough block below adds nothing
# and the container uses its baked 450x12 DATASET_PATH.
if [[ -z "${WORKLOAD_NAME}" && ( -n "${SHAREGPT_MIN_TURNS}" || -n "${SHAREGPT_MAX_TURNS}" ) ]]; then
    WORKLOAD_NAME="sharegpt"
fi
# Only 12/12 (450x12 subset) and 2/2 (full corpus; the loader's own >=2-turn
# floor, so 1 is a legacy alias for 2) are prepared; reject anything else so the
# corpus mount below can't pair min-turns 1|2 with a bogus max and force an
# unvalidated DATASET_PATH. max-turns mirrors min-turns when unset.
if [[ "${WORKLOAD_NAME}" == "sharegpt" ]]; then
    _mn="${SHAREGPT_MIN_TURNS:-12}"; _mx="${SHAREGPT_MAX_TURNS:-$_mn}"
    if ! { [[ "${_mn}" == "12" && "${_mx}" == "12" ]] || \
           { [[ "${_mn}" == "1" || "${_mn}" == "2" ]] && [[ "${_mx}" == "${_mn}" ]]; }; }; then
        echo "error: sharegpt workload accepts only 12/12 (the 450-conv subset) or 2/2" >&2
        echo "       (the full corpus; 1 also accepted); got min=${_mn} max=${_mx}. Use DATASET_PATH otherwise." >&2
        exit 2
    fi
fi

# Default the conversation count from the turn config (mirrors
# run_multiturn_common._sharegpt_num_convs): 450 ONLY for the exactly-12/12
# subset, the whole corpus for every other turn config. Without this the
# hardcoded 450 default was forwarded as NUM_CONVS and — being the final override
# in resolve_workload — masked the corpus default, so min-turns 2 still ran 450
# convs. An explicit NUM_CONVS in the environment always wins.
if [[ -z "${NUM_CONVS}" ]]; then
    if [[ "${WORKLOAD_NAME}" == "long-doc-qa" ]]; then
        NUM_CONVS="${LONGDOC_NUM_DOCS:-1000}"   # whole generated corpus; load_convs caps here
    elif [[ "${SHAREGPT_MIN_TURNS:-12}" == "12" && "${SHAREGPT_MAX_TURNS:-${SHAREGPT_MIN_TURNS:-12}}" == "12" ]]; then
        NUM_CONVS=450     # exactly-12/12 subset
    else
        NUM_CONVS=94145   # everything else -> whole corpus (= _SHAREGPT_CORPUS_CONVS; load_convs caps here)
    fi
fi

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

# ── Offload-trace passthrough (only if TRACE_OFFLOAD set) ──
# The container runs --rm, so its offloading_trace_<pid>.jsonl would vanish on
# exit. Bind-mount a host dir (TRACE_OUT) at a fixed container path and point the
# tracer there via TRACE_DIR so the JSONL survives the run. :z relabels for
# rootless SELinux; writable (no ,ro) because the tracer creates files in it.
trace_flags=()
if [[ -n "${TRACE_OFFLOAD}" && "${TRACE_OFFLOAD}" != "0" ]]; then
    mkdir -p "${TRACE_OUT}"
    trace_flags+=(-v "${TRACE_OUT}:/workspace/trace:z"
                  -e "TRACE_OFFLOAD=${TRACE_OFFLOAD}"
                  -e "TRACE_DIR=/workspace/trace")
    echo "[run-bench] TRACE_OFFLOAD=${TRACE_OFFLOAD}: offload traces -> ${TRACE_OUT}" >&2
fi

# ── Named-workload passthrough (only if WORKLOAD_NAME set) ──
# Forward the selector plus any human-turn bounds. 12/12 is exactly the image's
# baked DATASET_PATH, so at 12/12 this is a no-op. min-turns 2 selects the FULL
# corpus, which is NOT baked: bind-mount data/sharegpt read-only and point
# DATASET_PATH at the mount (DATASET_PATH always wins in resolve_workload).
# (1 = legacy alias for 2.) Empty WORKLOAD_NAME => nothing added (baked default).
workload_name_env=()
if [[ -n "${WORKLOAD_NAME}" ]]; then
    workload_name_env+=(-e "WORKLOAD_NAME=${WORKLOAD_NAME}")
    [[ -n "${SHAREGPT_MIN_TURNS}" ]] && workload_name_env+=(-e "SHAREGPT_MIN_TURNS=${SHAREGPT_MIN_TURNS}")
    [[ -n "${SHAREGPT_MAX_TURNS}" ]] && workload_name_env+=(-e "SHAREGPT_MAX_TURNS=${SHAREGPT_MAX_TURNS}")
    echo "[run-bench] workload=${WORKLOAD_NAME} min_turns=${SHAREGPT_MIN_TURNS:-12} max_turns=${SHAREGPT_MAX_TURNS:-12}" >&2
    if [[ "${WORKLOAD_NAME}" == "sharegpt" && ( "${SHAREGPT_MIN_TURNS}" == "2" || "${SHAREGPT_MIN_TURNS}" == "1" ) ]]; then
        corpus="${REPO_ROOT}/data/sharegpt"
        if [[ -d "${corpus}" ]]; then
            workload_name_env+=(-v "${corpus}:/workspace/data/sharegpt:z,ro"
                                -e "DATASET_PATH=/workspace/data/sharegpt")
            echo "[run-bench] min-turns ${SHAREGPT_MIN_TURNS}: mounting full corpus ${corpus}" >&2
        else
            echo "warning: ${corpus} not found — min-turns ${SHAREGPT_MIN_TURNS} needs the full ShareGPT corpus (data/sharegpt/*.json); falling back to the baked 450x12 set." >&2
        fi
    elif [[ "${WORKLOAD_NAME}" != "sharegpt" ]]; then
        # A self-generating workload (e.g. long-doc-qa builds its own dataset in
        # the container). The image bakes DATASET_PATH=<450x12 sharegpt>, and an
        # explicit DATASET_PATH WINS over WORKLOAD_NAME in resolve_workload — so
        # blank it, else the named workload is silently ignored and the baked
        # 450-conv ShareGPT set runs instead. An empty value reads as unset there.
        workload_name_env+=(-e "DATASET_PATH=")
        echo "[run-bench] workload=${WORKLOAD_NAME}: clearing baked DATASET_PATH" >&2
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
    "${trace_flags[@]}" \
    "${workload_name_env[@]}" \
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
    -e "LONGDOC_DOC_TOKENS=${LONGDOC_DOC_TOKENS}" \
    -e "LONGDOC_QUESTIONS=${LONGDOC_QUESTIONS}" \
    -e "LONGDOC_NUM_DOCS=${LONGDOC_NUM_DOCS}" \
    -e "LONGDOC_SEED=${LONGDOC_SEED}" \
    -e "DTYPE=${DTYPE:-float16}" \
    "${IMAGE}"
