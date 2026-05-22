#!/bin/bash
# run_condition.sh MODE OUTPUT_FILE
# Starts server in given mode, runs 20-iteration client benchmark, saves output.
set -e
MODE="$1"
OUTPUT="$2"
WORKTREE="/home/nara/certus/ai-native-storage-certus/.nous-experiments/iter-2-be2ce320"
PCI="0000:63:00.0"
SOCKET="/tmp/gpu_p2p_bench.sock"
CHUNK=131072
STAGING=4194304

# Kill any running server
pkill -x gpu-p2p-server 2>/dev/null || true
sleep 2
rm -f "/var/tmp/spdk_pci_lock_${PCI}" "$SOCKET"

# Start server
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
"$WORKTREE/target/debug/gpu-p2p-server" \
  --socket "$SOCKET" --pci "$PCI" --mode "$MODE" \
  --chunk-size "$CHUNK" --staging-size "$STAGING" \
  > "/tmp/server_${MODE}.log" 2>&1 &
SERVER_PID=$!

# Wait for startup (p2p modes need longer for pool allocation)
sleep 7

# Verify server is up
if ! kill -0 $SERVER_PID 2>/dev/null; then
  echo "ERROR: Server failed to start" >&2
  cat "/tmp/server_${MODE}.log" >&2
  exit 1
fi

# Run client
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 "$WORKTREE/components/gpu-services/v0/tests/gpu_client_p2p.py" \
  4194304 "$SOCKET" --iterations 20 2>&1 | tee "$OUTPUT"

# Kill server
pkill -x gpu-p2p-server 2>/dev/null || true
sleep 2
rm -f "/var/tmp/spdk_pci_lock_${PCI}" "$SOCKET"
