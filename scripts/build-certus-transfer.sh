#!/usr/bin/env bash
# Build certus-server-yaml with the full-transfer profile: full + the
# certus-to-certus transfer endpoint behind shm-queue FETCH. The same build as
# scripts/build-certus-spdk.sh's default, by name. Extra cargo arguments pass
# through (e.g. --features otel).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CERTUS_PROFILE=full-transfer exec "$SCRIPT_DIR/build-certus-spdk.sh" "$@"
