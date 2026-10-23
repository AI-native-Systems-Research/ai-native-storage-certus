#!/bin/bash
# run-aiperf-weka.sh — replay a WEKA-format cc-trace onto a vLLM server (Certus-SHMQ
# or cputier backend) with NVIDIA aiperf, preserving MULTI-TURN + subagent structure.
#
# Why this exists (and why not guidellm): guidellm's `mooncake` loader replays each
# row as an independent single request, so the flattened *.mooncake-*.jsonl traces
# lose all turn/subagent structure (a 393-conversation corpus collapses to ~71%
# "single-turn"). guidellm's native `weka` loader is not in any released version
# (checked: 0.7.4 has trace_mooncake only). aiperf ships a real WekaTraceLoader that
# reconstructs conversations, subagent chains (parallel, SPAWN_JOIN barrier), and
# per-turn think-time — so USE AIPERF for multi-turn WEKA replay.
#
# This is a CLIENT: a vLLM OpenAI endpoint must already be serving on ${TARGET}.
# Start one first (the Certus backend needs a host certus-server + the shmq image):
#     ./run-serve-certus-shmq.sh          # Certus-SHMQ backend
#     ./run-serve-cputier.sh              # native CPU+disk tiering baseline
# and install aiperf on the HOST:  pip install --user aiperf   (this box: 0.12.0).
#
# KEY GOTCHA baked in here: aiperf's WekaTraceLoader does orjson.loads() on the WHOLE
# file, i.e. ONE JSON object == ONE conversation. The HuggingFace/Certus corpus ships
# as a single multi-line .jsonl (one conversation per line), which aiperf REJECTS
# ("unexpected content after document: line 2"). So we split the .jsonl into a
# directory of per-conversation .json files (cached; re-split only when the source is
# newer) and point --input-file at the directory.
#
# TIMING: WEKA encodes per-turn *delays*, not absolute timestamps, so plain
# --fixed-schedule fails ("no timing data in first record"). Pick a mode via TIMING:
#     asap  (default) -> --no-fixed-schedule --ignore-trace-delays   (back-to-back; max stress)
#     think           -> --no-fixed-schedule --use-think-time-only   (replay recorded think-time)
#     delaycap        -> --no-fixed-schedule --inter-turn-delay-cap-seconds ${DELAY_CAP}
#                        (replay authored inter-turn delays, but clamp the pathological
#                         ones — the corpus has gaps up to ~68000s that would otherwise
#                         make the client sleep for hours)
#
# Examples:
#   ./run-aiperf-weka.sh                                   # full corpus, asap, all convs
#   REQUEST_COUNT=50 ./run-aiperf-weka.sh                  # quick bounded run
#   TIMING=think ./run-aiperf-weka.sh                      # realistic per-request pacing
#   TIMING=delaycap DELAY_CAP=30 ./run-aiperf-weka.sh      # replay gaps, clamped to 30s
#   TRACE=/mnt/certus1/cc-traces-weka-062126.jsonl CONCURRENCY=8 ./run-aiperf-weka.sh
#   PORT=9000 MODEL=qwen2.5-7b ./run-aiperf-weka.sh        # match a non-default serve port
#   MAX_CONTEXT_LEN=131072 ./run-aiperf-weka.sh            # drop convs whose peak ctx exceeds the window
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── Target endpoint (must match the running serve script) ───────────────────────
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8000}"
TARGET="${TARGET:-http://${HOST}:${PORT}}"
MODEL="${MODEL:-qwen2.5-7b}"                          # --served-model-name the server advertises
TOKENIZER="${TOKENIZER:-Qwen/Qwen2.5-7B-Instruct}"   # aiperf rebuilds prompts from hash_ids -> tokenizer required
ENDPOINT_TYPE="${ENDPOINT_TYPE:-chat}"

# ── Trace source ────────────────────────────────────────────────────────────────
# TRACE may be: a WEKA .jsonl (auto-split), a single-conversation .json, or a
# directory of per-conversation .json files (used as-is).
TRACE="${TRACE:-/mnt/certus1/cc-traces-weka-062126.jsonl}"

# ── Timing / load profile ────────────────────────────────────────────────────────
TIMING="${TIMING:-asap}"            # asap | think | delaycap  (see header)
DELAY_CAP="${DELAY_CAP:-30}"        # seconds; only used by TIMING=delaycap
CONCURRENCY="${CONCURRENCY:-1}"     # concurrent requests (aiperf --concurrency)
STREAMING="${STREAMING:-1}"         # 1 = --streaming (per-token metrics), 0 = non-streaming
REQUEST_COUNT="${REQUEST_COUNT:-}"  # cap total requests (turns); empty = whole corpus
BENCHMARK_DURATION="${BENCHMARK_DURATION:-}"  # alternative wall-clock cap (seconds)
NUM_CONVERSATIONS="${NUM_CONVERSATIONS:-}"    # cap # conversations loaded (--num-dataset-entries)
MAX_CONTEXT_LEN="${MAX_CONTEXT_LEN:-}"        # drop convs whose peak context exceeds this (--max-context-length)

STAMP="$(date +%Y%m%d_%H%M%S)"
ARTIFACT_DIR="${ARTIFACT_DIR:-${SCRIPT_DIR}/aiperf_weka_${TIMING}_${STAMP}}"

# ── Preflight ──────────────────────────────────────────────────────────────────
if ! command -v aiperf >/dev/null 2>&1; then
  echo "error: aiperf not found on PATH — install it on the host: pip install --user aiperf" >&2
  exit 1
fi
if command -v curl >/dev/null 2>&1; then
  if ! curl -fsS --max-time 5 "${TARGET}/v1/models" >/dev/null 2>&1; then
    echo "error: no OpenAI server answering at ${TARGET}/v1/models" >&2
    echo "       start one first: ./run-serve-certus-shmq.sh  (or)  ./run-serve-cputier.sh" >&2
    exit 1
  fi
fi

# ── Resolve TRACE -> a directory of per-conversation .json files ────────────────
# aiperf wants one JSON object per file; split a multi-line .jsonl if needed.
resolve_input_dir() {
  local trace="$1"
  if [[ -d "$trace" ]]; then                    # already a directory of per-conv files
    echo "$trace"; return
  fi
  if [[ ! -f "$trace" ]]; then
    echo "error: TRACE not found: $trace" >&2; exit 1
  fi
  # A single-object .json: is it one JSON doc? If so, aiperf can read the file/dir directly.
  # Detect a .jsonl (multiple top-level objects) by counting non-blank lines.
  local nlines
  nlines="$(grep -cve '^[[:space:]]*$' "$trace" || true)"
  if [[ "$nlines" -le 1 ]]; then                # single JSON object -> use its parent dir
    echo "$(dirname "$trace")"; return
  fi
  # Multi-line .jsonl -> split into a cached sibling directory.
  local split_dir="${SPLIT_DIR:-${trace%.jsonl}.weka-split}"
  # Re-split only if the split dir is missing or older than the source.
  if [[ ! -d "$split_dir" || "$trace" -nt "$split_dir" ]]; then
    echo "[aiperf-weka] splitting $trace ($nlines convs) -> $split_dir" >&2
    rm -rf "$split_dir"; mkdir -p "$split_dir"
    python3 - "$trace" "$split_dir" >&2 <<'PY'
import json, sys, os
src, dst = sys.argv[1], sys.argv[2]
n = 0
seen = {}
with open(src) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        d = json.loads(line)
        cid = str(d.get("id") or f"conv{n:06d}")
        # guard against duplicate/empty ids clobbering files
        if cid in seen:
            seen[cid] += 1
            cid = f"{cid}-{seen[cid]}"
        else:
            seen[cid] = 0
        with open(os.path.join(dst, f"{cid}.json"), "w") as o:
            json.dump(d, o)
        n += 1
print(f"[aiperf-weka] wrote {n} per-conversation files", file=sys.stderr)
PY
  else
    echo "[aiperf-weka] reusing cached split dir $split_dir (source not newer)" >&2
  fi
  echo "$split_dir"
}
INPUT_DIR="$(resolve_input_dir "$TRACE")"

# ── Assemble aiperf args ──────────────────────────────────────────────────────────
ARGS=(
  profile
  --url "$TARGET"
  --model "$MODEL"
  --tokenizer "$TOKENIZER"
  --endpoint-type "$ENDPOINT_TYPE"
  --custom-dataset-type weka_trace
  --input-file "$INPUT_DIR"
  --concurrency "$CONCURRENCY"
  --artifact-dir "$ARTIFACT_DIR"
)
[[ "$STREAMING" == "1" ]] && ARGS+=(--streaming)

# Timing mode -> the flag set validated in benchmarks (plain --fixed-schedule fails on WEKA).
case "$TIMING" in
  asap)     ARGS+=(--no-fixed-schedule --ignore-trace-delays) ;;
  think)    ARGS+=(--no-fixed-schedule --use-think-time-only) ;;
  delaycap) ARGS+=(--no-fixed-schedule --inter-turn-delay-cap-seconds "$DELAY_CAP") ;;
  *) echo "error: TIMING must be asap|think|delaycap (got '$TIMING')" >&2; exit 1 ;;
esac

# Bounds (optional).
[[ -n "$REQUEST_COUNT"      ]] && ARGS+=(--request-count "$REQUEST_COUNT")
[[ -n "$BENCHMARK_DURATION" ]] && ARGS+=(--benchmark-duration "$BENCHMARK_DURATION")
[[ -n "$NUM_CONVERSATIONS"  ]] && ARGS+=(--num-dataset-entries "$NUM_CONVERSATIONS")
[[ -n "$MAX_CONTEXT_LEN"    ]] && ARGS+=(--max-context-length "$MAX_CONTEXT_LEN")

# ── Certus / KV-offload metrics snapshot (server-side, like run-guidellm.sh) ──────
# The number that matters for a tiering backend is the SERVER's external/offload
# counters (blocks served from the Certus tier), not the GPU prefix-cache rate.
METRICS_URL="${TARGET}/metrics"
METRICS_OUT="${ARTIFACT_DIR}/certus_offload.metrics.txt"
METRICS_RE='external_prefix_cache|kv_offload|prefix_cache_(hits|queries)|kv_transfer|offload'
snap_metrics() {   # $1 = label
  local out
  command -v curl >/dev/null 2>&1 || { echo "### ${1}: curl unavailable"; return; }
  out="$(curl -fsS --max-time 5 "$METRICS_URL" 2>/dev/null | grep -iE "$METRICS_RE" | grep -v '^#')"
  echo "### ${1} $(date -Is)"
  [[ -n "$out" ]] && echo "$out" || echo "# (no matching counters at ${METRICS_URL})"
}

mkdir -p "$ARTIFACT_DIR"
echo "[aiperf-weka] target=${TARGET}  model=${MODEL}  timing=${TIMING}  concurrency=${CONCURRENCY}"
echo "[aiperf-weka] input=${INPUT_DIR}"
echo "[aiperf-weka] artifacts -> ${ARTIFACT_DIR}"
snap_metrics "BEFORE" > "$METRICS_OUT" 2>/dev/null || true

# Run as a child (not exec) so we can capture post-run metrics; do not let a nonzero
# aiperf exit abort the metrics capture under `set -e`.
set +e
aiperf "${ARGS[@]}"
rc=$?
set -e

snap_metrics "AFTER" >> "$METRICS_OUT" 2>/dev/null || true
echo "[aiperf-weka] --- Certus KV-offload counters after run ---"
snap_metrics "AFTER" | grep -iE 'kv_offload_total_bytes|external_prefix_cache' || true
echo "[aiperf-weka] full before/after snapshot: ${METRICS_OUT}"
echo "[aiperf-weka] aiperf summary: ${ARTIFACT_DIR}/profile_export_aiperf.json"
exit "$rc"
