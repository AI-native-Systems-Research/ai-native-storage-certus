#!/bin/bash
# Run a single benchmark condition: start server, run client, save output, kill server.
# Usage: run_condition.sh <mode> <output_file> [<pci_addr>]
set -euo pipefail

MODE="$1"
OUTPUT="$2"
PCI="${3:-0000:62:00.0}"
SOCKET="/tmp/gpu_p2p_bench_$$_${MODE}.sock"
WORKTREE="/home/nara/certus/ai-native-storage-certus/.nous-experiments/iter-2-071136f1"
SERVER_BIN="${WORKTREE}/target/debug/gpu-p2p-server"
CLIENT_PY="${WORKTREE}/components/gpu-services/v0/tests/gpu_client_p2p.py"

export LD_LIBRARY_PATH="/usr/local/lib:/usr/local/cuda/lib64:${LD_LIBRARY_PATH:-}"

# Start server
"$SERVER_BIN" \
  --mode "$MODE" \
  --chunk-size 131072 \
  --staging-size 4194304 \
  --pci "$PCI" \
  --socket "$SOCKET" > "/tmp/server_${MODE}_$$.log" 2>&1 &
SERVER_PID=$!

# Wait for server to initialize
sleep 5

# Verify server is still running
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
  echo "ERROR: Server failed to start" >&2
  cat "/tmp/server_${MODE}_$$.log" >&2
  exit 1
fi

# Run benchmark client
set +e
python3 "$CLIENT_PY" 4194304 "$SOCKET" --iterations 50 > "$OUTPUT" 2>&1
CLIENT_EXIT=$?
set -e

# Kill server
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
rm -f "$SOCKET"
rm -f "/tmp/server_${MODE}_$$.log"

if [ "$CLIENT_EXIT" -ne 0 ]; then
  echo "ERROR: Client exited with code $CLIENT_EXIT" >&2
  cat "$OUTPUT" >&2
  exit 1
fi

echo "Condition '$MODE' complete. Output at: $OUTPUT"
grep -E "Throughput|Avg latency|Min latency|Max latency" "$OUTPUT" || true
