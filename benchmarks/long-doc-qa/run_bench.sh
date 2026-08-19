#!/usr/bin/env bash
# Run the long_doc_qa benchmark against an OpenAI-compatible server with a small
# calibration workload. MODEL is a param (defaults to Llama-3-8B; e.g. set
# MODEL=Qwen/Qwen2.5-7B-Instruct). By default this launches a BASELINE plain-vLLM
# OpenAI server
# (no KV offload — the control), waits for it, runs the client, and tears the
# server down. Point it at another backend (Certus / LMCache / CPU-offload)
# by exporting SERVE=0 and BASE_URL=<that server's /v1> — then no server is
# launched here.
#
#   ./run_bench.sh                        # baseline (default MODEL=Llama-3-8B)
#   SERVE=0 BASE_URL=http://host:8000/v1 ./run_bench.sh   # use an existing server
#
# MODEL is a param (env-overridable, defaults to Llama-3-8B) — e.g.
#   MODEL=Qwen/Qwen2.5-7B-Instruct ./run_bench.sh
# RoPE scaling below is applied automatically only for models that need it
# (Llama-3-8B); large-context models like Qwen2.5 (131072) get none.
#
# NOTE on doc-length vs context: Llama-3-8B's native context is 8192, but
# DOCUMENT_LENGTH (10000) produces a ~10000-token prompt. Simply forcing a bigger
# MAX_MODEL_LEN via VLLM_ALLOW_LONG_MAX_MODEL_LEN=1 is NOT enough — that flag only
# lets you *set* a longer limit; it does not extend the rotary-embedding (RoPE)
# table, so any token position >= 8192 indexes out of bounds and vLLM dies with a
# CUDA "device-side assert triggered" (index < 8192) mid-run. To actually accept
# 10k-token prompts we apply RoPE scaling (HF_OVERRIDES, linear x4 -> effective
# 32768), which rebuilds the rotary table for the full range. Positions past 8192
# then use scaled RoPE, so generated text is low-quality — irrelevant here (filler
# docs; we measure TTFT/throughput/prefix-cache only). To skip scaling instead,
# set DOCUMENT_LENGTH below ~7800 so every prompt stays within the native 8192.
set -euo pipefail

# ---- workload params (defaults = the requested calibration run) ----------
MODEL=${MODEL:-NousResearch/Meta-Llama-3-8B}
# Defaults = the tier-exercising run: 51 docs x 10k tokens overflows the GPU KV
# budget so evicted blocks are re-fetched from the offload tier (External prefix
# cache hit ~100%), unlike 5 docs where the working set stays GPU-resident and
# Certus is written but never read. out=100 + 4-way inflight give a realistic
# generation-bound wall clock. See results/certus-51docs/.
NUM_DOCUMENTS=${NUM_DOCUMENTS:-51}
DOCUMENT_LENGTH=${DOCUMENT_LENGTH:-10000}
OUTPUT_LEN=${OUTPUT_LEN:-100}
REPEAT_COUNT=${REPEAT_COUNT:-1}
REPEAT_MODE=${REPEAT_MODE:-tile}
MAX_INFLIGHT_REQUESTS=${MAX_INFLIGHT_REQUESTS:-4}

# ---- server / client plumbing --------------------------------------------
PORT=${PORT:-8000}
BASE_URL=${BASE_URL:-http://localhost:${PORT}/v1}
SERVE=${SERVE:-1}                       # 1 = launch baseline vLLM here; 0 = use BASE_URL
GPU=${GPU:-all}                         # podman CDI device selector (all | 0 | ...)
MAX_MODEL_LEN=${MAX_MODEL_LEN:-32768}   # forced > 8192 to fit the 10k-token docs
# RoPE scaling is MODEL-SPECIFIC. Llama-3-8B has an 8192-wide rotary table, so
# DOCUMENT_LENGTH > ~7800 needs linear x4 (-> 32768) or vLLM device-asserts (see
# header). Models with a natively-large context (Qwen2.5 = 131072) must NOT be
# scaled — it would corrupt their positions. So default the override ON only for
# Llama-3; empty otherwise. Override explicitly with HF_OVERRIDES=... ; use
# HF_OVERRIDES="" (exported empty) to force-disable for a small-context model.
case "${MODEL}" in
    *Meta-Llama-3-8B*|*Llama-3-8B*) _def_hf_overrides='{"rope_scaling":{"rope_type":"linear","factor":4.0}}' ;;
    *)                              _def_hf_overrides='' ;;
esac
HF_OVERRIDES=${HF_OVERRIDES-$_def_hf_overrides}
TENSOR_PARALLEL_SIZE=${TENSOR_PARALLEL_SIZE:-1}
# NOTE: --load-format dummy (random weights) is TEMPTING here — output text is
# irrelevant (filler docs; we only measure TTFT/throughput/prefix-cache) and it
# skips the ~16GB weight load. But it does NOT work: uninitialized weights make
# the forward pass emit inf/NaN activations that trip a device-side assert in a
# CUDA kernel mid-generation ("CUDA error: device-side assert triggered"),
# killing EngineCore — even with greedy (temperature=0) decoding. So we load the
# real (locally cached) weights; that's the only cost of the extra startup time.
LOAD_FORMAT=${LOAD_FORMAT:-auto}
DTYPE=${DTYPE:-bfloat16}
GPU_MEM_UTIL=${GPU_MEM_UTIL:-0.90}
# Fix Python's hash seed in the server so prefix-cache block hashing (and any
# other dict/set-order-sensitive path) is reproducible across runs.
PYTHONHASHSEED=${PYTHONHASHSEED:-0}
READY_TIMEOUT=${READY_TIMEOUT:-600}     # seconds to wait for model load
# After the client finishes, wait this long before scraping/teardown so vLLM
# emits its post-run idle stats tick — that tick carries the settled cumulative
# prefix-cache hit rate. Must exceed vLLM's ~10s stats-logging interval.
STATS_SETTLE=${STATS_SETTLE:-15}

# ---- KV-offload connector (optional) --------------------------------------
# CONNECTOR=none   -> plain baseline vLLM (the control; default).
# CONNECTOR=certus -> attach Certus' shmq OffloadingConnector. The certus-server
#   must ALREADY be running on the host (target/release/certus-server ...
#   --shm-path /dev/shm/certus-shmq --channels 32 --format) — this script does NOT
#   start it. Requires:
#     * the connector-equipped image (certus-shmq-bench = vllm-openai + the
#       certus_shmq_connector package). Its ENTRYPOINT runs the multiturn driver,
#       so we reset it to `vllm serve` below.
#     * --ipc=host, which does double duty: the host certus-server can open the
#       CUDA IPC handles the container's vLLM process exports for its KV cache,
#       AND the container sees the host /dev/shm mailbox at SHM_PATH.
#   There is no network transport — the shared /dev/shm path IS the endpoint, so
#   the container just needs SHM_PATH to match the server's --shm-path.
CONNECTOR=${CONNECTOR:-none}
SHM_PATH=${SHM_PATH:-/dev/shm/certus-shmq}
# MUST be >= the per-block Reserve stride (block_bytes = KV page_size x num_layers,
# e.g. 917504 for Qwen2.5-7B). If slab_size < block_bytes the server's CopyToStore
# D2H bounds check fails on every block and offload silently dies (see the
# certus CopyToStore size bug). 2 MiB clears Qwen; bump for larger models.
SLAB_SIZE_BYTES=${SLAB_SIZE_BYTES:-2097152}
ENFORCE_EAGER=${ENFORCE_EAGER:-1}       # connectors are more robust without cudagraphs

ENGINE=${ENGINE:-podman}
# Connector-aware defaults: Certus needs the connector-equipped image (stock
# vllm-openai lacks certus_shmq_connector) and its own result/container names.
if [ "${CONNECTOR}" = "certus" ]; then
    _def_server_image=localhost/certus-shmq-bench:latest
    _def_server_name=ldq-vllm-certus
    _def_results=$PWD/results/certus-smoke
else
    _def_server_image=docker.io/vllm/vllm-openai:v0.20.0
    _def_server_name=ldq-vllm-llama3
    _def_results=$PWD/results/llama3-smoke
fi
SERVER_IMAGE=${SERVER_IMAGE:-$_def_server_image}
CLIENT_IMAGE=${CLIENT_IMAGE:-localhost/long-doc-qa-bench:latest}
SERVER_NAME=${SERVER_NAME:-$_def_server_name}
HF_CACHE=${HF_CACHE:-$HOME/.cache/huggingface}
RESULTS=${RESULTS:-$_def_results}

mkdir -p "$RESULTS"

cleanup() {
    if [ "${SERVE}" = "1" ]; then
        echo ">> stopping server container ${SERVER_NAME}" >&2
        ${ENGINE} rm -f "${SERVER_NAME}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# ---- connector-specific podman + vLLM flags -------------------------------
# server_extra_run  : extra `podman run` flags (placed among the run flags,
#                     BEFORE the image); server_extra_args : extra vLLM CLI args
#                     (placed AFTER the image). entrypoint_flag resets the
#                     connector image's driver ENTRYPOINT back to `vllm serve`.
server_extra_run=()
server_extra_args=()
entrypoint_flag=()
if [ "${CONNECTOR}" = "certus" ]; then
    entrypoint_flag=(--entrypoint '["vllm","serve"]')
    server_extra_run+=(--ipc=host)
    kv_cfg="{\"kv_connector\":\"OffloadingConnector\",\"kv_role\":\"kv_both\",\"kv_connector_extra_config\":{\"spec_name\":\"CertusShmqOffloadingSpec\",\"spec_module_path\":\"certus_shmq_connector.spec\",\"shm_path\":\"${SHM_PATH}\",\"slab_size_bytes\":${SLAB_SIZE_BYTES}}}"
    server_extra_args+=(--kv-transfer-config "${kv_cfg}")
    [ "${ENFORCE_EAGER}" = "1" ] && server_extra_args+=(--enforce-eager)
fi

# ---- 1. launch vLLM OpenAI server (unless SERVE=0) ------------------------
if [ "${SERVE}" = "1" ]; then
    echo ">> launching vLLM server (connector=${CONNECTOR}) for ${MODEL} on :${PORT}" >&2
    [ "${CONNECTOR}" = "certus" ] && echo ">> Certus offload -> ${SHM_PATH} (slab=${SLAB_SIZE_BYTES}B); server must be up" >&2
    ${ENGINE} rm -f "${SERVER_NAME}" >/dev/null 2>&1 || true
    # host networking: rootless-podman `-p` publishing is unreliable on this
    # box, and the client already uses --network host — so bind vLLM straight
    # onto the host at :${PORT} and both sides meet at localhost:${PORT}.
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
        --gpu-memory-utilization "${GPU_MEM_UTIL}" \
        --enable-prefix-caching \
        --enable-log-requests \
        "${server_extra_args[@]}" \
        --port "${PORT}" >/dev/null
    echo ">> waiting up to ${READY_TIMEOUT}s for /v1/models ..." >&2
    deadline=$((SECONDS + READY_TIMEOUT))
    until curl -fsS --max-time 3 "http://localhost:${PORT}/v1/models" >/dev/null 2>&1; do
        if [ "${SECONDS}" -ge "${deadline}" ]; then
            echo "ERROR: server not ready after ${READY_TIMEOUT}s; last logs:" >&2
            ${ENGINE} logs --tail 40 "${SERVER_NAME}" >&2 || true
            exit 1
        fi
        # bail early if the container died
        if ! ${ENGINE} ps --filter "name=${SERVER_NAME}" --filter status=running -q | grep -q .; then
            echo "ERROR: server container exited during startup; logs:" >&2
            ${ENGINE} logs --tail 60 "${SERVER_NAME}" >&2 || true
            exit 1
        fi
        sleep 3
    done
    echo ">> server ready" >&2
fi

# ---- 2. run the benchmark client -----------------------------------------
echo ">> running long_doc_qa: docs=${NUM_DOCUMENTS} len=${DOCUMENT_LENGTH} out=${OUTPUT_LEN} repeat=${REPEAT_COUNT}/${REPEAT_MODE} inflight=${MAX_INFLIGHT_REQUESTS}" >&2
${ENGINE} run --rm --network host \
    -v "${RESULTS}:/workspace/results:z" \
    -e BASE_URL="${BASE_URL}" \
    -e MODEL="${MODEL}" \
    -e NUM_DOCUMENTS="${NUM_DOCUMENTS}" \
    -e DOCUMENT_LENGTH="${DOCUMENT_LENGTH}" \
    -e OUTPUT_LEN="${OUTPUT_LEN}" \
    -e REPEAT_COUNT="${REPEAT_COUNT}" \
    -e REPEAT_MODE="${REPEAT_MODE}" \
    -e MAX_INFLIGHT_REQUESTS="${MAX_INFLIGHT_REQUESTS}" \
    -e COMPLETIONS="${COMPLETIONS:-1}" \
    -e JSON_OUTPUT=1 \
    "${CLIENT_IMAGE}"

# ---- 3. collect server-side cache/offload stats (before teardown) --------
if [ "${SERVE}" = "1" ]; then
    # let vLLM log its post-run idle tick (settled prefix-cache hit rate) before
    # we scrape the logs and tear the server down.
    echo ">> waiting ${STATS_SETTLE}s for vLLM's settling engine-stats tick ..." >&2
    sleep "${STATS_SETTLE}"
    echo ">> scraping server /metrics + logs into ${RESULTS}" >&2
    curl -fsS --max-time 5 "http://localhost:${PORT}/metrics" \
        > "${RESULTS}/metrics.txt" 2>/dev/null || true
    ${ENGINE} logs "${SERVER_NAME}" > "${RESULTS}/server.log" 2>&1 || true

    # --- per-interval engine stats emitted by vLLM's loggers.py: prompt & gen
    #     throughput, GPU KV cache usage and prefix-cache hit rate, one line per
    #     stats interval. Capture EVERY line (only strip the "(APIServer pid=N)"
    #     prefix). Do NOT drop idle ticks: the trailing "Running: 0" tick carries
    #     the SETTLED cumulative prefix-cache hit rate (e.g. "... Prefix cache hit
    #     rate: 47.6%") — the whole point of the warm round. Filtering idle lines
    #     would throw away exactly that number.
    grep -E "Engine [0-9]+: Avg prompt throughput" "${RESULTS}/server.log" 2>/dev/null \
        | sed -E 's/^\(APIServer pid=[0-9]+\) //' \
        > "${RESULTS}/engine_stats.txt" || true

    echo "==== engine stats — throughput / KV / prefix-cache hit rate (per interval) ====" >&2
    if [ -s "${RESULTS}/engine_stats.txt" ]; then
        cat "${RESULTS}/engine_stats.txt" >&2
    else
        echo "  (no active engine-stats intervals logged — run may be shorter than the stats interval)" >&2
    fi

    # --- cumulative prefix-cache counters from /metrics, with a computed rate.
    echo "==== prefix-cache counters (from /metrics) ====" >&2
    q=$(grep -E "^vllm:prefix_cache_queries_total" "${RESULTS}/metrics.txt" 2>/dev/null | awk '{print $2}' | tail -1)
    h=$(grep -E "^vllm:prefix_cache_hits_total"    "${RESULTS}/metrics.txt" 2>/dev/null | awk '{print $2}' | tail -1)
    if [ -n "${q:-}" ] && [ -n "${h:-}" ]; then
        awk -v h="$h" -v q="$q" 'BEGIN{ r=(q>0)?100*h/q:0; printf "  prefix_cache_hits=%d  queries=%d  hit_rate=%.2f%%\n", h, q, r }' >&2
    else
        echo "  (no prefix_cache counters found in /metrics)" >&2
    fi
fi

echo ">> results (warmup_round.csv / query_round.csv / engine_stats.txt / metrics.txt / server.log) in ${RESULTS}" >&2
