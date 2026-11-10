#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Default: the full-transfer profile (full + the certus-to-certus transfer
# endpoint behind shm-queue FETCH), which needs rdma-core (libibverbs,
# librdmacm) to build and a RoCE NIC to run. CERTUS_PROFILE=full builds the
# profile without the endpoint, and with no rdma-core.
CERTUS_PROFILE="${CERTUS_PROFILE:-full-transfer}"
FEATURES="spdk,otel,rw-telemetry"
if [[ "$CERTUS_PROFILE" == "full-transfer" ]]; then
    FEATURES="$FEATURES,rdma"
fi

echo "Building certus-server-yaml: profile $CERTUS_PROFILE, features $FEATURES"
CERTUS_PROFILE="$CERTUS_PROFILE" cargo build --release \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    -p certus-server-yaml \
    --features "$FEATURES" \
    "$@"

echo ""
echo "Usage:"
echo "  certus-server-yaml --device-pci <DDDD:BB:DD.F> [--device-pci ...]"
echo "  certus-server-yaml --drive-count <N>"
if [[ "$CERTUS_PROFILE" == "full-transfer" ]]; then
    echo
    echo "Transfer endpoint (full-transfer):"
    echo "  CERTUS_RDMA_BIND_IP=<RoCE IPv4>   listen address (unset: first active RoCE port)"
    echo "  CERTUS_RDMA_PORT=<port>           listen port (default 7481)"
    echo "  CERTUS_TRANSFER_CPU=<cpu>         pin the endpoint thread (a core on the NIC's node)"
    echo "  The endpoint logs its name, ip:port, at start: that is what a peer's"
    echo "  kv_transfer_params certus_fetch_from hint must say."
fi
