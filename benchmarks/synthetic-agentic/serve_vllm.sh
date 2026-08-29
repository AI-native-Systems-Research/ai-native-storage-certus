#!/usr/bin/env bash
# Serve ONE vLLM OpenAI server, parameterised by KV connector.
#
# The thing served is vLLM. The "backend" (NoOffload / CPUOffload / Tiered-CPU-FS
# / Certus-shmq) is NOT a separate server — it is a single --kv-transfer-config
# connector *parameter* passed to that one vLLM server, selected by CONNECTOR.
# This module is therefore workload-agnostic: it knows nothing about what client
# drives the endpoint (inference-perf, long_doc_qa, curl, ...). A caller brings
# the server up, points any HTTP client at http://localhost:${PORT}/v1, and tears
# it down.
#
# This is a focused refactor of the SERVER half of
# ../long-doc-qa/run_bench.sh: it drops that script's long_doc_qa client half and
# adds the cpu / tiered connector arms (whose kv_transfer_config JSON is lifted
# verbatim from ../kv-offload-replay/run_multiturn_offloading.py).
#
# Usage:
#   CONNECTOR=none   ./serve_vllm.sh up      # launch + wait for /v1/models, then block (Ctrl-C to stop)
#   CONNECTOR=cpu    DETACH=1 ./serve_vllm.sh up   # launch + wait, then EXIT leaving the container running
#   ./serve_vllm.sh stop                     # remove the server container
#   ./serve_vllm.sh                          # alias for `up`
#
# CONNECTOR arms:
#   none    plain baseline vLLM (no --kv-transfer-config; evicted KV recomputed)
#   cpu     OffloadingConnector + CPUOffloadingSpec        -> pinned host RAM
#   tiered  OffloadingConnector + TieringOffloadingSpec    -> CPU primary + fs secondary
#   certus  OffloadingConnector + CertusShmqOffloadingSpec -> host certus-server via /dev/shm
#           (the host certus-server shmq endpoint MUST already be running; this
#            script does NOT start it — same precondition as long-doc-qa's certus arm)
set -euo pipefail

CONNECTOR=${CONNECTOR:-none}

# ---- model / server params (defaults match the rest of the KV-offload suite) --
MODEL=${MODEL:-NousResearch/Meta-Llama-3-8B}
PORT=${PORT:-8000}
GPU=${GPU:-all}                          # podman CDI device selector (all | 0 | ...)
DTYPE=${DTYPE:-bfloat16}
LOAD_FORMAT=${LOAD_FORMAT:-auto}
TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-1}
GPU_MEM_UTIL=${GPU_MEM_UTIL:-0.90}
MAX_MODEL_LEN=${MAX_MODEL_LEN:-32768}    # > 8192 so agentic sessions + compaction fit (needs RoPE for Llama-3)
MAX_NUM_SEQS=${MAX_NUM_SEQS:-64}
PYTHONHASHSEED=${PYTHONHASHSEED:-0}      # reproducible prefix-cache block hashing
READY_TIMEOUT=${READY_TIMEOUT:-600}      # seconds to wait for model load
ENFORCE_EAGER=${ENFORCE_EAGER:-1}        # connectors are more robust without cudagraphs

# RoPE scaling is MODEL-SPECIFIC (see long-doc-qa/run_bench.sh header): Llama-3-8B
# has an 8192-wide rotary table, so any prompt position >= 8192 device-asserts
# unless we rescale. Enable linear x4 (-> 32768) for Llama-3 only; leave large-
# context models (Qwen2.5 = 131072) untouched. Override with HF_OVERRIDES=... ,
# or HF_OVERRIDES="" to force-disable.
case "${MODEL}" in
    *Meta-Llama-3-8B*|*Llama-3-8B*) _def_hf_overrides='{"rope_scaling":{"rope_type":"linear","factor":4.0}}' ;;
    *)                              _def_hf_overrides='' ;;
esac
HF_OVERRIDES=${HF_OVERRIDES-$_def_hf_overrides}

# ---- connector params -----------------------------------------------------
CPU_BYTES=${CPU_BYTES:-17179869184}                    # cpu / tiered primary host-RAM tier
FS_ROOT_DIR=${FS_ROOT_DIR:-/mnt/fs-tier/kv-tier}       # tiered fs secondary tier root (in-container path)
# tiered: bind-mount a host dir so the fs secondary tier survives on real storage
# (else it is written INSIDE the --rm container and vanishes at teardown, and never
# touches the RAID/XFS group we mean to measure). FS_ROOT_DIR MUST live under
# FS_TIER_MOUNT. Empty FS_TIER_HOST = no mount (tier is container-ephemeral).
FS_TIER_HOST=${FS_TIER_HOST:-}                         # tiered fs secondary: host dir to bind-mount
FS_TIER_MOUNT=${FS_TIER_MOUNT:-/mnt/fs-tier}           # tiered fs secondary: in-container mount point
SHM_PATH=${SHM_PATH:-/dev/shm/certus-shmq}             # certus shmq mailbox
SLAB_SIZE_BYTES=${SLAB_SIZE_BYTES:-2097152}            # certus per-block slab (>= block stride; see CopyToStore size bug)

# ---- images / container plumbing ------------------------------------------
ENGINE=${ENGINE:-podman}
# none/cpu/tiered share the unified offload image (built from
# ../kv-offload-replay/Dockerfile.offload; use --build-arg VLLM_FIX_TIERING=1 so
# tiering survives at scale). certus needs the shmq connector image.
OFFLOAD_IMAGE=${OFFLOAD_IMAGE:-certus-offload-bench}
SHMQ_IMAGE=${SHMQ_IMAGE:-localhost/certus-shmq-bench:latest}
SERVER_NAME=${SERVER_NAME:-sa-vllm-${CONNECTOR}}
HF_CACHE=${HF_CACHE:-$HOME/.cache/huggingface}
DETACH=${DETACH:-0}                      # 1 = return after ready, leave container running

case "${CONNECTOR}" in
    certus) SERVER_IMAGE=${SERVER_IMAGE:-$SHMQ_IMAGE} ;;
    *)      SERVER_IMAGE=${SERVER_IMAGE:-$OFFLOAD_IMAGE} ;;
esac

log() { echo ">> [serve_vllm ${CONNECTOR}] $*" >&2; }

stop_server() {
    log "removing server container ${SERVER_NAME}"
    ${ENGINE} rm -f "${SERVER_NAME}" >/dev/null 2>&1 || true
}

# ---- build the connector-specific vLLM + podman flags ---------------------
build_flags() {
    server_extra_run=()
    server_extra_args=()
    # All connector images bake the vllm CLI (FROM vllm/vllm-openai) but set their
    # own ENTRYPOINT (the offload/shmq driver); reset it back to `vllm serve`.
    entrypoint_flag=(--entrypoint '["vllm","serve"]')

    case "${CONNECTOR}" in
        none)
            log "baseline vLLM — no kv_transfer_config (evicted KV recomputed)"
            ;;
        cpu)
            kv_cfg="{\"kv_connector\":\"OffloadingConnector\",\"kv_role\":\"kv_both\",\"kv_connector_extra_config\":{\"cpu_bytes_to_use\":${CPU_BYTES},\"spec_name\":\"CPUOffloadingSpec\"}}"
            server_extra_args+=(--kv-transfer-config "${kv_cfg}")
            log "CPUOffload — CPUOffloadingSpec, cpu_bytes=${CPU_BYTES}"
            ;;
        tiered)
            # CPU primary (cpu_bytes_to_use) + "fs" secondary at FS_ROOT_DIR.
            # TieringOffloadingSpec resolves "fs" to FileSystemTierManager (vLLM >= 0.26).
            kv_cfg="{\"kv_connector\":\"OffloadingConnector\",\"kv_role\":\"kv_both\",\"kv_connector_extra_config\":{\"cpu_bytes_to_use\":${CPU_BYTES},\"spec_name\":\"TieringOffloadingSpec\",\"spec_extra_config\":{\"tiers\":[{\"type\":\"fs\",\"root_dir\":\"${FS_ROOT_DIR}\"}]}}}"
            server_extra_args+=(--kv-transfer-config "${kv_cfg}")
            server_extra_run+=(-e "FS_ROOT_DIR=${FS_ROOT_DIR}")
            # Persist the fs secondary tier on real storage when a host dir is given.
            if [ -n "${FS_TIER_HOST}" ]; then
                mkdir -p "${FS_TIER_HOST}" 2>/dev/null || true
                server_extra_run+=(-v "${FS_TIER_HOST}:${FS_TIER_MOUNT}:z")
            fi
            log "Tiered-CPU-FS — TieringOffloadingSpec, cpu_bytes=${CPU_BYTES}, fs root=${FS_ROOT_DIR}${FS_TIER_HOST:+ (host ${FS_TIER_HOST} -> ${FS_TIER_MOUNT})}"
            ;;
        certus)
            # shmq OffloadingConnector: the host certus-server owns SHM_PATH and
            # opens the CUDA IPC handles vLLM exports -> needs --ipc=host.
            kv_cfg="{\"kv_connector\":\"OffloadingConnector\",\"kv_role\":\"kv_both\",\"kv_connector_extra_config\":{\"spec_name\":\"CertusShmqOffloadingSpec\",\"spec_module_path\":\"certus_shmq_connector.spec\",\"shm_path\":\"${SHM_PATH}\",\"slab_size_bytes\":${SLAB_SIZE_BYTES}}}"
            server_extra_args+=(--kv-transfer-config "${kv_cfg}")
            server_extra_run+=(--ipc=host)
            log "Certus-shmq — CertusShmqOffloadingSpec, shm_path=${SHM_PATH} slab=${SLAB_SIZE_BYTES}B (host certus-server MUST be up)"
            ;;
        *)
            echo "ERROR: unknown CONNECTOR='${CONNECTOR}' (expected none|cpu|tiered|certus)" >&2
            exit 2
            ;;
    esac
    # NB: keep this an `if`, not `[ ... ] && ...`. As the LAST statement of the
    # function the `&&` form would make build_flags return the test's exit status,
    # so with ENFORCE_EAGER=0 it returns non-zero and `set -e` aborts start_server
    # right here (server never launches, caller sees "not ready").
    if [ "${ENFORCE_EAGER}" = "1" ]; then
        server_extra_args+=(--enforce-eager)
    fi
}

start_server() {
    build_flags
    log "launching vLLM (${SERVER_IMAGE}) for ${MODEL} on :${PORT}"
    ${ENGINE} rm -f "${SERVER_NAME}" >/dev/null 2>&1 || true
    # host networking: rootless-podman -p publishing is unreliable on this box, so
    # bind vLLM straight onto the host at :${PORT}; clients meet it at localhost.
    ${ENGINE} run -d --name "${SERVER_NAME}" \
        --device "nvidia.com/gpu=${GPU}" \
        --network host \
        "${server_extra_run[@]}" \
        "${entrypoint_flag[@]}" \
        -v "${HF_CACHE}:/root/.cache/huggingface:z" \
        -e HF_HUB_OFFLINE=1 \
        -e VLLM_ALLOW_LONG_MAX_MODEL_LEN=1 \
        -e PYTHONHASHSEED="${PYTHONHASHSEED}" \
        "${SERVER_IMAGE}" \
        --model "${MODEL}" \
        --dtype "${DTYPE}" \
        --load-format "${LOAD_FORMAT}" \
        --tensor-parallel-size "${TENSOR_PARALLEL_SIZE}" \
        ${HF_OVERRIDES:+--hf-overrides "${HF_OVERRIDES}"} \
        --max-model-len "${MAX_MODEL_LEN}" \
        --max-num-seqs "${MAX_NUM_SEQS}" \
        --gpu-memory-utilization "${GPU_MEM_UTIL}" \
        --enable-prefix-caching \
        --enable-log-requests \
        "${server_extra_args[@]}" \
        --port "${PORT}" >/dev/null

    log "waiting up to ${READY_TIMEOUT}s for /v1/models ..."
    local deadline=$((SECONDS + READY_TIMEOUT))
    until curl -fsS --max-time 3 "http://localhost:${PORT}/v1/models" >/dev/null 2>&1; do
        if [ "${SECONDS}" -ge "${deadline}" ]; then
            log "ERROR: server not ready after ${READY_TIMEOUT}s; last logs:"
            ${ENGINE} logs --tail 40 "${SERVER_NAME}" >&2 || true
            return 1
        fi
        if ! ${ENGINE} ps --filter "name=${SERVER_NAME}" --filter status=running -q | grep -q .; then
            log "ERROR: server container exited during startup; logs:"
            ${ENGINE} logs --tail 60 "${SERVER_NAME}" >&2 || true
            return 1
        fi
        sleep 3
    done
    log "server ready at http://localhost:${PORT}/v1"
}

cmd=${1:-up}
case "${cmd}" in
    up)
        start_server
        if [ "${DETACH}" = "1" ]; then
            log "detaching (container ${SERVER_NAME} left running; call \`$0 stop\` to remove)"
            exit 0
        fi
        trap stop_server EXIT INT TERM
        log "serving; Ctrl-C to stop"
        # block on the container so Ctrl-C tears it down via the trap
        ${ENGINE} wait "${SERVER_NAME}" >/dev/null 2>&1 || true
        ;;
    stop)
        stop_server
        ;;
    *)
        echo "usage: $0 [up|stop]   (env: CONNECTOR, MODEL, PORT, DETACH, ...)" >&2
        exit 2
        ;;
esac
