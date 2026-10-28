#!/bin/bash
# poll-kv-metrics.sh — sample a vLLM server's /metrics at a fixed interval and
# append one CSV row per sample, for a GPU-time-vs-KV-IO-time timeline. Run it in
# its own terminal (or background) ALONGSIDE a client such as
# run-guidellm-mha-iobound.sh; stop it with Ctrl-C or let DURATION expire, then
# plot the CSV with plot-gpu-vs-io.py.
#
# Each row holds the server's CUMULATIVE counters at that instant; plot-gpu-vs-io.py
# diffs consecutive rows to get per-interval seconds/bytes. Columns:
#   wall_s        seconds since this poller started
#   load_time_s   cum. KV load  time  (vllm:kv_offload_total_time CPU_to_GPU)  -- IO onto GPU
#   store_time_s  cum. KV store time  (                           GPU_to_CPU)  -- IO off GPU
#   load_bytes    cum. KV load  bytes
#   store_bytes   cum. KV store bytes
#   prefill_sum_s cum. request prefill time (scheduled->first token; contains the reload)
#   infer_sum_s   cum. request inference time (scheduled->last token)
#   prompt_tok    cum. prompt tokens (all sources: computed + cache hits)
#   gen_tok       cum. generation tokens
#   computed_tok  cum. prompt tokens actually computed on the GPU     (prompt_tokens_by_source local_compute)
#   local_hit_tok cum. prompt tokens served by the GPU prefix cache   (            "           local_cache_hit)
#   ext_hit_tok   cum. prompt tokens loaded from the offload tier     (            "           external_kv_transfer)
#
#   ./poll-kv-metrics.sh                                  # :8000, every 2s, until Ctrl-C
#   INTERVAL=1 DURATION=1800 ./poll-kv-metrics.sh         # 1s cadence, 30 min then stop
#   PORT=8001 OUT=/mnt/certus1/vllm-serve-matrix/kvio/run2.csv ./poll-kv-metrics.sh
set -euo pipefail

PORT="${PORT:-8000}"
TARGET="${TARGET:-http://127.0.0.1:${PORT}}"
METRICS_URL="${METRICS_URL:-${TARGET}/metrics}"
INTERVAL="${INTERVAL:-2}"           # seconds between samples
DURATION="${DURATION:-0}"           # 0 = run until Ctrl-C; else stop after this many seconds
OUT_DIR="${OUT_DIR:-/mnt/certus1/vllm-serve-matrix/kvio}"   # results on /mnt/certus1, never /home
OUT="${OUT:-${OUT_DIR}/kvio_$(date +%Y%m%d_%H%M%S).csv}"

command -v curl >/dev/null 2>&1 || { echo "error: curl not found" >&2; exit 1; }
mkdir -p "$(dirname "$OUT")"
if ! curl -fsS --max-time 5 "$METRICS_URL" >/dev/null 2>&1; then
  echo "error: cannot reach ${METRICS_URL} — is the server up?" >&2
  exit 1
fi

echo "wall_s,load_time_s,store_time_s,load_bytes,store_bytes,prefill_sum_s,infer_sum_s,prompt_tok,gen_tok,computed_tok,local_hit_tok,ext_hit_tok" > "$OUT"
echo "[poll] ${METRICS_URL} every ${INTERVAL}s -> ${OUT}  (Ctrl-C to stop)"

t0="$(date +%s.%N)"
while :; do
  now="$(date +%s.%N)"
  wall="$(awk -v a="$t0" -v b="$now" 'BEGIN{printf "%.3f", b-a}')"
  # One scrape, aggregate the counters we need. $NF is the Prometheus sample value.
  # Resilient scrape: /metrics can be slow under heavy load, so capture with a
  # generous timeout and never let a failed/empty scrape kill the loop (set -e +
  # a pipe would otherwise exit) or append a bogus all-zero row — just skip it.
  raw="$(curl -fsS --max-time 15 "$METRICS_URL" 2>/dev/null || true)"
  if [[ -z "$raw" ]]; then sleep "$INTERVAL"; continue; fi
  # Two naming schemes across vLLM/connector images:
  #   split  : vllm:kv_offload_load_time_total / _store_time_total / _load_bytes_total / _store_bytes_total
  #   labeled: vllm:kv_offload_total_time_total{transfer_type="CPU_to_GPU"|"GPU_to_CPU"} (+ _bytes_)
  # vLLM 0.30 emits both for the same transfers (equal for cputier; the labelled one
  # stays 0 for Certus), so take the split one and the labelled one only without it.
  # The [{ ] boundary stops the name matching the sibling _created series.
  printf '%s\n' "$raw" | awk -v wall="$wall" '
    /^vllm:kv_offload_load_time_total[{ ]/      { lt+=$NF; split_seen=1 }
    /^vllm:kv_offload_store_time_total[{ ]/     { st+=$NF; split_seen=1 }
    /^vllm:kv_offload_load_bytes_total[{ ]/     { lb+=$NF; split_seen=1 }
    /^vllm:kv_offload_store_bytes_total[{ ]/    { sb+=$NF; split_seen=1 }
    /^vllm:kv_offload_total_time_total[{ ]/     { if ($0 ~ /CPU_to_GPU/) llt+=$NF; else if ($0 ~ /GPU_to_CPU/) lst+=$NF }
    /^vllm:kv_offload_total_bytes_total[{ ]/    { if ($0 ~ /CPU_to_GPU/) llb+=$NF; else if ($0 ~ /GPU_to_CPU/) lsb+=$NF }
    /^vllm:request_prefill_time_seconds_sum[{ ]/   { pf+=$NF }
    /^vllm:request_inference_time_seconds_sum[{ ]/ { inf+=$NF }
    /^vllm:prompt_tokens_total[{ ]/              { pt+=$NF }
    /^vllm:generation_tokens_total[{ ]/          { gt+=$NF }
    /^vllm:prompt_tokens_by_source_total[{ ]/    { if ($0 ~ /"local_compute"/) ct+=$NF; else if ($0 ~ /"local_cache_hit"/) lh+=$NF; else if ($0 ~ /"external_kv_transfer"/) eh+=$NF }
    END { if (!split_seen) { lt=llt; st=lst; lb=llb; sb=lsb }
          printf "%s,%.6f,%.6f,%.0f,%.0f,%.6f,%.6f,%.0f,%.0f,%.0f,%.0f,%.0f\n", wall, lt, st, lb, sb, pf, inf, pt, gt, ct, lh, eh }
  ' >> "$OUT"

  if [[ "$DURATION" != "0" ]] && awk -v w="$wall" -v d="$DURATION" 'BEGIN{exit !(w>=d)}'; then
    echo "[poll] reached DURATION=${DURATION}s, stopping -> ${OUT}"
    break
  fi
  sleep "$INTERVAL"
done
echo "[poll] wrote ${OUT}; plot with: ./plot-gpu-vs-io.py ${OUT}"
