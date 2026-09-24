#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Certus Mooncake Benchmark Runner
#
# Builds certus-server, starts it, runs the trace-replay benchmark against
# Certus, then tears down the server.  Sweep mode iterates over page sizes
# AND drive counts, restarting the server for each drive configuration.
#
# Server lifecycle follows the pattern from
# workbench/targets/evolve-throughput/evaluate.py:
#   build → clean stale shm → start server (sudo) → wait for shmq ready
#   → run benchmark → stop server → clean up
#
# Usage:
#   ./run.sh                                     # 4 drives, toolagent, default model
#   ./run.sh --sweep                             # page-size × drive-count sweep
#   ./run.sh --sweep --sweep-drives "1 2 4"      # custom drive counts
#   ./run.sh --disk-only                         # smoke test, no server/GPU
#   ./run.sh --server-running                    # server already running
# ---------------------------------------------------------------------------
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ---------------------------------------------------------------------------
# Auto-discover library paths for building/running certus-server
# ---------------------------------------------------------------------------
_add_lib_path() {
    local var="$1" dir="$2"
    [[ -d "$dir" ]] || return
    local cur="${!var:-}"
    case ":${cur}:" in
        *:"$dir":*) ;;
        *) export "$var"="${cur:+${cur}:}$dir" ;;
    esac
}
_gdrcopy_src="$REPO_ROOT/kernel/modules/gdrcopy/src"
_add_lib_path LIBRARY_PATH "$_gdrcopy_src"
_add_lib_path LD_LIBRARY_PATH "$_gdrcopy_src"
_add_lib_path LIBRARY_PATH /usr/local/lib
_add_lib_path LD_LIBRARY_PATH /usr/local/lib
_add_lib_path LIBRARY_PATH /usr/local/cuda/lib64
_add_lib_path LD_LIBRARY_PATH /usr/local/cuda/lib64
_pip_cuda="$HOME/.local/lib/python3.9/site-packages/nvidia/cuda_runtime/lib"
_add_lib_path LIBRARY_PATH "$_pip_cuda"
_add_lib_path LD_LIBRARY_PATH "$_pip_cuda"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
SHM_PATH="/dev/shm/certus-shmq"
CHANNELS=8
MEMORY_TIER_SIZE="4G"
GPU_DEVICE=0
DRIVE_COUNT=4
MODEL="glm5"
MODE="replay"
SWEEP=false
SWEEP_SIZES="1 2 3 4 5 6 7 8 9 10"
SWEEP_SIZES_KIB=""                     # additional sub-MiB sizes in KiB (e.g. "160 400")
SWEEP_DRIVES=""                        # empty = just use DRIVE_COUNT / EXPLICIT_PCI
RESULTS_DIR=""
SCENARIO="toolagent"
MAX_REQUESTS=""
MAX_PAGES=2000
THREADS=1
REPLAY_SCALES="0"
PROGRESS_INTERVAL=100
DISK_ONLY=false
SKIP_BUILD=false
SERVER_RUNNING=false
STORAGE_DIR="/tmp/certus_mooncake_bench"

SERVER_PID=""
SERVER_LOG_DIR=""

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Options:
  --drive-count N          Use first N discovered NVMe drives (default: 4)
  --device-pci PCI         NVMe PCI address (repeatable; overrides --drive-count)
  --model MODEL            Model preset (default: glm5). Use --list-models for all.
  --mode MODE              replay (default) or tiered-stress
  --sweep                  Page-size × drive-count sweep with HTML report
  --sweep-sizes "1 2 5 10" Page sizes in MiB (default: 1 2 3 4 5 6 7 8 9 10)
  --sweep-sizes-kib "160 400"  Additional page sizes in KiB (for sub-MiB models)
  --sweep-drives "1 2 4"   Drive counts to sweep (default: just --drive-count)
  --results-dir DIR        Save JSON + HTML report here (default: results/<timestamp>)
  --disk-only              Run disk baseline only (no server, no GPU)
  --skip-build             Skip cargo build (use existing binary)
  --server-running         Don't start/stop server (assume already running)
  --scenario SCENARIO      conversation, synthetic, toolagent, or all (default: toolagent)
  --max-requests N         Limit requests per trace (default: all)
  --max-pages N            Max pages / modulo mapping (default: 2000)
  --threads N              Client worker threads (default: 1)
  --shm-path PATH          shmq mailbox path (default: /dev/shm/certus-shmq)
  --channels N             shmq channels (default: 8)
  --memory-tier-size SIZE  Memory tier size (default: 4G)
  --gpu-device N           CUDA device (default: 0)
  --storage-dir DIR        Disk backend storage dir (default: /tmp/certus_mooncake_bench)
  -h, --help               Show this help

Examples:
  ./run.sh                                        # 4 drives, toolagent, default model
  ./run.sh --sweep                                # sweep 1-10 MiB × current drive count
  ./run.sh --sweep --sweep-drives "1 2 3 4"       # sweep page sizes × 1,2,3,4 drives
  ./run.sh --sweep-sizes "1 5 10" --sweep-drives "1 4"  # targeted sweep
  ./run.sh --model llama-3-70b                    # single run, different model
  ./run.sh --disk-only --max-requests 100         # smoke test
  ./run.sh --server-running                       # server already up
EOF
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
EXPLICIT_PCI=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --disk-only)        DISK_ONLY=true; shift ;;
        --skip-build)       SKIP_BUILD=true; shift ;;
        --server-running)   SERVER_RUNNING=true; shift ;;
        --model)            MODEL="$2"; shift 2 ;;
        --mode)             MODE="$2"; shift 2 ;;
        --sweep)            SWEEP=true; shift ;;
        --sweep-sizes)      SWEEP=true; SWEEP_SIZES="$2"; shift 2 ;;
        --sweep-sizes-kib)  SWEEP=true; SWEEP_SIZES_KIB="$2"; shift 2 ;;
        --sweep-drives)     SWEEP=true; SWEEP_DRIVES="$2"; shift 2 ;;
        --results-dir)      RESULTS_DIR="$2"; shift 2 ;;
        --scenario)         SCENARIO="$2"; shift 2 ;;
        --max-requests)     MAX_REQUESTS="$2"; shift 2 ;;
        --max-pages)        MAX_PAGES="$2"; shift 2 ;;
        --threads)          THREADS="$2"; shift 2 ;;
        --replay-scales)    REPLAY_SCALES="$2"; shift 2 ;;
        --progress-interval) PROGRESS_INTERVAL="$2"; shift 2 ;;
        --shm-path)         SHM_PATH="$2"; shift 2 ;;
        --channels)         CHANNELS="$2"; shift 2 ;;
        --memory-tier-size) MEMORY_TIER_SIZE="$2"; shift 2 ;;
        --gpu-device)       GPU_DEVICE="$2"; shift 2 ;;
        --storage-dir)      STORAGE_DIR="$2"; shift 2 ;;
        --device-pci)       EXPLICIT_PCI+=("$2"); shift 2 ;;
        --drive-count)      DRIVE_COUNT="$2"; shift 2 ;;
        -h|--help)          usage; exit 0 ;;
        *)                  echo "Unknown option: $1"; usage; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Server lifecycle functions
# ---------------------------------------------------------------------------
start_server() {
    # Args: device args to pass to certus-server (e.g. --device-pci X or --drive-count N)
    local device_args=("$@")
    local server_binary="$REPO_ROOT/target/release/certus-server"

    if [[ ! -x "$server_binary" ]]; then
        echo "ERROR: Server binary not found: $server_binary"
        echo "Run without --skip-build, or build manually."
        return 1
    fi

    # Kill any stale certus-server processes (from previous runs / SIGKILLs)
    local stale_pids
    stale_pids=$(pgrep -f "$server_binary" 2>/dev/null || true)
    if [[ -n "$stale_pids" ]]; then
        echo "  Cleaning stale server processes: $stale_pids"
        for pid in $stale_pids; do
            sudo -n kill -TERM "$pid" 2>/dev/null || true
        done
        sleep 2
        for pid in $stale_pids; do
            sudo -n kill -KILL "$pid" 2>/dev/null || true
        done
        sleep 1
    fi

    # Clean stale shm + SPDK/DPDK state
    sudo -n rm -f "$SHM_PATH" 2>/dev/null || rm -f "$SHM_PATH" 2>/dev/null || true
    sudo -n rm -rf /dev/hugepages/spdk_pid* 2>/dev/null || true
    sudo -n rm -rf /var/run/dpdk/spdk* 2>/dev/null || true

    SERVER_LOG_DIR="$(mktemp -d /tmp/certus-mooncake-bench-server.XXXXXX)"

    echo ""
    echo "  ┌──────────────────────────────────────────────────────────────────┐"
    echo "  │  Starting certus-server                                          │"
    echo "  │  Devices: ${device_args[*]}"
    echo "  │  Channels: $CHANNELS  Memory tier: $MEMORY_TIER_SIZE"
    echo "  │  Logs: $SERVER_LOG_DIR/"
    echo "  └──────────────────────────────────────────────────────────────────┘"

    sudo -n "$server_binary" \
        "${device_args[@]}" \
        --shm-path "$SHM_PATH" \
        --channels "$CHANNELS" \
        --memory-tier-size "$MEMORY_TIER_SIZE" \
        --format \
        > "$SERVER_LOG_DIR/server.stdout.log" \
        2> "$SERVER_LOG_DIR/server.stderr.log" &
    SERVER_PID=$!

    # Wait for shmq readiness
    local ready_timeout=20
    for _ in $(seq 1 $((ready_timeout * 5))); do
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            echo "  ERROR: Server exited before shmq ready."
            echo "    stderr: $(tail -3 "$SERVER_LOG_DIR/server.stderr.log" 2>/dev/null)"
            SERVER_PID=""
            return 1
        fi
        if [[ -e "$SHM_PATH" ]]; then
            sudo -n chmod 666 "$SHM_PATH" 2>/dev/null || true
            echo "  shmq ready (pid=$SERVER_PID)."
            return 0
        fi
        sleep 0.2
    done

    echo "  ERROR: shmq not ready after ${ready_timeout}s"
    echo "    stderr: $(tail -5 "$SERVER_LOG_DIR/server.stderr.log" 2>/dev/null)"
    return 1
}

stop_server() {
    if [[ -z "$SERVER_PID" ]]; then
        return
    fi
    echo "  Stopping server (pid=$SERVER_PID)..."
    # SIGTERM first — give SPDK time to release VFIO fds and hugepages
    sudo -n kill -TERM -- "-$SERVER_PID" 2>/dev/null || true
    for _ in $(seq 1 80); do
        kill -0 "$SERVER_PID" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "  Server did not exit on SIGTERM, sending SIGKILL..."
        sudo -n kill -KILL -- "-$SERVER_PID" 2>/dev/null || true
        sleep 2
    fi
    SERVER_PID=""
    # Clean shm + stale SPDK/DPDK state so the next server can attach
    sudo -n rm -f "$SHM_PATH" 2>/dev/null || rm -f "$SHM_PATH" 2>/dev/null || true
    sudo -n rm -rf /dev/hugepages/spdk_pid* 2>/dev/null || true
    sudo -n rm -rf /var/run/dpdk/spdk* 2>/dev/null || true
    # Brief settle for VFIO group release
    sleep 1
}

# Cleanup on exit
cleanup() { stop_server; }
trap cleanup EXIT INT TERM

# Build device args from EXPLICIT_PCI or DRIVE_COUNT
build_device_args() {
    local count="$1"
    if [[ ${#EXPLICIT_PCI[@]} -gt 0 ]]; then
        # Use explicit PCI — take first $count addresses
        local args=()
        local i=0
        for pci in "${EXPLICIT_PCI[@]}"; do
            if [[ $i -ge $count ]]; then break; fi
            args+=("--device-pci" "$pci")
            ((i++))
        done
        echo "${args[@]}"
    else
        echo "--drive-count $count"
    fi
}

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
if [[ "$DISK_ONLY" == false && "$SKIP_BUILD" == false && "$SERVER_RUNNING" == false ]]; then
    echo "================================================================================"
    echo "  Building certus-server (release)"
    echo "================================================================================"
    cd "$REPO_ROOT"
    cargo build -p certus-server --release
    echo "[run.sh] Build complete."
fi

# ---------------------------------------------------------------------------
# Benchmark common args (passed to benchmark.py)
# ---------------------------------------------------------------------------
BENCH_COMMON=(
    --mode "$MODE"
    --scenario "$SCENARIO"
    --max-pages "$MAX_PAGES"
    --threads "$THREADS"
    --replay-scales "$REPLAY_SCALES"
    --progress-interval "$PROGRESS_INTERVAL"
    --storage-dir "$STORAGE_DIR"
)
if [[ -n "$MAX_REQUESTS" ]]; then
    BENCH_COMMON+=(--max-requests "$MAX_REQUESTS")
fi

CERTUS_ARGS=(
    --backend certus
    --shm-path "$SHM_PATH"
    --gpu-device "$GPU_DEVICE"
)

# ---------------------------------------------------------------------------
# Disk-only mode
# ---------------------------------------------------------------------------
# TODO: Mount one NVMe drive with ext4/xfs and point --storage-dir at it for a
#       fair disk-vs-certus comparison on the same hardware (e.g. unbind one
#       drive from VFIO, mkfs.ext4, mount, pass --storage-dir /mnt/nvme_bench).
if [[ "$DISK_ONLY" == true ]]; then
    echo "================================================================================"
    echo "  Running disk baseline (smoke test only — OS disk, not SPDK NVMe)"
    echo "================================================================================"
    cd "$SCRIPT_DIR"
    python3 benchmark.py --backend disk --model "$MODEL" "${BENCH_COMMON[@]}"
    echo ""
    echo "  NOTE: Disk backend used ${STORAGE_DIR} (OS disk / tmpfs)."
    exit 0
fi

# ---------------------------------------------------------------------------
# Sweep mode: page sizes × drive counts
# ---------------------------------------------------------------------------
if [[ "$SWEEP" == true ]]; then
    # Default sweep-drives to just the current DRIVE_COUNT if not set
    if [[ -z "$SWEEP_DRIVES" ]]; then
        SWEEP_DRIVES="$DRIVE_COUNT"
    fi

    if [[ -z "$RESULTS_DIR" ]]; then
        RESULTS_DIR="$SCRIPT_DIR/results/$(date +%Y%m%d-%H%M%S)"
    fi
    mkdir -p "$RESULTS_DIR"
    RESULTS_JSON="$RESULTS_DIR/sweep_results.json"
    # Don't wipe existing results — benchmark.py --output-json appends
    if [[ ! -f "$RESULTS_JSON" ]]; then
        echo "[]" > "$RESULTS_JSON"
    fi

    echo ""
    echo "╔══════════════════════════════════════════════════════════════════════════════╗"
    if [[ -n "$SWEEP_SIZES_KIB" ]]; then
        echo "║  SWEEP: page sizes [${SWEEP_SIZES_KIB}] KiB + [${SWEEP_SIZES}] MiB × drives [${SWEEP_DRIVES}]"
    else
        echo "║  SWEEP: page sizes [${SWEEP_SIZES}] MiB × drives [${SWEEP_DRIVES}]"
    fi
    echo "║  Results: ${RESULTS_DIR}/"
    echo "╚══════════════════════════════════════════════════════════════════════════════╝"

    for NUM_DRIVES in $SWEEP_DRIVES; do
        echo ""
        echo "┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓"
        echo "┃  Drive configuration: ${NUM_DRIVES} drive(s)"
        echo "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛"

        if [[ "$SERVER_RUNNING" == false ]]; then
            stop_server  # stop previous iteration's server
            local_device_args=($(build_device_args "$NUM_DRIVES"))
            if ! start_server "${local_device_args[@]}"; then
                echo "  ⚠️  Failed to start server with ${NUM_DRIVES} drive(s), skipping..."
                continue
            fi
        fi

        # Sub-MiB page sizes (KiB)
        for SIZE_KIB in $SWEEP_SIZES_KIB; do
            # bytes_per_token = (SIZE_KiB * 1024) / page_size_tokens
            # Default page_size_tokens=512, so bytes_per_token = SIZE_KiB * 2
            BPT=$((SIZE_KIB * 2))
            # page_size_mib as decimal for tagging
            PAGE_MIB=$(python3 -c "print(round($SIZE_KIB / 1024, 3))")
            echo ""
            echo "  ── ${NUM_DRIVES} drive(s) × ${SIZE_KIB} KiB pages (bytes_per_token=${BPT}) ──"
            cd "$SCRIPT_DIR"
            python3 benchmark.py \
                --bytes-per-token "$BPT" \
                --tag "drives=$NUM_DRIVES" \
                --tag "page_size_mib=$PAGE_MIB" \
                --output-json "$RESULTS_JSON" \
                "${CERTUS_ARGS[@]}" "${BENCH_COMMON[@]}" || {
                echo "  ⚠️  ${NUM_DRIVES}d × ${SIZE_KIB} KiB failed, continuing..."
                continue
            }
        done

        # MiB page sizes
        for SIZE_MIB in $SWEEP_SIZES; do
            BPT=$((SIZE_MIB * 2048))
            echo ""
            echo "  ── ${NUM_DRIVES} drive(s) × ${SIZE_MIB} MiB pages (bytes_per_token=${BPT}) ──"
            cd "$SCRIPT_DIR"
            python3 benchmark.py \
                --bytes-per-token "$BPT" \
                --tag "drives=$NUM_DRIVES" \
                --tag "page_size_mib=$SIZE_MIB" \
                --output-json "$RESULTS_JSON" \
                "${CERTUS_ARGS[@]}" "${BENCH_COMMON[@]}" || {
                echo "  ⚠️  ${NUM_DRIVES}d × ${SIZE_MIB} MiB failed, continuing..."
                continue
            }
        done

        if [[ "$SERVER_RUNNING" == false ]]; then
            stop_server
        fi
    done

    # Generate HTML report
    echo ""
    echo "================================================================================"
    echo "  Generating HTML report..."
    echo "================================================================================"
    cd "$SCRIPT_DIR"
    python3 report.py "$RESULTS_JSON" --output "$RESULTS_DIR/report.html"
    echo ""
    echo "╔══════════════════════════════════════════════════════════════════════════════╗"
    echo "║  Sweep complete"
    echo "║  Results JSON: ${RESULTS_JSON}"
    echo "║  HTML report:  ${RESULTS_DIR}/report.html"
    echo "╚══════════════════════════════════════════════════════════════════════════════╝"
    exit 0
fi

# ---------------------------------------------------------------------------
# Single model run
# ---------------------------------------------------------------------------
if [[ "$SERVER_RUNNING" == false ]]; then
    device_args=($(build_device_args "$DRIVE_COUNT"))
    start_server "${device_args[@]}" || exit 1
fi

SINGLE_ARGS=(--model "$MODEL")
if [[ -n "$RESULTS_DIR" ]]; then
    mkdir -p "$RESULTS_DIR"
    SINGLE_ARGS+=(--output-json "$RESULTS_DIR/results.json")
fi

echo ""
echo "================================================================================"
echo "  Running certus backend (model: ${MODEL})"
echo "================================================================================"
cd "$SCRIPT_DIR"
python3 benchmark.py "${SINGLE_ARGS[@]}" "${CERTUS_ARGS[@]}" "${BENCH_COMMON[@]}"

echo ""
echo "================================================================================"
echo "  Benchmark complete"
echo "================================================================================"
