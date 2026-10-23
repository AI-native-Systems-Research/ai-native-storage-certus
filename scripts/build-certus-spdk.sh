#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CERTUS_PROFILE=full cargo build --release \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    -p certus-server-yaml \
    --features spdk

echo ""
echo "Usage:"
echo "  certus-server-yaml --device-pci <DDDD:BB:DD.F> [--device-pci ...]"
echo "  certus-server-yaml --drive-count <N>"
