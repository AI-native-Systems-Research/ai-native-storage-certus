#!/bin/bash
#
# config.sh - Shared configuration and helpers for the Certus multi-instance
#             launcher. Sourced by launch-servers.sh / run-benchmarks.sh /
#             stop-servers.sh. Not meant to be executed directly.
#
# One certus-server instance is launched per NVMe SSD. Each instance:
#   * binds a distinct gRPC port           (BASE_PORT + i)
#   * pins its NVMe poller to a dedicated   (--poller-base-cpu CORE)
#     physical core in the SSD's NUMA zone
#   * is optionally wrapped in `numactl` so the server threads and the
#     memory-tier DMA pool stay local to that same NUMA node.
#
set -euo pipefail

# --- Paths -------------------------------------------------------------------
# This script lives in <repo>/deploy/multi-instance/.
CONFIG_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$CONFIG_DIR/../.." && pwd)"

SERVER_BIN="${CERTUS_SERVER_BIN:-$REPO_ROOT/target/release/certus-server}"
BENCH_SCRIPT="${CERTUS_BENCH_SCRIPT:-$REPO_ROOT/apps/python/certus-api-bench.py}"
PYTHON="${CERTUS_PYTHON:-python3.12}"

# --- Tunables (override via environment) -------------------------------------
SESSION="${CERTUS_SESSION:-certus}"          # tmux session name
BASE_PORT="${CERTUS_BASE_PORT:-50051}"       # instance i listens on BASE_PORT+i
RUN_DIR="${CERTUS_RUNDIR:-/tmp/certus-multi-instance}"  # logs + instance map
INSTANCES_TSV="$RUN_DIR/instances.tsv"

# Number of leading physical cores to reserve (skip) on each NUMA node before
# allocating poller cores -- keeps core 0 etc. free for the OS / gRPC threads.
POLLER_RESERVE_CORES="${CERTUS_POLLER_RESERVE:-1}"

# Per-instance memory-tier pool size passed to certus-server (e.g. 2G, 512M).
# Empty => use the server's built-in default (2 GiB).
MEMORY_TIER_SIZE="${CERTUS_MEMORY_TIER_SIZE:-}"

# Bind each server's threads + memory to its SSD's NUMA node via numactl.
# 1 = on (default, if numactl is present), 0 = off.
USE_NUMACTL="${CERTUS_USE_NUMACTL:-1}"

# NVMe PCI class code (Mass Storage Controller, NVM Express).
NVME_CLASS_PREFIX="0x0108"

# --- Logging helpers ---------------------------------------------------------
log()  { printf '\033[1;34m[multi]\033[0m %s\n' "$*" >&2; }
warn() { printf '\033[1;33m[multi]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[multi]\033[0m %s\n' "$*" >&2; exit 1; }

# --- Topology helpers --------------------------------------------------------

# Expand a Linux cpulist ("0-3,8,12-13") into space-separated ids, preserving
# the kernel's enumeration order (physical cores typically precede their HT
# siblings, so allocating from the front favours full physical cores).
expand_cpulist() {
    local list="$1" seg lo hi
    local -a out=()
    IFS=',' read -ra segs <<< "$list"
    for seg in "${segs[@]}"; do
        if [[ "$seg" == *-* ]]; then
            lo="${seg%-*}"; hi="${seg#*-}"
            for ((c = lo; c <= hi; c++)); do out+=("$c"); done
        else
            out+=("$seg")
        fi
    done
    printf '%s\n' "${out[@]}"
}

# Ordered list of CPU ids belonging to a NUMA node.
node_core_list() {
    local node="$1"
    local f="/sys/devices/system/node/node${node}/cpulist"
    [[ -r "$f" ]] || die "cannot read $f"
    expand_cpulist "$(< "$f")"
}

# NUMA node of a PCI BDF (falls back to 0 for devices reporting -1).
numa_of_bdf() {
    local bdf="$1" n
    n="$(cat "/sys/bus/pci/devices/${bdf}/numa_node" 2>/dev/null || echo -1)"
    [[ "$n" == "-1" ]] && n=0
    echo "$n"
}

# PIDs of running certus-server instances launched by this tool. Matches on the
# executable name (comm) -- NOT a command-line substring -- so it can never
# match the calling shell, pkill itself, or an unrelated process that merely
# mentions "certus-server"; then keeps only those carrying --device-pci.
server_pids() {
    local pid
    for pid in $(pgrep -x certus-server 2>/dev/null || true); do
        grep -qa -- '--device-pci' "/proc/$pid/cmdline" 2>/dev/null && echo "$pid"
    done
}

# Discover all NVMe controllers currently bound to vfio-pci, one BDF per line,
# ordered by (numa_node, bdf) so node-0 drives come first.
discover_nvme_bdfs() {
    local dev_path bdf driver class
    {
        for dev_path in /sys/bus/pci/devices/*; do
            [[ -L "$dev_path/driver" ]] || continue
            driver="$(basename "$(readlink "$dev_path/driver")")"
            [[ "$driver" == "vfio-pci" ]] || continue
            class="$(cat "$dev_path/class" 2>/dev/null || echo)"
            [[ "$class" == ${NVME_CLASS_PREFIX}* ]] || continue
            bdf="$(basename "$dev_path")"
            printf '%s\t%s\n' "$(numa_of_bdf "$bdf")" "$bdf"
        done
    } | sort -k1,1n -k2,2 | cut -f2
}
