#!/usr/bin/env bash
#
# run-certus-shmq-server.sh - Launch certus-server-yaml on NUMA node 0 with the
#                             4-drive shmq memory-tier configuration used for the
#                             KV-offload replay benchmarks.
#
# Usage:
#   ./run-certus-shmq-server.sh [--memory-tier-size SIZE]
#                               [--eviction-threshold FRAC]
#                               [--format]
#
# Options:
#   --memory-tier-size SIZE   Memory-tier pool size (default: 32G).
#   --eviction-threshold FRAC Memory-tier eviction threshold, 0.0-1.0
#                             (default: 0.9).
#   --format                  Pass --format to the server. DESTROYS existing
#                             on-disk data.
#   -h, --help                Show this help.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- Defaults ----------------------------------------------------------------
NUMA_NODE=0
SHM_PATH="/dev/shm/certus-shmq"
MEMORY_TIER_SIZE="60G"
EVICTION_THRESHOLD="0.9"
STORE_BACKPRESSURE_MS=5000
CHANNELS=64
POLLER_BASE_CPU=2
SHMQ_POLLER_CPU=6
FORMAT_FLAG=""

DEVICE_PCI=(
    0000:61:00.0
    0000:62:00.0
    0000:63:00.0
    0000:64:00.0
)

SERVER_BIN="$REPO_ROOT/target/release/certus-server-yaml"

# --- Parse arguments ---------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --memory-tier-size)   MEMORY_TIER_SIZE="$2"; shift 2 ;;
        --eviction-threshold) EVICTION_THRESHOLD="$2"; shift 2 ;;
        --format)             FORMAT_FLAG="--format"; shift ;;
        -h|--help)            sed -n '2,21p' "$0"; exit 0 ;;
        *)                    echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

log() { printf '\033[1;34m[certus-shmq]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[certus-shmq]\033[0m %s\n' "$*" >&2; exit 1; }

[[ -x "$SERVER_BIN" ]] || die "Server binary not found: $SERVER_BIN (build with build-certus-server.sh)"

# --- Launch Server -----------------------------------------------------------
SERVER_CMD=(
    numactl --cpunodebind="$NUMA_NODE" --membind="$NUMA_NODE"
    "$SERVER_BIN"
)
for pci in "${DEVICE_PCI[@]}"; do
    SERVER_CMD+=(--device-pci "$pci")
done
SERVER_CMD+=(
    --shm-path "$SHM_PATH"
    --memory-tier-size "$MEMORY_TIER_SIZE"
    --memory-tier-eviction-threshold "$EVICTION_THRESHOLD"
    --store-backpressure-ms "$STORE_BACKPRESSURE_MS"
    --channels "$CHANNELS"
    --poller-base-cpu "$POLLER_BASE_CPU"
    --shmq-poller-cpu "$SHMQ_POLLER_CPU"
)
[[ -n "$FORMAT_FLAG" ]] && SERVER_CMD+=("$FORMAT_FLAG")

log "memory-tier=$MEMORY_TIER_SIZE eviction-threshold=$EVICTION_THRESHOLD${FORMAT_FLAG:+ (format)}"
log "cmd: ${SERVER_CMD[*]}"
exec "${SERVER_CMD[@]}"
