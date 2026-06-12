#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../ai-native-storage-certus" && pwd)"

CERTUS_PROFILE=full-kernel-block cargo build --release \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    -p certus-server-yaml \
    --no-default-features \
    --features kernel

echo ""
echo "Usage:"
echo "  certus-server-yaml --device-path /dev/nvme0n1 [--device-path /dev/md127 ...]"
