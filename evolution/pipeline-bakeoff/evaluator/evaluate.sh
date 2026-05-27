#!/usr/bin/env bash
# Shell wrapper for the pipeline bakeoff evaluator.
# Usage: evaluate.sh <candidate_pipeline.rs> [--eval fixed|mixed|concurrent]
#
# Compatible with SkyDiscover/OpenEvolve evaluator interface.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$SCRIPT_DIR/evaluate.py" "$@"
