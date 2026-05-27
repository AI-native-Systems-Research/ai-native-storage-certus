#!/bin/bash
# Run the H3 (concurrent multi-client) bakeoff
# Evolves: pipeline.rs + lib.rs (dispatcher architecture)
# Evaluates: 8 concurrent clients, 6 NVMe drives, 4 MiB objects
#
# Usage:
#   ./run_bakeoff_h3.sh                    # All frameworks (skip shinkaevolve)
#   ./run_bakeoff_h3.sh --frameworks nous  # Just Nous
#   ./run_bakeoff_h3.sh --iterations 20    # Custom iteration count

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BAKEOFF_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=================================================================="
echo "  H3 Bakeoff: Multi-Client Concurrent Throughput"
echo "  Evolving: pipeline.rs + lib.rs (architectural contention)"
echo "  Evaluator: 8 clients, 6 NVMe drives, 4 MiB cold lookups"
echo "  Baseline: ~5 GB/s (mutex-bound, same as single client)"
echo "  Target: 15-20 GB/s aggregate"
echo "=================================================================="
echo ""

# Default: all H3 frameworks (skip shinkaevolve and claude_code)
DEFAULT_FRAMEWORKS="adaevolve,evox,gepa_native,openevolve_native,ksearch,nous"

# If no arguments given, use defaults. Otherwise pass everything through.
if [ $# -eq 0 ]; then
    python3 "${BAKEOFF_DIR}/run_bakeoff.py" \
        --eval concurrent \
        --iterations 30 \
        --frameworks "$DEFAULT_FRAMEWORKS"
else
    python3 "${BAKEOFF_DIR}/run_bakeoff.py" \
        --eval concurrent \
        "$@"
fi
