#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CERTUS_PROFILE=full-remote cargo build --release \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    -p certus-server-yaml \
    --features "spdk,rdma,remote-lookup-rdma-initiator/rdma,remote-lookup-rdma-responder/rdma"

echo ""
echo "Usage:"
echo "  certus-server-yaml OPTS --device-pci <DDDD:BB:DD.F> [--device-pci ...]"
echo "  certus-server-yaml OPTS --drive-count <N>"
echo
echo "OPTS"
echo "  [--rl-group cluster-name]   join/create a named cluster (default: isolated single node)"
