#!/bin/bash
#
# Launch RDMA test client/server pair over SSH.
#
# Usage:
#   ./launch.sh <server_host> <client_host> [rdma-test options...]
#
# Examples:
#   ./launch.sh node1 node2
#   ./launch.sh node1 node2 --size 65536 --iterations 50000 --test throughput
#   ./launch.sh node1 node2 -s 64 -n 100000 -t latency
#
# Prerequisites:
#   - SSH key-based access to both hosts (no password prompts)
#   - rdma-test binary installed at the same path on both hosts
#   - RDMA devices available and configured on both hosts

set -euo pipefail

BINARY="${RDMA_TEST_BIN:-rdma-test}"
PORT="${RDMA_TEST_PORT:-7471}"
SERVER_STARTUP_DELAY="${RDMA_TEST_STARTUP_DELAY:-2}"

usage() {
    cat <<EOF
Usage: $0 <server_host> <client_host> [options...]

Launch an RDMA test between two hosts using SSH.

Arguments:
  server_host    Hostname or IP for the server node
  client_host    Hostname or IP for the client node

Environment variables:
  RDMA_TEST_BIN           Path to rdma-test binary (default: rdma-test)
  RDMA_TEST_PORT          Port number (default: 7471)
  RDMA_TEST_STARTUP_DELAY Seconds to wait for server startup (default: 2)

Additional options are passed directly to rdma-test (both server and client).
EOF
    exit 1
}

if [[ $# -lt 2 ]]; then
    usage
fi

SERVER_HOST="$1"
CLIENT_HOST="$2"
shift 2

EXTRA_OPTS=("$@")

cleanup() {
    echo "Stopping server on ${SERVER_HOST}..."
    ssh -o ConnectTimeout=5 "${SERVER_HOST}" \
        "pkill -f '${BINARY} server' 2>/dev/null || true" 2>/dev/null || true
}
trap cleanup EXIT

echo "=== RDMA Test Launch ==="
echo "  Server: ${SERVER_HOST}"
echo "  Client: ${CLIENT_HOST}"
echo "  Binary: ${BINARY}"
echo "  Port:   ${PORT}"
if [[ ${#EXTRA_OPTS[@]} -gt 0 ]]; then
    echo "  Options: ${EXTRA_OPTS[*]}"
fi
echo ""

echo "Starting server on ${SERVER_HOST}..."
ssh -o ConnectTimeout=10 "${SERVER_HOST}" \
    "${BINARY} server --address 0.0.0.0 --port ${PORT} ${EXTRA_OPTS[*]:-}" \
    </dev/null &>/tmp/rdma-test-server-$$.log &
SERVER_PID=$!

echo "Waiting ${SERVER_STARTUP_DELAY}s for server to initialize..."
sleep "${SERVER_STARTUP_DELAY}"

if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
    echo "ERROR: Server process exited prematurely."
    echo "Server log:"
    cat /tmp/rdma-test-server-$$.log 2>/dev/null || true
    exit 1
fi

echo "Starting client on ${CLIENT_HOST} -> ${SERVER_HOST}..."
echo ""
ssh -o ConnectTimeout=10 "${CLIENT_HOST}" \
    "${BINARY} client --address ${SERVER_HOST} --port ${PORT} ${EXTRA_OPTS[*]:-}"

CLIENT_EXIT=$?

echo ""
if [[ ${CLIENT_EXIT} -eq 0 ]]; then
    echo "=== Test completed successfully ==="
else
    echo "=== Test failed (exit code: ${CLIENT_EXIT}) ==="
    echo ""
    echo "Server log:"
    cat /tmp/rdma-test-server-$$.log 2>/dev/null || true
fi

rm -f /tmp/rdma-test-server-$$.log
exit ${CLIENT_EXIT}
