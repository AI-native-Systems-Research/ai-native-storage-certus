#!/bin/bash
# Collect baseline measurements for all transfer configurations.
#
# Runs gpu-bb-vs-p2p with various ring sizes, chunk sizes, and stream sizes.
# Results are saved to baselines/results/ as JSON.
#
# Prerequisites:
#   - gpu-bb-vs-p2p built: cargo build -p gpu-bb-vs-p2p --release
#   - NVMe bound to VFIO, hugepages allocated, nvidia-peermem + gdrdrv loaded
#
# Usage:
#   ./run_baselines.sh [--pci 0000:62:00.0]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
BINARY="$REPO_ROOT/target/release/gpu-bb-vs-p2p"
RESULTS_DIR="$SCRIPT_DIR/results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

PCI_ARG=""
if [ "${1:-}" = "--pci" ] && [ -n "${2:-}" ]; then
    PCI_ARG="--pci $2"
fi

if [ ! -f "$BINARY" ]; then
    echo "Building gpu-bb-vs-p2p..."
    cargo build -p gpu-bb-vs-p2p --release --manifest-path "$REPO_ROOT/Cargo.toml"
fi

mkdir -p "$RESULTS_DIR"

echo "========================================"
echo "  P2P Baseline Collection"
echo "  Binary: $BINARY"
echo "  Results: $RESULTS_DIR"
echo "  Timestamp: $TIMESTAMP"
echo "========================================"
echo ""

run_config() {
    local label="$1"
    local ring_size="$2"
    local chunk_size="$3"
    local stream_size="$4"
    local iterations="${5:-10}"

    local outfile="$RESULTS_DIR/${label}_${TIMESTAMP}.txt"

    echo "  Running: $label (ring=$ring_size, chunk=$chunk_size, stream=$stream_size)"

    LD_LIBRARY_PATH="${LD_LIBRARY_PATH:-/usr/local/lib}" \
        "$BINARY" \
        $PCI_ARG \
        --ring-size "$ring_size" \
        --chunk-size "$chunk_size" \
        --stream-size "$stream_size" \
        --iterations "$iterations" \
        --warmup 3 \
        > "$outfile" 2>&1

    if [ $? -eq 0 ]; then
        echo "    -> saved to $outfile"
        # Extract key numbers
        grep -E "(bounce-buf|p2p-direct)" "$outfile" | tail -2
    else
        echo "    -> FAILED (see $outfile)"
    fi
    echo ""
}

echo "--- Ring Size Sweep (chunk=128K, stream=5M) ---"
run_config "ring04_chunk128k_stream5m" 4 131072 5242880
run_config "ring08_chunk128k_stream5m" 8 131072 5242880
run_config "ring16_chunk128k_stream5m" 16 131072 5242880
run_config "ring32_chunk128k_stream5m" 32 131072 5242880
run_config "ring48_chunk128k_stream5m" 48 131072 5242880

echo "--- Chunk Size Sweep (ring=32, stream=5M) ---"
run_config "ring32_chunk64k_stream5m" 32 65536 5242880
run_config "ring32_chunk128k_stream5m_v2" 32 131072 5242880
run_config "ring32_chunk256k_stream5m" 32 262144 5242880

echo "--- Stream Size Sweep (ring=32, chunk=128K) ---"
run_config "ring32_chunk128k_stream1m" 32 131072 1048576
run_config "ring32_chunk128k_stream4m" 32 131072 4194304
run_config "ring32_chunk128k_stream16m" 32 131072 16777216
run_config "ring32_chunk128k_stream100m" 32 131072 104857600 5

echo ""
echo "========================================"
echo "  Baselines complete. Results in: $RESULTS_DIR"
echo "========================================"

# Generate summary
echo ""
echo "--- Summary ---"
for f in "$RESULTS_DIR"/*_${TIMESTAMP}.txt; do
    label=$(basename "$f" | sed "s/_${TIMESTAMP}.txt//")
    bounce=$(grep "bounce-buf" "$f" 2>/dev/null | grep -oP '[\d.]+\s*MB/s' | head -1 || echo "N/A")
    p2p=$(grep "p2p-direct" "$f" 2>/dev/null | grep -oP '[\d.]+\s*MB/s' | head -1 || echo "N/A")
    printf "  %-40s  BB: %s  P2P: %s\n" "$label" "$bounce" "$p2p"
done
