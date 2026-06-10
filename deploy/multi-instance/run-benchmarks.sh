#!/bin/bash
#
# run-benchmarks.sh - Run one certus-api-bench client per running server
#                     instance (in parallel) and aggregate the results.
#
# Reads the instance map written by launch-servers.sh, fires a benchmark client
# at each server's gRPC port concurrently, then parses the per-phase aggregate
# throughput (Populate / Lookup hot / Lookup cold) from every client and prints
# a per-instance breakdown plus the system-wide totals.
#
# Usage:
#   ./run-benchmarks.sh [-s SESSION] [--no-gpu-affinity] [-- BENCH_ARGS...]
#
# Anything after `--` is forwarded verbatim to each certus-api-bench.py client,
# e.g.  ./run-benchmarks.sh -- --clients 8 --num-objects 32 --iterations 20
#
# GPU affinity: by default client i is pinned to GPU (i % num_gpus) via
# CUDA_VISIBLE_DEVICES; pass --no-gpu-affinity to let every client use GPU 0.
#
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/config.sh"

GPU_AFFINITY=1
declare -a BENCH_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        -s) SESSION="$2"; shift 2 ;;
        --no-gpu-affinity) GPU_AFFINITY=0; shift ;;
        --) shift; BENCH_ARGS=("$@"); break ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) die "unknown option: $1 (use -- to pass benchmark args)" ;;
    esac
done

[[ -f "$INSTANCES_TSV" ]] || die "instance map $INSTANCES_TSV not found; run ./launch-servers.sh first"
[[ -f "$BENCH_SCRIPT" ]] || die "benchmark script not found at $BENCH_SCRIPT"
command -v "$PYTHON" >/dev/null || die "$PYTHON not found (set CERTUS_PYTHON)"

# Provide a sensible default workload if the caller passed none.
if [[ ${#BENCH_ARGS[@]} -eq 0 ]]; then
    BENCH_ARGS=(--clients 4 --num-objects 32 --iterations 10 --block-size 4M)
    warn "no benchmark args given; using defaults: ${BENCH_ARGS[*]}"
fi

# Detect GPU count for round-robin affinity.
NUM_GPUS=0
if [[ "$GPU_AFFINITY" == 1 ]] && command -v nvidia-smi >/dev/null; then
    NUM_GPUS="$(nvidia-smi -L 2>/dev/null | wc -l)"
fi
[[ "$NUM_GPUS" -ge 1 ]] || GPU_AFFINITY=0

BENCH_DIR="$(dirname "$BENCH_SCRIPT")"
mapfile -t ROWS < "$INSTANCES_TSV"
N=${#ROWS[@]}
[[ $N -gt 0 ]] || die "instance map is empty"

log "Running $N benchmark client(s) in parallel: ${BENCH_ARGS[*]}"

declare -a PIDS=()
declare -a PORTS=()
declare -a OUTS=()

for row in "${ROWS[@]}"; do
    IFS=$'\t' read -r i bdf node port core <<< "$row"
    out="$RUN_DIR/bench-$i.log"
    PORTS+=("$port")
    OUTS+=("$out")

    gpu_env=()
    if [[ "$GPU_AFFINITY" == 1 ]]; then
        gpu_env=(env "CUDA_VISIBLE_DEVICES=$((i % NUM_GPUS))")
        log "  client $i -> localhost:$port (GPU $((i % NUM_GPUS)))"
    else
        log "  client $i -> localhost:$port"
    fi

    # Each client runs from the bench script's dir so its pb2 stubs import.
    ( cd "$BENCH_DIR" && "${gpu_env[@]}" "$PYTHON" "$BENCH_SCRIPT" \
        --server "localhost:$port" "${BENCH_ARGS[@]}" ) > "$out" 2>&1 &
    PIDS+=("$!")
done

# --- Wait for all clients ----------------------------------------------------
fail=0
for idx in "${!PIDS[@]}"; do
    if ! wait "${PIDS[$idx]}"; then
        warn "client $idx (port ${PORTS[$idx]}) exited non-zero -- see ${OUTS[$idx]}"
        fail=1
    fi
done

# --- Parse aggregate throughput per phase ------------------------------------
# Mirrors bench_devices_sweep.py: locate the stats line for a phase label, then
# read the "aggregate=<X> GB/s" value on the following line.
parse_agg() {  # <logfile> <label>
    awk -v lbl="$2" '
        index($0, lbl) && /us/ { f = 1; next }
        f && /aggregate=/ {
            n = $0
            sub(/.*aggregate=[ ]*/, "", n)
            sub(/[ ]*GB.*/, "", n)
            print (n + 0); exit
        }
    ' "$1" 2>/dev/null
}

log "Aggregating results..."
echo
printf '%-5s %-16s %12s %12s %12s\n' "IDX" "ENDPOINT" "POPULATE" "HOT" "COLD"
printf '%-5s %-16s %12s %12s %12s\n' "---" "----------------" "------------" "------------" "------------"

sum_pop=0; sum_hot=0; sum_cold=0; counted=0
for idx in "${!OUTS[@]}"; do
    out="${OUTS[$idx]}"; port="${PORTS[$idx]}"
    pop="$(parse_agg "$out" "Populate")"
    hot="$(parse_agg "$out" "Lookup (hot)")"
    cold="$(parse_agg "$out" "Lookup (cold)")"
    pop="${pop:-0}"; hot="${hot:-0}"; cold="${cold:-0}"
    printf '%-5s %-16s %9.2f GB/s %9.2f GB/s %9.2f GB/s\n' \
        "$idx" "localhost:$port" "$pop" "$hot" "$cold"
    read -r sum_pop sum_hot sum_cold <<< "$(awk -v a="$sum_pop" -v b="$sum_hot" -v c="$sum_cold" \
        -v p="$pop" -v h="$hot" -v d="$cold" 'BEGIN{printf "%.6f %.6f %.6f", a+p, b+h, c+d}')"
    counted=$((counted + 1))
done

printf '%-5s %-16s %12s %12s %12s\n' "---" "----------------" "------------" "------------" "------------"
printf '%-5s %-16s %9.2f GB/s %9.2f GB/s %9.2f GB/s\n' \
    "ALL" "$counted instance(s)" "$sum_pop" "$sum_hot" "$sum_cold"
echo
log "Per-client logs: $RUN_DIR/bench-*.log"

exit "$fail"
