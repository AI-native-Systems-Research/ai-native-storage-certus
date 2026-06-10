#!/bin/bash
#
# launch-servers.sh - Launch N certus-server instances, one per NVMe SSD, each
#                     in its own tmux window.
#
# Each instance binds a distinct gRPC port (BASE_PORT + i) and pins its NVMe
# poller to a dedicated physical core located in the SSD's NUMA zone. When
# numactl is available the whole server is bound (cpu + memory) to that node.
#
# Usage:
#   ./launch-servers.sh [-n NUM] [-p BASE_PORT] [-s SESSION] [--format]
#                       [--mem SIZE] [--no-numactl] [BDF ...]
#
# Options:
#   -n NUM         Number of instances/SSDs to use (default: all discovered).
#   -p BASE_PORT   First gRPC port; instance i uses BASE_PORT+i (default 50051).
#   -s SESSION     tmux session name (default "certus").
#   --format       Pass --format to each server (DESTROYS existing data).
#   --mem SIZE     Per-instance memory-tier size (e.g. 2G, 512M).
#   --no-numactl   Do not wrap servers in numactl.
#   BDF ...        Explicit list of NVMe PCI addresses (overrides discovery).
#
# Environment overrides: see config.sh (CERTUS_SERVER_BIN, CERTUS_RUNDIR, ...).
#
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/config.sh"

# --- Parse arguments ---------------------------------------------------------
NUM=""
FORMAT_FLAG=""
declare -a EXPLICIT_BDFS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        -n) NUM="$2"; shift 2 ;;
        -p) BASE_PORT="$2"; shift 2 ;;
        -s) SESSION="$2"; shift 2 ;;
        --format) FORMAT_FLAG="--format"; shift ;;
        --mem) MEMORY_TIER_SIZE="$2"; shift 2 ;;
        --no-numactl) USE_NUMACTL=0; shift ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        -*) die "unknown option: $1" ;;
        *) EXPLICIT_BDFS+=("$1"); shift ;;
    esac
done

# --- Preflight ---------------------------------------------------------------
[[ -x "$SERVER_BIN" ]] || die "server binary not found at $SERVER_BIN
  build it with:  cargo build --release -p certus-server"
command -v tmux >/dev/null || die "tmux is required but not installed"

if [[ "$USE_NUMACTL" == 1 ]] && ! command -v numactl >/dev/null; then
    warn "numactl not found; launching without NUMA binding"
    USE_NUMACTL=0
fi

if tmux has-session -t "$SESSION" 2>/dev/null; then
    die "tmux session '$SESSION' already exists; run ./stop-servers.sh -s $SESSION first"
fi

# --- Select devices ----------------------------------------------------------
declare -a BDFS=()
if [[ ${#EXPLICIT_BDFS[@]} -gt 0 ]]; then
    BDFS=("${EXPLICIT_BDFS[@]}")
else
    mapfile -t BDFS < <(discover_nvme_bdfs)
fi
[[ ${#BDFS[@]} -gt 0 ]] || die "no NVMe devices bound to vfio-pci found
  bind them first, e.g.:  scripts/spdk-scripts/bind_vfio.sh"

if [[ -n "$NUM" ]]; then
    [[ "$NUM" =~ ^[0-9]+$ && "$NUM" -ge 1 ]] || die "-n must be a positive integer"
    [[ "$NUM" -le ${#BDFS[@]} ]] || die "-n $NUM requested but only ${#BDFS[@]} device(s) available"
    BDFS=("${BDFS[@]:0:NUM}")
fi

# --- Build per-instance plan -------------------------------------------------
# Allocate a distinct poller core per instance from its SSD's NUMA node,
# advancing a per-node cursor that starts after the reserved cores.
mkdir -p "$RUN_DIR"
: > "$INSTANCES_TSV"
declare -A NODE_CURSOR=()
declare -A NODE_CORES=()

log "Planning ${#BDFS[@]} instance(s):"
printf '  %-3s %-14s %-5s %-6s %s\n' "IDX" "BDF" "NUMA" "PORT" "POLLER_CPU" >&2

for i in "${!BDFS[@]}"; do
    bdf="${BDFS[$i]}"
    node="$(numa_of_bdf "$bdf")"
    port=$((BASE_PORT + i))

    # Cache the node's ordered core list once.
    if [[ -z "${NODE_CORES[$node]:-}" ]]; then
        NODE_CORES[$node]="$(node_core_list "$node" | tr '\n' ' ')"
        NODE_CURSOR[$node]=$POLLER_RESERVE_CORES
    fi
    read -ra cores <<< "${NODE_CORES[$node]}"
    idx=${NODE_CURSOR[$node]}
    [[ $idx -lt ${#cores[@]} ]] || die "ran out of cores on NUMA node $node for poller pinning"
    core=${cores[$idx]}
    NODE_CURSOR[$node]=$((idx + 1))

    printf '  %-3s %-14s %-5s %-6s %s\n' "$i" "$bdf" "$node" "$port" "$core" >&2
    printf '%s\t%s\t%s\t%s\t%s\n' "$i" "$bdf" "$node" "$port" "$core" >> "$INSTANCES_TSV"
done

# --- Launch tmux windows -----------------------------------------------------
log "Launching servers in tmux session '$SESSION' (logs in $RUN_DIR)"
[[ -n "$FORMAT_FLAG" ]] && warn "--format set: existing on-disk data will be DESTROYED"

while IFS=$'\t' read -r i bdf node port core; do
    logfile="$RUN_DIR/srv-$i.log"

    # Assemble the server command.
    cmd="'$SERVER_BIN' --device-pci '$bdf' --listen '0.0.0.0:$port' --poller-base-cpu $core"
    [[ -n "$MEMORY_TIER_SIZE" ]] && cmd="$cmd --memory-tier-size '$MEMORY_TIER_SIZE'"
    [[ -n "$FORMAT_FLAG" ]] && cmd="$cmd $FORMAT_FLAG"
    if [[ "$USE_NUMACTL" == 1 ]]; then
        cmd="numactl --cpunodebind=$node --membind=$node $cmd"
    fi
    # tee to a logfile; keep the pane alive after exit for inspection.
    full="$cmd 2>&1 | tee '$logfile'; printf '\\n[srv$i exited rc=%s]\\n' \${PIPESTATUS[0]}; exec bash"

    win="srv$i:$port"
    if [[ "$i" -eq 0 ]]; then
        tmux new-session -d -s "$SESSION" -n "$win" "$full"
    else
        tmux new-window -t "$SESSION" -n "$win" "$full"
    fi
done < "$INSTANCES_TSV"

# --- Wait for readiness ------------------------------------------------------
log "Waiting for servers to come up..."
TIMEOUT="${CERTUS_READY_TIMEOUT:-60}"
all_ready=1
while IFS=$'\t' read -r i bdf node port core; do
    logfile="$RUN_DIR/srv-$i.log"
    deadline=$((SECONDS + TIMEOUT))
    ready=0
    while [[ $SECONDS -lt $deadline ]]; do
        if grep -q "listening on" "$logfile" 2>/dev/null; then ready=1; break; fi
        # Genuine fatal: a Rust panic or a top-level "Error:" from main().
        # NOTE: ignore benign SPDK probe noise -- every SPDK process enumerates
        # all vfio devices and logs "Failed to open VFIO group" / "cannot be
        # used" for the ones its sibling instances already hold.
        if grep -qE "panicked|^Error:|[Ii]nit failed" "$logfile" 2>/dev/null; then break; fi
        sleep 0.5
    done
    if [[ $ready -eq 1 ]]; then
        log "  srv$i (port $port, $bdf) ready"
    else
        all_ready=0
        warn "  srv$i (port $port, $bdf) NOT ready -- check: tail -f $logfile"
    fi
done < "$INSTANCES_TSV"

echo >&2
if [[ $all_ready -eq 1 ]]; then
    log "All ${#BDFS[@]} server(s) ready."
else
    warn "Some servers did not report ready within ${TIMEOUT}s."
fi
cat >&2 <<EOF

  Instance map : $INSTANCES_TSV
  Attach tmux  : tmux attach -t $SESSION   (Ctrl-b n / Ctrl-b p to switch windows)
  Run clients  : ./run-benchmarks.sh -s $SESSION -- --clients 4 --num-objects 32
  Stop all     : ./stop-servers.sh -s $SESSION
EOF
