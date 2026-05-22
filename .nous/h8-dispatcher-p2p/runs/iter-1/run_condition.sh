#!/bin/bash
# Usage: run_condition.sh <mode> <chunk_size> <output_json>
# Runs gpu-p2p-server in the given mode, runs benchmark client, saves results to JSON.

MODE=$1
CHUNK_SIZE=$2
OUTPUT_JSON=$3

BINARY=/home/nara/certus/ai-native-storage-certus/target/debug/gpu-p2p-server
CLIENT=/home/nara/certus/ai-native-storage-certus/.nous-experiments/iter-1-c0dded0c/components/gpu-services/v0/tests/gpu_client_p2p.py
SOCK=/tmp/gpu_p2p_bench.sock
PCI=0000:63:00.0
ITERATIONS=20
TRANSFER_SIZE=4194304
STAGING_SIZE=4194304

export LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH

# Clean up stale state
rm -f /var/tmp/spdk_pci_lock_$PCI $SOCK 2>/dev/null
sleep 1

# Start server
$BINARY --socket $SOCK --pci $PCI --mode $MODE \
  --chunk-size $CHUNK_SIZE --staging-size $STAGING_SIZE \
  > /tmp/server_${MODE}_${CHUNK_SIZE}.log 2>&1 &
SERVER_PID=$!

# Wait for server startup (SPDK + CUDA + GDRCopy init)
sleep 7

# Check server is still running
if ! kill -0 $SERVER_PID 2>/dev/null; then
  echo "ERROR: Server died during startup" >&2
  cat /tmp/server_${MODE}_${CHUNK_SIZE}.log >&2
  exit 1
fi

# Run client and capture output
CLIENT_OUTPUT=$(python3 $CLIENT $TRANSFER_SIZE $SOCK --iterations $ITERATIONS 2>&1)
CLIENT_EXIT=$?

# Kill server
kill $SERVER_PID 2>/dev/null
wait $SERVER_PID 2>/dev/null
rm -f /var/tmp/spdk_pci_lock_$PCI $SOCK 2>/dev/null

if [ $CLIENT_EXIT -ne 0 ]; then
  echo "ERROR: Client failed with exit code $CLIENT_EXIT" >&2
  echo "$CLIENT_OUTPUT" >&2
  exit 1
fi

echo "$CLIENT_OUTPUT"

# Parse metrics from client output
THROUGHPUT=$(echo "$CLIENT_OUTPUT" | grep -oP 'Throughput:\s+\K[\d.]+')
AVG_LAT=$(echo "$CLIENT_OUTPUT" | grep -oP 'Avg latency:\s+\K[\d.]+')
MIN_LAT=$(echo "$CLIENT_OUTPUT" | grep -oP 'Min latency:\s+\K[\d.]+')
MAX_LAT=$(echo "$CLIENT_OUTPUT" | grep -oP 'Max latency:\s+\K[\d.]+')

# Write JSON
mkdir -p "$(dirname "$OUTPUT_JSON")"
cat > "$OUTPUT_JSON" <<EOF
{
  "mode": "$MODE",
  "chunk_size": $CHUNK_SIZE,
  "iterations": $ITERATIONS,
  "transfer_size": $TRANSFER_SIZE,
  "pci": "$PCI",
  "throughput_mbs": $THROUGHPUT,
  "avg_latency_ms": $AVG_LAT,
  "min_latency_ms": $MIN_LAT,
  "max_latency_ms": $MAX_LAT,
  "raw_output": $(echo "$CLIENT_OUTPUT" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read()))")
}
EOF

echo "Results written to $OUTPUT_JSON"
