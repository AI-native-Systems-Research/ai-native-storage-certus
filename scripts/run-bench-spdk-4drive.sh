#!/usr/bin/env bash
#
# run-bench-spdk-4drive.sh - Build the SPDK server, launch it with 4 drives on
#                            NUMA node 0, then run certus-api-bench_v2 with
#                            64 objects and 1 client.
# To mount hugetlbfs:
#   sudo mkdir -p /dev/hugepages
#   sudo mount -t hugetlbfs nodev /dev/hugepages
# Usage:
#   ./run-bench-spdk-4drive.sh [--format] [--mem SIZE] [--server-only] [--bench-only]
#
# Options:
#   --format       Pass --format to the server (DESTROYS existing on-disk data).
#   --mem SIZE     Memory-tier pool size (default: 4G).
#   --server-only  Build and start the server, but skip the benchmark.
#   --bench-only   Skip build and server launch; run only the benchmark
#                  (assumes server is already running on localhost:50051).
#   --no-build     Skip the cargo build step (use existing binary).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- Defaults ----------------------------------------------------------------
DRIVE_COUNT=4
NUMA_NODE=0
LISTEN="0.0.0.0:50051"
MEMORY_TIER_SIZE="4G"
FORMAT_FLAG=""
SERVER_ONLY=0
BENCH_ONLY=0
NO_BUILD=0

# Benchmark defaults
NUM_OBJECTS=64
CLIENTS=1
ITERATIONS=10
PIPELINE_DEPTH=4
BLOCK_SIZE="4M"
WRITES_SETTLE=30

PYTHON="${CERTUS_PYTHON:-python3}"
SERVER_BIN="$REPO_ROOT/target/release/certus-server-yaml"
BENCH_SCRIPT="$REPO_ROOT/apps/python/certus-api-bench_v2.py"

# --- Parse arguments ---------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --format)       FORMAT_FLAG="--format"; shift ;;
        --drives)       DRIVE_COUNT="$2"; shift 2 ;;
        --mem)          MEMORY_TIER_SIZE="$2"; shift 2 ;;
        --server-only)  SERVER_ONLY=1; shift ;;
        --bench-only)   BENCH_ONLY=1; shift ;;
        --no-build)     NO_BUILD=1; shift ;;
        -h|--help)      sed -n '2,18p' "$0"; exit 0 ;;
        *)              echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

log()  { printf '\033[1;34m[certus-bench]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[certus-bench]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[certus-bench]\033[0m %s\n' "$*" >&2; exit 1; }

cleanup() {
    if [[ -n "${SERVER_PID:-}" ]]; then
        log "Stopping server (pid $SERVER_PID)..."
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}

# --- Build -------------------------------------------------------------------
if [[ "$BENCH_ONLY" -eq 0 && "$NO_BUILD" -eq 0 ]]; then
    log "Building certus-server-yaml (SPDK profile)..."
    CERTUS_PROFILE=full cargo build --release \
        --manifest-path "$REPO_ROOT/Cargo.toml" \
        -p certus-server-yaml \
        --features spdk
    log "Build complete."
fi

[[ "$BENCH_ONLY" -eq 1 ]] || [[ -x "$SERVER_BIN" ]] || die "Server binary not found: $SERVER_BIN"

# --- Launch Server -----------------------------------------------------------
if [[ "$BENCH_ONLY" -eq 0 ]]; then
    log "Launching server: $DRIVE_COUNT drives, NUMA node $NUMA_NODE, memory-tier $MEMORY_TIER_SIZE"

    SERVER_CMD=(
        numactl --cpunodebind="$NUMA_NODE" --membind="$NUMA_NODE"
        "$SERVER_BIN"
        --drive-count "$DRIVE_COUNT"
        --listen "$LISTEN"
        --memory-tier-size "$MEMORY_TIER_SIZE"
    )
    [[ -n "$FORMAT_FLAG" ]] && SERVER_CMD+=("$FORMAT_FLAG")

    log "  cmd: ${SERVER_CMD[*]}"
    "${SERVER_CMD[@]}" &
    SERVER_PID=$!
    trap cleanup EXIT

    # Wait for server readiness (gRPC port accepting connections).
    PORT="${LISTEN##*:}"
    log "Waiting for server on port $PORT..."
    DEADLINE=$((SECONDS + 60))
    while [[ $SECONDS -lt $DEADLINE ]]; do
        if (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
            exec 3>&-
            log "Server ready (pid $SERVER_PID)."
            break
        fi
        sleep 0.5
    done

    if ! (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
        die "Server did not become ready within 60s."
    fi
    exec 3>&- 2>/dev/null || true

    if [[ "$SERVER_ONLY" -eq 1 ]]; then
        log "Server running. Press Ctrl-C to stop."
        wait "$SERVER_PID"
        exit 0
    fi
fi

# --- Run Benchmark -----------------------------------------------------------
log "Running benchmark: $NUM_OBJECTS objects, $CLIENTS client(s), $ITERATIONS iterations"
log "  pipeline-depth=$PIPELINE_DEPTH, block-size=$BLOCK_SIZE, writes-settle=${WRITES_SETTLE}s"

BENCH_CMD=(
    "$PYTHON" "$BENCH_SCRIPT"
    --server "localhost:${LISTEN##*:}"
    --clients "$CLIENTS"
    --num-objects "$NUM_OBJECTS"
    --iterations "$ITERATIONS"
    --pipeline-depth "$PIPELINE_DEPTH"
    --block-size "$BLOCK_SIZE"
    --writes-settle "$WRITES_SETTLE"
)

log "  cmd: ${BENCH_CMD[*]}"
echo ""

cd "$(dirname "$BENCH_SCRIPT")"
"${BENCH_CMD[@]}"
BENCH_RC=$?

echo ""
if [[ $BENCH_RC -eq 0 ]]; then
    log "Benchmark completed successfully."
else
    warn "Benchmark exited with code $BENCH_RC."
fi

exit $BENCH_RC
