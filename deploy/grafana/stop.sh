#!/bin/bash
#
# stop.sh - Stop the Certus observability stack
#
# Usage:
#   ./stop.sh            # stop containers (preserves data volumes)
#   ./stop.sh --clean    # stop and remove volumes (deletes all stored metrics)
#
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

if [[ "${1:-}" == "--clean" ]]; then
    echo "Stopping stack and removing volumes..."
    docker compose down -v
else
    echo "Stopping stack (volumes preserved)..."
    docker compose down
fi

echo "Done."
