#!/bin/sh
# Start this host's stress-run workload node agents, one per NUMA domain, and do
# not return until each is LISTENING.
#
# One agent per Certus instance, pinned to the same NUMA domain as the server it
# attaches to. The agents exit shortly after the generator disconnects (a linger),
# so this is re-run before every measured run rather than left up.
#
# Hostnames are never hardcoded: each domain's GPU is resolved from the PCI
# topology, so a host with one GPU, two GPUs, or GPUs on unexpected nodes all
# work without editing this script.
#
# ENVIRONMENT
#   DOMAINS      NUMA domains to start (default: those with a Certus mailbox present)
#   PROBE        --probe mode (default lookup; see below -- this choice IS the experiment)
#   BLOCK_BYTES  --block-bytes (default 2097152, i.e. 2 MiB)
#   BATCH_KEYS   --batch-keys (default 64)
#   PORT_BASE    first agent port (default 7420; domain n uses BASE+n)
#   GPU_<n>      override the GPU index for domain <n>
#   BIN          path to workload-node-agent
#
# WHY --probe MATTERS: `lookup` is the only mode that reaches remote lookup at
# all, so a run with `check` is a different experiment, not a cheaper one.
#
# TRAPS THIS EXISTS TO AVOID
#
#   * A process-presence check races a linger-shutdown. An agent visible to `ps`
#     may be seconds from exiting, so wait on the LISTEN state instead.
#   * NEVER TCP-probe an agent to test reachability: the probe counts as a
#     generator connection, and the linger then exits the agent out from under
#     the run you were about to start.
#   * Do not sleep a guess after launch. The agent attaches its mailbox and
#     allocates a device buffer first, and a generator that connects too early
#     sees ECONNREFUSED and aborts the whole run.
#   * A cross-NUMA GPU is a legitimate configuration, not an error -- a host with
#     one GPU must serve both domains, and that asymmetry belongs in the report
#     rather than in a refusal. This script says which domains are cross-NUMA.
set -e

R="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$R/target/release/workload-node-agent}"
PORT_BASE="${PORT_BASE:-7420}"

[ -x "$BIN" ] || { echo "no workload-node-agent at $BIN. Build with: cargo build --release -p workload-gen -p workload-node-agent" >&2; exit 2; }

# --- GPU topology ----------------------------------------------------------
# Map each visible GPU to the NUMA node its PCI function reports. nvidia-smi
# gives index and BDF; sysfs gives the node. No hostname table.
gpu_node_pairs() {
    nvidia-smi --query-gpu=index,pci.bus_id --format=csv,noheader 2>/dev/null \
    | while IFS=, read -r idx bdf; do
        idx="$(echo "$idx" | tr -d ' ')"
        bdf="$(echo "$bdf" | tr -d ' ' | tr 'A-Z' 'a-z')"
        # nvidia-smi prints 00000000:41:00.0; sysfs wants 0000:41:00.0
        short="${bdf#????}"
        n="$(cat "/sys/bus/pci/devices/$short/numa_node" 2>/dev/null || echo -1)"
        echo "$idx $n"
      done
}

GPUMAP="$(gpu_node_pairs)"
[ -n "$GPUMAP" ] || { echo "no GPU visible to nvidia-smi; the agent needs one" >&2; exit 2; }

gpu_for_node() {
    node="$1"
    eval "override=\${GPU_$node:-}"
    if [ -n "$override" ]; then echo "$override"; return; fi
    # Prefer a GPU on this node; otherwise the lowest-indexed one, cross-NUMA.
    local_gpu="$(echo "$GPUMAP" | awk -v n="$node" '$2==n{print $1; exit}')"
    if [ -n "$local_gpu" ]; then echo "$local_gpu"; return; fi
    echo "$GPUMAP" | awk 'NR==1{print $1}'
}

is_local_gpu() {
    echo "$GPUMAP" | awk -v g="$1" -v n="$2" '$1==g && $2==n{found=1} END{exit !found}'
}

# Default to the domains that actually have a Certus mailbox to attach to.
default_domains() {
    for m in /dev/shm/certus-stress-n*; do
        [ -e "$m" ] || continue
        b="$(basename "$m")"
        printf '%s ' "${b##*-n}"
    done
}
DOMAINS="${DOMAINS:-$(default_domains)}"
[ -n "$DOMAINS" ] || { echo "no /dev/shm/certus-stress-n* mailbox found; start the servers first" >&2; exit 2; }

listening() { ss -ltn 2>/dev/null | grep -q ":$1 "; }

for n in $DOMAINS; do
    port=$((PORT_BASE + n))
    gpu="$(gpu_for_node "$n")"
    log="/tmp/agent-n$n.log"
    if listening "$port"; then
        echo "numa$n: port $port already listening, left alone"
        continue
    fi
    setsid numactl --cpunodebind="$n" --membind="$n" "$BIN" \
        --port "$port" \
        --shm-path "/dev/shm/certus-stress-n$n" \
        --block-bytes "${BLOCK_BYTES:-2097152}" \
        --batch-keys "${BATCH_KEYS:-64}" \
        --probe "${PROBE:-lookup}" \
        --gpu-device "$gpu" \
        > "$log" 2>&1 &
    pid=$!
    # Wait for LISTEN rather than sleeping a guess -- see the traps above.
    i=0
    while ! listening "$port"; do
        i=$((i + 1))
        if [ "$i" -gt 100 ]; then
            echo "numa$n: port $port never came up; see $log" >&2
            exit 1
        fi
        kill -0 "$pid" 2>/dev/null || { echo "numa$n: agent exited; see $log" >&2; exit 1; }
        sleep 0.2
    done
    if is_local_gpu "$gpu" "$n"; then
        where="NUMA-local"
    else
        where="CROSS-NUMA (this host has no GPU on node $n)"
    fi
    echo "numa$n: pid $pid port $port gpu $gpu $where (log $log)"
done
