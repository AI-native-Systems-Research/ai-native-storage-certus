#!/usr/bin/env bash
set -euo pipefail

REAL_SCRIPT="$(readlink -f "${BASH_SOURCE[0]}")"
SCRIPT_DIR="$(cd "$(dirname "$REAL_SCRIPT")" && pwd)"
exec python3 "$SCRIPT_DIR/run.py" "$@"
