#!/bin/bash
# run-guidellm.sh — EXAMPLE guidellm client for a vLLM server started by
# run-serve-shmq.sh or run-serve-cputier.sh.
#
# guidellm (https://github.com/vllm-project/guidellm) is an HTTP load generator
# for OpenAI-compatible endpoints. This script points it at the /v1 surface the
# serve scripts publish and drives the "Shared-Prefix Synthetic" smoke workload
# from benchmarks/BENCHMARKING.md (a shared system prompt reused across a group
# of unique questions — the access pattern that exercises the KV-offload read
# path). Everything is env-overridable; this is a starting point, not a fixed
# benchmark.
#
# Prerequisites:
#   1. A server is already running and serving on ${TARGET} — start one first:
#        ./run-serve-certus-shmq.sh   # Certus-SHMQ backend (needs a host certus-server)
#        ./run-serve-cputier.sh       # native CPU+disk tiering backend
#   2. guidellm is installed on the HOST (not in the server container):
#        pip install guidellm            # or: pipx install guidellm
#
# Examples:
#   ./run-guidellm.sh                                  # sweep the full load range, 120s/stage
#   RATE_TYPE=throughput MAX_SECONDS=60 ./run-guidellm.sh
#   RATE_TYPE=constant RATE=8 ./run-guidellm.sh        # fixed 8 req/s
#   PORT=9000 MODEL=qwen2.5-7b ./run-guidellm.sh       # match a non-default serve port
#   DATA="prompt_tokens=512,output_tokens=512,prefix_tokens=4096,prefix_count=16" ./run-guidellm.sh
#
#   # Replay a Mooncake trace file (flat JSONL: timestamp,input_length,output_length,hash_ids).
#   # A Certus "cc" conversation trace must first be flattened with
#   # benchmarks/kv-offload-replay/cc_trace_to_mooncake.py (see that script). Point DATA at
#   # the resulting file; RATE_TYPE=replay reproduces the trace's inter-arrival timing.
#   DATA=/mnt/certus1/cc-traces-weka-062126.mooncake-131k.jsonl RATE_TYPE=replay ./run-guidellm.sh
#   DATA=/path/trace.jsonl HASH_ID_BLOCK_SIZE=64 RATE_TYPE=throughput ./run-guidellm.sh
#   DATA="kind=mooncake,path=/path/trace.jsonl,hash_id_block_size=64,timestamp_column=t" ./run-guidellm.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── Target endpoint (must match the running serve script) ───────────────────────
# guidellm's openai_http backend takes the server ROOT and appends /v1/...; the
# serve scripts publish IPv4 on 127.0.0.1 (podman does not publish IPv6).
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8000}"
TARGET="${TARGET:-http://${HOST}:${PORT}}"
MODEL="${MODEL:-qwen2.5-7b}"          # the --served-model-name the server advertises
PROCESSOR="${PROCESSOR:-Qwen/Qwen2.5-7B-Instruct}"  # tokenizer for token accounting

# ── Load profile ─────────────────────────────────────────────────────────────────
# rate-type: sweep (auto low->saturation), throughput (max), synchronous (1 at a
# time), constant/poisson (need RATE), replay (reproduce a trace's inter-arrival
# timing — pair with a Mooncake DATA file). Bound each stage by time or request count.
RATE_TYPE="${RATE_TYPE:-sweep}"
RATE="${RATE:-}"                       # req/s — only used by constant/poisson
MAX_CONCURRENCY="${MAX_CONCURRENCY:-256}"  # concurrency cap — REQUIRED by the throughput profile (guidellm >= 0.7)
SWEEP_SIZE="${SWEEP_SIZE:-}"           # sweep profile: number of rate stages (guidellm default 10); e.g. SWEEP_SIZE=2
TIME_SCALE="${TIME_SCALE:-1.0}"        # replay profile: multiply trace timestamps (2.0 = 2x slower arrivals)
MAX_SECONDS="${MAX_SECONDS:-120}"      # per-stage wall-clock budget
MAX_REQUESTS="${MAX_REQUESTS:-}"       # alternative bound; if set, overrides MAX_SECONDS

# ── Workload — either a synthetic spec OR a trace file ──────────────────────────
# DATA is interpreted three ways (see "Translate the DATA spec" below):
#   * a "kind=...,key=val" spec        -> passed to guidellm --data verbatim (any kind)
#   * a path to an existing file       -> replayed as a Mooncake trace (kind=mooncake)
#   * a comma-separated synthetic spec -> the BENCHMARKING.md smoke test (default):
#       prefix_tokens : shared system prompt per group
#       prefix_count  : distinct prefix groups
#       prompt_tokens : unique user question
#       output_tokens : generated length
DATA="${DATA:-prompt_tokens=256,output_tokens=256,prefix_tokens=2048,prefix_count=32}"
HASH_ID_BLOCK_SIZE="${HASH_ID_BLOCK_SIZE:-64}"  # tokens per hash id, for Mooncake file replay

STAMP="$(date +%Y%m%d_%H%M%S)"
OUTPUT="${OUTPUT:-${SCRIPT_DIR}/guidellm_${RATE_TYPE}_${STAMP}.json}"

# ── Preflight ──────────────────────────────────────────────────────────────────
if ! command -v guidellm >/dev/null 2>&1; then
  echo "error: guidellm not found on PATH — install it on the host: pip install guidellm" >&2
  exit 1
fi
# Fail fast with a clear message if no server is answering (rather than deep in guidellm).
if command -v curl >/dev/null 2>&1; then
  if ! curl -fsS --max-time 5 "${TARGET}/v1/models" >/dev/null 2>&1; then
    echo "error: no OpenAI server answering at ${TARGET}/v1/models" >&2
    echo "       start one first: ./run-serve-certus-shmq.sh  (or)  ./run-serve-cputier.sh" >&2
    exit 1
  fi
fi

# ── Translate the DATA spec to a JSON synthetic_text config ─────────────────────
# guidellm >= 0.7 validates --data strictly and no longer accepts the flat
# prefix_tokens/prefix_count keys — they must be expressed as a prefix_buckets
# entry. Keep the familiar comma-separated DATA env interface and fold any
# prefix_tokens/prefix_count into a single bucket here.
build_data_json() {
  local spec="$1" pt="" pc="" kv k v f
  local -a parts fields=()
  IFS=',' read -ra parts <<< "$spec"
  for kv in "${parts[@]}"; do
    [[ -z "$kv" ]] && continue
    k="${kv%%=*}"; v="${kv#*=}"
    case "$k" in
      prefix_tokens) pt="$v" ;;
      prefix_count)  pc="$v" ;;
      *) fields+=("\"$k\":$v") ;;
    esac
  done
  local json="{\"kind\":\"synthetic_text\""
  for f in "${fields[@]}"; do json+=",$f"; done
  if [[ -n "$pt" || -n "$pc" ]]; then
    json+=",\"prefix_buckets\":[{\"bucket_weight\":100,\"prefix_count\":${pc:-1},\"prefix_tokens\":${pt:-0}}]"
  fi
  printf '%s}' "$json"
}
# Resolve DATA into the single --data spec guidellm receives.
#   kind=...  -> verbatim (mooncake with custom columns, csv_file, huggingface, ...)
#   a file    -> Mooncake trace replay (flat JSONL/JSON/CSV/parquet of
#                timestamp,input_length,output_length,hash_ids)
#   otherwise -> synthetic_text (folded from the comma-separated spec above)
if [[ "$DATA" == kind=* ]]; then
  DATA_SPEC="$DATA"
elif [[ -f "$DATA" ]]; then
  DATA_SPEC="kind=mooncake,path=${DATA},hash_id_block_size=${HASH_ID_BLOCK_SIZE}"
elif [[ "$DATA" == */* || "$DATA" == *.jsonl || "$DATA" == *.json || "$DATA" == *.csv || "$DATA" == *.parquet ]]; then
  # Looks like a path but does not exist — fail clearly rather than mis-parsing it as synthetic.
  echo "error: DATA looks like a trace file but was not found: ${DATA}" >&2
  echo "       flatten a Certus cc-trace first: benchmarks/kv-offload-replay/cc_trace_to_mooncake.py" >&2
  exit 1
else
  DATA_SPEC="$(build_data_json "$DATA")"
fi

# ── Assemble guidellm args ──────────────────────────────────────────────────────
# guidellm >= 0.7 replaced the flat flags (--target/--model/--rate-type/--output-path)
# with structured "kind=...,key=val" options under the `run` subcommand.
ARGS=(
  run
  --backend "kind=openai_http,target=${TARGET},model=${MODEL}"
  --tokenizer "kind=huggingface_auto,model=${PROCESSOR}"
  --data "$DATA_SPEC"
  --output "kind=json,path=${OUTPUT}"
)
# constant/poisson require a numeric rate; throughput requires a max_concurrency;
# sweep/synchronous take no extra field.
if [[ "$RATE_TYPE" == "constant" || "$RATE_TYPE" == "poisson" ]]; then
  [[ -n "$RATE" ]] || { echo "error: RATE_TYPE=${RATE_TYPE} needs RATE=<req/s>" >&2; exit 1; }
  ARGS+=(--profile "kind=${RATE_TYPE},rate=${RATE}")
elif [[ "$RATE_TYPE" == "throughput" ]]; then
  # guidellm >= 0.7's throughput profile rejects a bare kind=throughput with
  # "Field required (at 'profile.throughput.max_concurrency')"; supply the cap.
  ARGS+=(--profile "kind=throughput,max_concurrency=${MAX_CONCURRENCY}")
elif [[ "$RATE_TYPE" == "replay" ]]; then
  # Reproduce the trace's inter-arrival timing from each row's relative_timestamp
  # (arrival = start + time_scale * relative_timestamp). Needs a trace DATA source.
  ARGS+=(--profile "kind=replay,time_scale=${TIME_SCALE}")
elif [[ "$RATE_TYPE" == "sweep" && -n "$SWEEP_SIZE" ]]; then
  # sweep_size = number of rate stages the sweep runs (guidellm default 10).
  ARGS+=(--profile "kind=sweep,sweep_size=${SWEEP_SIZE}")
else
  ARGS+=(--profile "kind=${RATE_TYPE}")
fi
# Bound each stage: request count takes precedence over wall-clock when both set.
if [[ -n "$MAX_REQUESTS" ]]; then
  ARGS+=(--constraint "kind=max_requests,count=${MAX_REQUESTS}")
else
  ARGS+=(--constraint "kind=max_duration,seconds=${MAX_SECONDS}")
fi

# ── KV-offload / prefix-cache metrics probe ─────────────────────────────────────
# The prefix-cache hit rate you care about for a tiering backend is the SERVER's
# "External prefix cache hit rate" (blocks served from the CPU/fs offload tiers),
# NOT the GPU "Prefix cache hit rate" — under the OffloadingConnector the GPU one
# sits at ~0% by design because reused prefixes live in the offload tier. So we
# snapshot the server's /metrics counters before and after the run: if the
# offload STORE counters climb but the external HIT counters stay flat, stores
# are landing but no lookup ever matches (shared prefixes destroyed upstream) —
# distinct from the case where stores never happen at all (dropped stores).
METRICS_URL="${TARGET}/metrics"
METRICS_OUT="${OUTPUT%.json}.metrics.txt"
# Broad grep: vLLM's offload/prefix counter names vary across versions, so match
# any that mention prefix cache, the external/offload path, or the KV connector.
METRICS_RE='prefix_cache|external|offload|kv_transfer|connector|kv_cache_usage'
snap_metrics() {  # $1 = label; prints matching counters (or a note if none)
  local out
  command -v curl >/dev/null 2>&1 || { echo "### ${1}: curl unavailable"; return; }
  out="$(curl -fsS --max-time 5 "$METRICS_URL" 2>/dev/null | grep -iE "$METRICS_RE" | grep -v '^#')"
  echo "### ${1} $(date -Is)"
  [[ -n "$out" ]] && echo "$out" || echo "# (no matching counters at ${METRICS_URL})"
}

echo "[guidellm] target=${TARGET}  model=${MODEL}  rate-type=${RATE_TYPE}${RATE:+ rate=${RATE}}"
echo "[guidellm] data: ${DATA}"
echo "[guidellm] results -> ${OUTPUT}"
echo "[guidellm] metrics -> ${METRICS_OUT}"

snap_metrics "BEFORE" > "$METRICS_OUT" 2>/dev/null || true

# Run guidellm as a child (not exec) so we can capture post-run metrics. Do not
# let a nonzero guidellm exit abort the metrics capture under `set -e`.
set +e
guidellm "${ARGS[@]}"
rc=$?
set -e

snap_metrics "AFTER" >> "$METRICS_OUT" 2>/dev/null || true

echo "[guidellm] --- KV-offload / prefix-cache counters after run ---"
snap_metrics "AFTER" | grep -v '^###'
echo "[guidellm] full before/after snapshot: ${METRICS_OUT}"
exit "$rc"
