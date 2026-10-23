#!/usr/bin/env bash
# Sweep all benchmark modes and collect YAML results.
#
# Usage:
#   ./sweep_modes.sh                     # defaults: 10s per mode, results in bench-results/
#   ./sweep_modes.sh --min-duration 30   # longer run
#   ./sweep_modes.sh --tag 3-drive       # label the run
#
# Assumes certus-server is already running with appropriate flags.

set -euo pipefail

# NVIDIA driver libs on RHCOS live under a containerized path
if [ -d /home/core/nvidia-libs ]; then
    export LD_LIBRARY_PATH="/home/core/nvidia-libs:${LD_LIBRARY_PATH:-}"
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BENCH="$SCRIPT_DIR/bench_connector_lifecycle.py"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTDIR="${OUTDIR:-$REPO_ROOT/bench-results}"

MIN_DURATION=10
TAG=""
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --min-duration) MIN_DURATION="$2"; shift 2 ;;
        --tag)          TAG="$2"; shift 2 ;;
        --outdir)       OUTDIR="$2"; shift 2 ;;
        *)              EXTRA_ARGS+=("$1"); shift ;;
    esac
done

mkdir -p "$OUTDIR"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
TAG_SUFFIX=""
[ -n "$TAG" ] && TAG_SUFFIX="_${TAG}"

MODES=(default pipelined prefix-miss contention scheduler-step)

echo "=== certus-connector-bench mode sweep ==="
echo "  modes:        ${MODES[*]}"
echo "  min-duration: ${MIN_DURATION}s"
echo "  output dir:   $OUTDIR"
echo "  tag:          ${TAG:-<none>}"
echo ""

COMBINED="$OUTDIR/sweep${TAG_SUFFIX}_${TIMESTAMP}.yaml"
CSV_FILE="$OUTDIR/sweep${TAG_SUFFIX}_${TIMESTAMP}.csv"

for mode in "${MODES[@]}"; do
    YAML_OUT="$OUTDIR/${mode}${TAG_SUFFIX}_${TIMESTAMP}.yaml"
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
echo "  combined CSV:  $CSV_FILE"
