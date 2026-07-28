#!/usr/bin/env bash
# Run the long_doc_qa benchmark against Llama-3-8B with a small calibration
# workload. By default this launches a BASELINE plain-vLLM OpenAI server
# (no KV offload — the control), waits for it, runs the client, and tears the
# server down. Point it at another backend (Certus / LMCache / CPU-offload)
# by exporting SERVE=0 and BASE_URL=<that server's /v1> — then no server is
# launched here.
#
#   ./run_llama3.sh                       # baseline llama3, requested params
#   SERVE=0 BASE_URL=http://host:8000/v1 ./run_llama3.sh   # use an existing server
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
NUM_DOCUMENTS=${NUM_DOCUMENTS:-5}
DOCUMENT_LENGTH=${DOCUMENT_LENGTH:-10000}
OUTPUT_LEN=${OUTPUT_LEN:-1}
REPEAT_COUNT=${REPEAT_COUNT:-1}
REPEAT_MODE=${REPEAT_MODE:-tile}
MAX_INFLIGHT_REQUESTS=${MAX_INFLIGHT_REQUESTS:-1}

# ---- server / client plumbing --------------------------------------------
PORT=${PORT:-8000}
BASE_URL=${BASE_URL:-http://localhost:${PORT}/v1}
SERVE=${SERVE:-1}                       # 1 = launch baseline vLLM here; 0 = use BASE_URL
GPU=${GPU:-all}                         # podman CDI device selector (all | 0 | ...)
MAX_MODEL_LEN=${MAX_MODEL_LEN:-32768}   # forced > 8192 to fit the 10k-token docs
# RoPE scaling to extend Llama-3's 8192 rotary table to cover MAX_MODEL_LEN.
# Linear x4 -> 8192*4 = 32768. REQUIRED for DOCUMENT_LENGTH > ~7800 (see header).
# Set HF_OVERRIDES="" to disable (only safe when all prompts stay under 8192).
HF_OVERRIDES=${HF_OVERRIDES:-'{"rope_scaling":{"rope_type":"linear","factor":4.0}}'}
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
READY_TIMEOUT=${READY_TIMEOUT:-600}     # seconds to wait for model load

ENGINE=${ENGINE:-podman}
SERVER_IMAGE=${SERVER_IMAGE:-docker.io/vllm/vllm-openai:v0.20.0}
CLIENT_IMAGE=${CLIENT_IMAGE:-localhost/long-doc-qa-bench:latest}
SERVER_NAME=${SERVER_NAME:-ldq-vllm-llama3}
HF_CACHE=${HF_CACHE:-$HOME/.cache/huggingface}
RESULTS=${RESULTS:-$PWD/results/llama3-smoke}

mkdir -p "$RESULTS"

cleanup() {
    if [ "${SERVE}" = "1" ]; then
        echo ">> stopping server container ${SERVER_NAME}" >&2
        ${ENGINE} rm -f "${SERVER_NAME}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# ---- 1. launch baseline vLLM OpenAI server (unless SERVE=0) ---------------
if [ "${SERVE}" = "1" ]; then
    echo ">> launching baseline vLLM server for ${MODEL} on :${PORT}" >&2
    ${ENGINE} rm -f "${SERVER_NAME}" >/dev/null 2>&1 || true
    # host networking: rootless-podman `-p` publishing is unreliable on this
    # box, and the client already uses --network host — so bind vLLM straight
    # onto the host at :${PORT} and both sides meet at localhost:${PORT}.
    ${ENGINE} run -d --name "${SERVER_NAME}" \
        --device "nvidia.com/gpu=${GPU}" \
        --network host \
        -v "${HF_CACHE}:/root/.cache/huggingface:z" \
        -e HF_HUB_OFFLINE=1 \
        -e VLLM_ALLOW_LONG_MAX_MODEL_LEN=1 \
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
    echo ">> scraping server /metrics + logs into ${RESULTS}" >&2
    curl -fsS --max-time 5 "http://localhost:${PORT}/metrics" \
        > "${RESULTS}/metrics.txt" 2>/dev/null || true
    ${ENGINE} logs "${SERVER_NAME}" > "${RESULTS}/server.log" 2>&1 || true

    # --- per-iteration engine stats emitted by vLLM's loggers.py: throughput,
    #     KV cache usage and prefix-cache hit rate, one line per stats interval.
    #     These are the driver's primary offload/cache readout. Strip the
    #     "(APIServer pid=N)" prefix and drop fully-idle ticks (0 tok/s AND
    #     0% KV) so only lines with real activity survive.
    grep -E "Engine [0-9]+: Avg prompt throughput" "${RESULTS}/server.log" 2>/dev/null \
        | sed -E 's/^\(APIServer pid=[0-9]+\) //' \
        | grep -vE "Avg prompt throughput: 0.0 tokens/s.*GPU KV cache usage: 0.0%" \
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
