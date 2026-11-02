#!/usr/bin/env bash
# Sweep all benchmark modes and collect YAML results.
#
# Usage:
#   ./sweep_modes.sh --nvme 0000:41:00.0,0000:42:00.0              # start server on these drives, sweep
#   ./sweep_modes.sh --nvme 0000:41:00.0 --min-duration 30         # longer run
#   ./sweep_modes.sh --nvme 0000:41:00.0 --tag 2drive              # label the run
#   ./sweep_modes.sh --nvme 0000:41:00.0 --hw-profile              # also run hardware ceiling profiler
#   ./sweep_modes.sh --no-server                                    # assume certus-server already running
#
# When --nvme is given (the default workflow), the script starts certus-server on
# those drives before the sweep and stops it afterward. With --no-server, it
# assumes certus-server is already running.

set -euo pipefail

# NVIDIA driver libs on RHCOS live under a containerized path
if [ -d /home/core/nvidia-libs ]; then
    export LD_LIBRARY_PATH="/home/core/nvidia-libs:${LD_LIBRARY_PATH:-}"
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BENCH="$SCRIPT_DIR/bench_connector_lifecycle.py"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTDIR="${OUTDIR:-$REPO_ROOT/bench-results}"
HW_CEILING_DIR="$REPO_ROOT/benchmarks/hw-ceiling"
[[ ! -d "$HW_CEILING_DIR" ]] && HW_CEILING_DIR="$REPO_ROOT/tools/hw-ceiling"

MIN_DURATION=10
TAG=""
HW_PROFILE=0
NVME_BDFS=""
NO_SERVER=0
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
MEMORY_TIER_SIZE="${MEMORY_TIER_SIZE:-4G}"
CHANNELS="${CHANNELS:-32}"
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --min-duration)     MIN_DURATION="$2"; shift 2 ;;
        --tag)              TAG="$2"; shift 2 ;;
        --outdir)           OUTDIR="$2"; shift 2 ;;
        --hw-profile)       HW_PROFILE=1; shift ;;
        --nvme)             NVME_BDFS="$2"; shift 2 ;;
        --no-server)        NO_SERVER=1; shift ;;
        --memory-tier-size) MEMORY_TIER_SIZE="$2"; shift 2 ;;
        --channels)         CHANNELS="$2"; shift 2 ;;
        *)                  EXTRA_ARGS+=("$1"); shift ;;
    esac
done

mkdir -p "$OUTDIR"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
TAG_SUFFIX=""
[ -n "$TAG" ] && TAG_SUFFIX="_${TAG}"

MODES=(default pipelined prefix-miss contention scheduler-step)

# ── Driver switching ─────────────────────────────────────────────────────────
# Rebind NVMe BDFs between kernel nvme driver (for fio) and vfio-pci (for SPDK).

bind_to_nvme_driver() {
    local device_pcis="${NVME_BDFS//,/ }"
    for bdf in $device_pcis; do
        local drv
        drv="$(basename "$(readlink "/sys/bus/pci/devices/$bdf/driver" 2>/dev/null)" 2>/dev/null)"
        if [[ "$drv" == "nvme" ]]; then
            echo "[sweep] $bdf already on nvme driver"
            continue
        fi
        echo "[sweep] rebinding $bdf: $drv -> nvme"
        [[ -n "$drv" ]] && echo "$bdf" | sudo tee "/sys/bus/pci/drivers/$drv/unbind" >/dev/null 2>&1
        echo "" | sudo tee "/sys/bus/pci/devices/$bdf/driver_override" >/dev/null 2>&1
        echo "$bdf" | sudo tee /sys/bus/pci/drivers/nvme/bind >/dev/null 2>&1
    done
    sleep 2
    for bdf in $device_pcis; do
        local drv
        drv="$(basename "$(readlink "/sys/bus/pci/devices/$bdf/driver" 2>/dev/null)" 2>/dev/null)"
        if [[ "$drv" != "nvme" ]]; then
            echo "[sweep] WARN: $bdf failed to bind to nvme (current: ${drv:-none})" >&2
        fi
    done
}

bind_to_vfio_pci() {
    local device_pcis="${NVME_BDFS//,/ }"
    # Ensure vfio-pci module is loaded (noiommu mode if needed)
    if ! lsmod | grep -q vfio_pci; then
        sudo modprobe vfio-pci 2>/dev/null || true
    fi
    if [[ -f /sys/module/vfio/parameters/enable_unsafe_noiommu_mode ]]; then
        echo 1 | sudo tee /sys/module/vfio/parameters/enable_unsafe_noiommu_mode >/dev/null 2>&1 || true
    fi
    for bdf in $device_pcis; do
        local drv
        drv="$(basename "$(readlink "/sys/bus/pci/devices/$bdf/driver" 2>/dev/null)" 2>/dev/null)"
        if [[ "$drv" == "vfio-pci" ]]; then
            echo "[sweep] $bdf already on vfio-pci"
            continue
        fi
        echo "[sweep] rebinding $bdf: $drv -> vfio-pci"
        [[ -n "$drv" ]] && echo "$bdf" | sudo tee "/sys/bus/pci/drivers/$drv/unbind" >/dev/null 2>&1
        echo "vfio-pci" | sudo tee "/sys/bus/pci/devices/$bdf/driver_override" >/dev/null 2>&1
        echo "$bdf" | sudo tee /sys/bus/pci/drivers_probe >/dev/null 2>&1
    done
    sleep 1
    for bdf in $device_pcis; do
        local drv
        drv="$(basename "$(readlink "/sys/bus/pci/devices/$bdf/driver" 2>/dev/null)" 2>/dev/null)"
        if [[ "$drv" != "vfio-pci" ]]; then
            echo "[sweep] ERROR: $bdf failed to bind to vfio-pci (current: ${drv:-none})" >&2
            return 1
        fi
    done
}

# ── certus-server lifecycle ──────────────────────────────────────────────────
CERTUS_PGID=""

stop_certus() {
    [[ "$NO_SERVER" == 1 ]] && return 0
    [[ -n "$CERTUS_PGID" ]] || { pgrep -f 'certus-server-yaml.*--shm-path' >/dev/null 2>&1 || return 0; }
    echo "[sweep] stopping certus-server"
    [[ -n "$CERTUS_PGID" ]] && sudo kill -TERM "-$CERTUS_PGID" 2>/dev/null
    sudo pkill -TERM -f 'certus-server-yaml.*--shm-path' 2>/dev/null
    for _ in $(seq 1 30); do pgrep -f 'certus-server-yaml.*--shm-path' >/dev/null 2>&1 || break; sleep 1; done
    sudo pkill -KILL -f 'certus-server-yaml.*--shm-path' 2>/dev/null
    sudo rm -f "$SHM_PATH" 2>/dev/null
    CERTUS_PGID=""
}

start_certus() {
    stop_certus
    local device_pcis="${NVME_BDFS//,/ }"

    local server_bin="$REPO_ROOT/target/release/certus-server-yaml"
    if [[ ! -x "$server_bin" ]]; then
        echo "[sweep] ERROR: $server_bin not found" >&2; return 1
    fi

    # ── Pre-flight: hugepages ─────────────────────────────────────────────
    local hp_free
    hp_free="$(awk '/^HugePages_Free:/{print $2}' /proc/meminfo 2>/dev/null)"
    if [[ -z "$hp_free" || "$hp_free" -eq 0 ]]; then
        echo "[sweep] ERROR: no free hugepages" >&2; return 1
    fi

    # Ensure drives are on vfio-pci
    bind_to_vfio_pci || return 1

    # ── Start certus-server ───────────────────────────────────────────────
    local dev_args=()
    for pci in $device_pcis; do dev_args+=(--device-pci "$pci"); done

    local nvidia_libs="/home/core/nvidia-libs"
    local extra_ld=""
    [[ -d "$nvidia_libs" ]] && extra_ld="$nvidia_libs"

    echo "[sweep] starting certus-server (${MEMORY_TIER_SIZE} tier, drives: ${device_pcis})"
    setsid sudo env \
        ${extra_ld:+LD_LIBRARY_PATH="$extra_ld"} \
        "$server_bin" \
        "${dev_args[@]}" \
        --shm-path "$SHM_PATH" \
        --memory-tier-size "$MEMORY_TIER_SIZE" \
        --memory-tier-eviction-threshold "${EVICTION_THRESHOLD:-0.9}" \
        --store-backpressure-ms "${STORE_BACKPRESSURE_MS:-5000}" \
        --channels "$CHANNELS" \
        --format \
        >"$OUTDIR/certus-server.log" 2>&1 &
    CERTUS_PGID=$!

    for _ in $(seq 1 90); do
        [[ -e "$SHM_PATH" ]] && {
            sleep 3
            sudo chmod a+rw "$SHM_PATH"
            echo "[sweep] certus-server up (mailbox $SHM_PATH)"
            return 0
        }
        sleep 1
    done
    echo "[sweep] ERROR: certus-server mailbox did not appear; see $OUTDIR/certus-server.log" >&2
    return 1
}

cleanup() { stop_certus; }
trap cleanup EXIT INT TERM

echo "=== certus-connector-bench mode sweep ==="
echo "  modes:        ${MODES[*]}"
echo "  min-duration: ${MIN_DURATION}s"
echo "  output dir:   $OUTDIR"
echo "  tag:          ${TAG:-<none>}"
echo "  hw-profile:   $([[ $HW_PROFILE == 1 ]] && echo 'yes' || echo 'no')"
[[ -n "$NVME_BDFS" ]] && echo "  nvme:         $NVME_BDFS"
echo "  server:       $([[ $NO_SERVER == 1 ]] && echo 'external (--no-server)' || echo 'managed')"
echo ""

# ── Phase 1: Hardware ceiling profile (drives on kernel nvme driver for fio) ─
if [[ "$HW_PROFILE" == 1 ]]; then
    if [[ -z "$NVME_BDFS" ]]; then
        echo "error: --nvme required for --hw-profile" >&2; exit 1
    fi

    echo "── phase 1: hardware ceiling profile (kernel nvme driver) ──"
    bind_to_nvme_driver

    PROFILE_DIR="$OUTDIR/hw-ceiling"
    PROFILE_REL="bench-results/hw-ceiling"
    mkdir -p "$PROFILE_DIR"
    PROFILE_ARGS=("$PROFILE_REL")
    [[ -n "$TAG" ]]       && PROFILE_ARGS+=(--tag "$TAG")
    [[ -n "$NVME_BDFS" ]] && PROFILE_ARGS+=(--nvme "$NVME_BDFS")

    if [[ -x "$HW_CEILING_DIR/run-profile.sh" ]]; then
        "$HW_CEILING_DIR/run-profile.sh" "${PROFILE_ARGS[@]}" || \
            echo "  WARN: hardware ceiling profile exited $? — continuing with sweep"
        echo "  → $PROFILE_DIR/"
    else
        echo "  WARN: hw-ceiling profiler not found at $HW_CEILING_DIR — skipping"
    fi
    echo ""
fi

# ── Phase 2: certus-server + connector bench sweep (drives on vfio-pci) ─────
if [[ "$NO_SERVER" != 1 ]]; then
    if [[ -z "$NVME_BDFS" ]]; then
        echo "error: --nvme required (certus-server needs drive BDFs). Use --no-server if it's already running." >&2
        exit 1
    fi
    echo "── phase 2: certus connector bench (vfio-pci / SPDK) ──"
    start_certus || exit 1
fi

# ── Connector bench sweep ────────────────────────────────────────────────────
for mode in "${MODES[@]}"; do
    YAML_OUT="$OUTDIR/${mode}${TAG_SUFFIX}_${TIMESTAMP}.yaml"
    CSV_FILE="$OUTDIR/${mode}${TAG_SUFFIX}_${TIMESTAMP}.csv"
    echo "── $mode ──"
    TAG_ARGS=()
    [ -n "$TAG" ] && TAG_ARGS=(--tag "$TAG")

    python3 "$BENCH" \
        --mode "$mode" \
        --min-duration "$MIN_DURATION" \
        --output "$YAML_OUT" \
        --csv "$CSV_FILE" \
        "${TAG_ARGS[@]}" \
        "${EXTRA_ARGS[@]}"

    echo "  → $YAML_OUT"
    echo ""
done

echo "=== sweep complete ==="
echo "  per-mode YAML: $OUTDIR/*${TAG_SUFFIX}_${TIMESTAMP}.yaml"
echo "  per-mode CSV:  $OUTDIR/*${TAG_SUFFIX}_${TIMESTAMP}.csv"
[[ "$HW_PROFILE" == 1 ]] && echo "  hw profile:    $OUTDIR/hw-ceiling/"
