#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CERTUS_PROFILE=full-fs-block cargo build --release \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    -p certus-server-yaml \
    --no-default-features \
    --features filesys

echo ""
echo "Usage:"
echo "  certus-server-yaml --drive-count <N>"
