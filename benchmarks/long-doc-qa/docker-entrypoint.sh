#!/usr/bin/env bash
# Entrypoint for the LMCache long_doc_qa benchmark CLIENT.
#
# This is a pure OpenAI-API client: it connects to an ALREADY-RUNNING vLLM (or
# any OpenAI-compatible) server and drives the long-document QA workload. It
# never starts a server. Point it at your backend with BASE_URL (preferred) or
# HOST + PORT.
#
# Two ways to invoke:
#   1. Pass benchmark flags directly after the image name — they are used
#      verbatim and this env-assembly is skipped entirely:
#        docker run ... IMAGE --base-url http://host:8000/v1 --num-documents 16
#   2. Pass nothing and configure via env (the knobs below).
set -euo pipefail

# If the caller supplied any args, respect them exactly and get out of the way.
if [ "$#" -gt 0 ]; then
    exec python /workspace/long_doc_qa.py "$@"
fi

args=()

# --- server target (mandatory: BASE_URL, or HOST+PORT) -------------------
if [ -n "${BASE_URL:-}" ]; then
    args+=(--base-url "${BASE_URL}")
elif [ -n "${HOST:-}" ] && [ -n "${PORT:-}" ]; then
    args+=(--host "${HOST}" --port "${PORT}")
else
    echo "ERROR: set BASE_URL (e.g. http://host.docker.internal:8000/v1)" >&2
    echo "       or both HOST and PORT, or pass benchmark flags directly." >&2
    exit 2
fi

# --- model: 'auto' resolves from the server's /models endpoint -----------
args+=(--model "${MODEL:-auto}")

# --- workload shape (only append when the env var is set) ----------------
add() { [ -n "${2:-}" ] && args+=("$1" "$2") || true; }
add --document-length        "${DOCUMENT_LENGTH:-}"
add --num-documents          "${NUM_DOCUMENTS:-}"
add --output-len             "${OUTPUT_LEN:-}"
add --repeat-count           "${REPEAT_COUNT:-}"
add --repeat-mode            "${REPEAT_MODE:-}"
add --shuffle-seed           "${SHUFFLE_SEED:-}"
add --max-inflight-requests  "${MAX_INFLIGHT_REQUESTS:-}"
add --sleep-time-after-warmup "${SLEEP_TIME_AFTER_WARMUP:-}"
add --hit-miss-ratio         "${HIT_MISS_RATIO:-}"
add --eos-token-id           "${EOS_TOKEN_ID:-}"
add --trim-fraction          "${TRIM_FRACTION:-}"
add --output                 "${OUTPUT:-}"

# --- boolean flags -------------------------------------------------------
[ "${COMPLETIONS:-0}" = "1" ] && args+=(--completions) || true
[ "${VISUALIZE:-0}"   = "1" ] && args+=(--visualize)   || true
[ "${JSON_OUTPUT:-0}" = "1" ] && args+=(--json-output) || true

echo "+ python long_doc_qa.py ${args[*]}" >&2
exec python /workspace/long_doc_qa.py "${args[@]}"
