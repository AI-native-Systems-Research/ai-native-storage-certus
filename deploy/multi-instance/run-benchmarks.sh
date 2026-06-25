#!/bin/bash
#
# run-benchmarks.sh - Run one or more certus-api-bench clients per running
#                     server instance (in parallel) and aggregate the results.
#
# Reads the instance map written by launch-servers.sh and fires benchmark
# client(s) at each server's gRPC port concurrently, then parses the per-phase
# aggregate throughput (Populate / Lookup hot / Lookup cold) from every client,
# sums each instance's clients together, and prints a per-instance breakdown
# plus the system-wide totals.
#
# Usage:
#   ./run-benchmarks.sh [-s SESSION] [-c N] [--no-gpu-affinity] [-- BENCH_ARGS...]
#
#   -c N, --clients-per-server N
#       Launch N benchmark client *processes* against each server instance
#       (default 1). This is distinct from the bench script's own --clients
#       flag, which sets the number of threads *within* a single process; total
#       concurrency per server = N processes x (forwarded --clients threads).
#
# Anything after `--` is forwarded verbatim to each certus-api-bench.py client,
# e.g.  ./run-benchmarks.sh -c 2 -- --clients 8 --num-objects 32 --iterations 20
#
# GPU affinity (via CUDA_VISIBLE_DEVICES):
#   default            pick a GPU in the SAME NUMA zone as the server instance
#                      (round-robin among that node's GPUs); falls back to a
#                      global round-robin if a node has no local GPU
#   --gpu-spread       spread clients round-robin across ALL GPUs (ignore NUMA)
#   --gpu N            pin ALL clients to GPU N
#   --no-gpu-affinity  set nothing; every client uses its default device (GPU 0)
#
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/config.sh"

GPU_AFFINITY=1
GPU_MODE="numa"          # numa | spread | fixed
FIXED_GPU=""
PER_SERVER=1
declare -a BENCH_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        -s) SESSION="$2"; shift 2 ;;
        -c|--clients-per-server) PER_SERVER="$2"; shift 2 ;;
        --gpu) FIXED_GPU="$2"; GPU_MODE="fixed"; shift 2 ;;
        --gpu-spread) GPU_MODE="spread"; shift ;;
        --no-gpu-affinity) GPU_AFFINITY=0; shift ;;
        --) shift; BENCH_ARGS=("$@"); break ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) die "unknown option: $1 (use -- to pass benchmark args)" ;;
    esac
done

[[ "$PER_SERVER" =~ ^[0-9]+$ && "$PER_SERVER" -ge 1 ]] || die "-c must be a positive integer"
[[ -z "$FIXED_GPU" || "$FIXED_GPU" =~ ^[0-9]+$ ]] || die "--gpu must be a non-negative integer"
[[ -f "$INSTANCES_TSV" ]] || die "instance map $INSTANCES_TSV not found; run ./launch-servers.sh first"
[[ -f "$BENCH_SCRIPT" ]] || die "benchmark script not found at $BENCH_SCRIPT"
command -v "$PYTHON" >/dev/null || die "$PYTHON not found (set CERTUS_PYTHON)"

# Provide a sensible default workload if the caller passed none.
if [[ ${#BENCH_ARGS[@]} -eq 0 ]]; then
    BENCH_ARGS=(--clients 4 --num-objects 32 --iterations 10 --block-size 4M)
    warn "no benchmark args given; using defaults: ${BENCH_ARGS[*]}"
fi

# Enumerate GPUs and their NUMA nodes (needed for numa/spread modes; a fixed
# --gpu pin does not require enumeration).
declare -A NODE_GPUS=()       # numa node -> space-separated GPU indices
declare -a ALL_GPUS=()        # all GPU indices, ascending
if [[ "$GPU_AFFINITY" == 1 && "$GPU_MODE" != "fixed" ]]; then
    while IFS=$'\t' read -r gn gi; do
        [[ -n "$gi" ]] || continue
        NODE_GPUS[$gn]="${NODE_GPUS[$gn]:-}${NODE_GPUS[$gn]:+ }$gi"
    done < <(gpu_numa_map)
    mapfile -t ALL_GPUS < <(gpu_numa_map | cut -f2 | sort -n)
    if [[ ${#ALL_GPUS[@]} -lt 1 ]]; then
        warn "no GPUs detected; clients will use default device"
        GPU_AFFINITY=0
    elif [[ "$GPU_MODE" == "numa" && ${#ALL_GPUS[@]} -eq 1 ]]; then
        # Only one GPU: NUMA locality is moot, every client uses it.
        GPU_MODE="spread"
    fi
fi

# Pick the GPU for a client on the given NUMA node. Round-robins among the GPUs
# local to that node (numa mode), or across all GPUs (spread / fallback).
declare -A NODE_GPU_CURSOR=()
warned_no_local_gpu=0
pick_gpu() {  # <numa_node>  -> echoes GPU index
    local node="$1" gpu
    if [[ "$GPU_MODE" == "numa" ]]; then
        local -a local_gpus=()
        read -ra local_gpus <<< "${NODE_GPUS[$node]:-}"
        if [[ ${#local_gpus[@]} -gt 0 ]]; then
            local cur="${NODE_GPU_CURSOR[$node]:-0}"
            gpu="${local_gpus[$((cur % ${#local_gpus[@]}))]}"
            NODE_GPU_CURSOR[$node]=$((cur + 1))
            echo "$gpu"; return
        fi
        if [[ "$warned_no_local_gpu" == 0 ]]; then
            warn "no GPU local to NUMA node $node; falling back to global round-robin"
            warned_no_local_gpu=1
        fi
    fi
    # spread mode or numa fallback: global round-robin over all GPUs.
    echo "${ALL_GPUS[$((gpu_idx % ${#ALL_GPUS[@]}))]}"
}

BENCH_DIR="$(dirname "$BENCH_SCRIPT")"
mapfile -t ROWS < "$INSTANCES_TSV"
N=${#ROWS[@]}
[[ $N -gt 0 ]] || die "instance map is empty"

TOTAL=$((N * PER_SERVER))

# Pre-flight: a server that failed to start (e.g. out of hugepages) leaves its
# port unreachable; without this check the client just yields a misleading
# 0.00 GB/s. Warn up front so the cause is obvious.
down=0
for row in "${ROWS[@]}"; do
    IFS=$'\t' read -r i bdf node port core <<< "$row"
    if ! port_listening "$port"; then
        warn "instance $i endpoint localhost:$port NOT reachable -- server likely failed to start (see $RUN_DIR/srv-$i.log)"
        down=$((down + 1))
    fi
done
[[ $down -eq 0 ]] || warn "$down of $N endpoint(s) unreachable; their results will be reported as FAILED"

log "Running $TOTAL client process(es) = $N instance(s) x $PER_SERVER per server: ${BENCH_ARGS[*]}"

T_BENCH_START="$(date +%s.%N)"

# Per-job (flat) tracking arrays.
declare -a PIDS=() J_INST=() J_PORT=() J_OUT=()
gpu_idx=0

for row in "${ROWS[@]}"; do
    IFS=$'\t' read -r i bdf node port core <<< "$row"
    for ((r = 0; r < PER_SERVER; r++)); do
        out="$RUN_DIR/bench-$i-$r.log"

        gpu_env=()
        if [[ "$GPU_AFFINITY" == 1 ]]; then
            if [[ "$GPU_MODE" == "fixed" ]]; then
                gpu="$FIXED_GPU"
            else
                gpu="$(pick_gpu "$node")"
            fi
            gpu_env=(env "CUDA_VISIBLE_DEVICES=$gpu")
            log "  instance $i (NUMA $node) client $r -> localhost:$port (GPU $gpu)"
        else
            log "  instance $i (NUMA $node) client $r -> localhost:$port"
        fi
        gpu_idx=$((gpu_idx + 1))

        # Each client runs from the bench script's dir so its pb2 stubs import.
        ( cd "$BENCH_DIR" && "${gpu_env[@]}" "$PYTHON" "$BENCH_SCRIPT" \
            --server "localhost:$port" "${BENCH_ARGS[@]}" ) > "$out" 2>&1 &
        PIDS+=("$!")
        J_INST+=("$i")
        J_PORT+=("$port")
        J_OUT+=("$out")
    done
done

# --- Wait for all clients ----------------------------------------------------
fail=0
for j in "${!PIDS[@]}"; do
    if ! wait "${PIDS[$j]}"; then
        warn "instance ${J_INST[$j]} client (port ${J_PORT[$j]}) exited non-zero -- see ${J_OUT[$j]}"
        fail=1
    fi
done

T_BENCH_END="$(date +%s.%N)"
BENCH_WALL="$(awk "BEGIN{printf \"%.3f\", $T_BENCH_END - $T_BENCH_START}")"

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
# Parse "Total wall time: <X>s" from a bench log.
parse_wall_time() {  # <logfile>
    awk '/Total wall time:/ { sub(/.*: */, ""); sub(/s.*/, ""); print ($0 + 0); exit }' "$1" 2>/dev/null
}
# Parse "Total per client: <N> objects" from a bench log.
parse_total_objects() {  # <logfile>
    awk '/Total per client:/ { for(i=1;i<=NF;i++) if($i+0>0){print $i+0; exit} }' "$1" 2>/dev/null
}
# Parse "Block size: <N> MiB" from a bench log.
parse_block_mib() {  # <logfile>
    awk '/Block size:/ { for(i=1;i<=NF;i++) if($i ~ /^[0-9]+$/ && $(i+1)=="MiB"){print $i+0; exit} }' "$1" 2>/dev/null
}
fadd() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.6f", a + b }'; }

# Accumulate each client's throughput into its parent instance. Track whether
# any replica produced parseable data: an instance with none (server down /
# connection refused) is reported as FAILED rather than a misleading 0.00.
declare -A INST_POP=() INST_HOT=() INST_COLD=() INST_PORT=() INST_NCLI=() INST_DATA=()
for j in "${!J_OUT[@]}"; do
    i="${J_INST[$j]}"
    pop_raw="$(parse_agg "${J_OUT[$j]}" "Populate")"
    hot_raw="$(parse_agg "${J_OUT[$j]}" "Lookup (hot)")"
    cold_raw="$(parse_agg "${J_OUT[$j]}" "Lookup (cold)")"
    [[ -n "${pop_raw}${hot_raw}${cold_raw}" ]] && INST_DATA[$i]=1
    pop="${pop_raw:-0}"; hot="${hot_raw:-0}"; cold="${cold_raw:-0}"
    INST_PORT[$i]="${J_PORT[$j]}"
    INST_NCLI[$i]=$(( ${INST_NCLI[$i]:-0} + 1 ))
    INST_POP[$i]="$(fadd "${INST_POP[$i]:-0}" "$pop")"
    INST_HOT[$i]="$(fadd "${INST_HOT[$i]:-0}" "$hot")"
    INST_COLD[$i]="$(fadd "${INST_COLD[$i]:-0}" "$cold")"
done

log "Aggregating results..."
echo
printf '%-5s %-16s %5s %12s %12s %12s\n' "IDX" "ENDPOINT" "NCLI" "POPULATE" "HOT" "COLD"
printf '%-5s %-16s %5s %12s %12s %12s\n' "---" "----------------" "-----" "------------" "------------" "------------"

sum_pop=0; sum_hot=0; sum_cold=0; counted=0; failed=0
# Iterate instances in map order for deterministic output.
for row in "${ROWS[@]}"; do
    IFS=$'\t' read -r i bdf node port core <<< "$row"
    [[ -n "${INST_PORT[$i]:-}" ]] || continue
    if [[ "${INST_DATA[$i]:-0}" != 1 ]]; then
        # No parseable throughput from any replica: distinguish "unreachable"
        # (connection refused in the client log) from generic "no data".
        reason="no data"
        grep -q "Connection refused" "$RUN_DIR/bench-$i-0.log" 2>/dev/null && reason="unreachable"
        printf '%-5s %-16s %5s %14s %14s %14s   (%s)\n' \
            "$i" "localhost:${INST_PORT[$i]}" "${INST_NCLI[$i]}" "FAILED" "FAILED" "FAILED" "$reason"
        failed=$((failed + 1)); fail=1
        continue
    fi
    pop="${INST_POP[$i]}"; hot="${INST_HOT[$i]}"; cold="${INST_COLD[$i]}"
    printf '%-5s %-16s %5s %9.2f GB/s %9.2f GB/s %9.2f GB/s\n' \
        "$i" "localhost:${INST_PORT[$i]}" "${INST_NCLI[$i]}" "$pop" "$hot" "$cold"
    sum_pop="$(fadd "$sum_pop" "$pop")"
    sum_hot="$(fadd "$sum_hot" "$hot")"
    sum_cold="$(fadd "$sum_cold" "$cold")"
    counted=$((counted + 1))
done

printf '%-5s %-16s %5s %12s %12s %12s\n' "---" "----------------" "-----" "------------" "------------" "------------"
printf '%-5s %-16s %5s %9.2f GB/s %9.2f GB/s %9.2f GB/s\n' \
    "SUM" "$counted ok$([[ $failed -gt 0 ]] && echo " / $failed FAILED")" "$TOTAL" "$sum_pop" "$sum_hot" "$sum_cold"

# Compute effective system throughput from wall-clock elapsed time.
# Instances sharing a GPU cannot exceed its PCIe bandwidth; the SUM above
# overcounts in that case. EFFECTIVE = total bytes / wall-clock (all phases
# sequential within each client, but clients run concurrently).
total_objs=0; block_mib=4
for j in "${!J_OUT[@]}"; do
    objs="$(parse_total_objects "${J_OUT[$j]}")"
    bm="$(parse_block_mib "${J_OUT[$j]}")"
    [[ -n "$objs" ]] && total_objs=$((total_objs + objs))
    [[ -n "$bm" ]] && block_mib="$bm"
done
if [[ "$total_objs" -gt 0 && "$(awk "BEGIN{print ($BENCH_WALL > 0)}")" == 1 ]]; then
    total_gib="$(awk "BEGIN{printf \"%.6f\", $total_objs * $block_mib / 1024}")"
    eff_gbps="$(awk "BEGIN{printf \"%.2f\", $total_gib / $BENCH_WALL}")"
    printf '%-5s %-16s %5s %38s\n' \
        "EFF" "wall=${BENCH_WALL}s" "" "${eff_gbps} GB/s effective (all phases, wall-clock)"
fi

# Flag if SUM hot exceeds plausible GPU PCIe bandwidth (~32 GB/s per GPU).
gpu_bw_limit=32
if awk "BEGIN{exit !($sum_hot > $gpu_bw_limit)}" 2>/dev/null; then
    warn "SUM hot lookup ($(printf '%.1f' "$sum_hot") GB/s) exceeds single-GPU PCIe bandwidth (~${gpu_bw_limit} GB/s)."
    warn "Per-instance numbers reflect pipelined throughput with shared GPU contention."
    warn "Use CERTUS_BENCH_SCRIPT=.../certus-api-bench.py for sequential (non-pipelined) measurement."
fi
echo
if [[ $failed -gt 0 ]]; then
    warn "$failed of $N endpoint(s) produced no data -- servers down or unreachable. Check $RUN_DIR/srv-*.log"
fi

# Check server logs for memory-tier exhaustion warnings.
mt_exhausted=0
for row in "${ROWS[@]}"; do
    IFS=$'\t' read -r i bdf node port core <<< "$row"
    srvlog="$RUN_DIR/srv-$i.log"
    if [[ -f "$srvlog" ]] && grep -q "memory-tier exhausted" "$srvlog"; then
        mt_exhausted=$((mt_exhausted + 1))
        if [[ $mt_exhausted -eq 1 ]]; then
            echo
            warn "MEMORY-TIER EXHAUSTED on one or more instances:"
        fi
        warn "  instance $i ($srvlog): $(grep 'memory-tier exhausted' "$srvlog" | head -1)"
    fi
done
if [[ $mt_exhausted -gt 0 ]]; then
    warn "Throughput results may be degraded. Increase --memory-tier-size or reduce load."
    fail=1
fi

log "Per-client logs: $RUN_DIR/bench-*.log"

exit "$fail"
