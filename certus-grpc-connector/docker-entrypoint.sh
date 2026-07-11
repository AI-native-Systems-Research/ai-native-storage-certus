#!/bin/bash
# Client-side entrypoint: wait for the external certus-server to accept gRPC
# connections, then run the multi-turn workload through the connector.
#
# The server runs separately (owns SPDK/NVMe); this container only drives vLLM
# and offloads over gRPC to CERTUS_SERVER.
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/certus-grpc-connector/run_multiturn_grpc_certus.py}"
WAIT_SECS="${WAIT_SECS:-120}"

host="${CERTUS_SERVER%%:*}"
port="${CERTUS_SERVER##*:}"

echo "[entrypoint] waiting up to ${WAIT_SECS}s for certus-server at ${CERTUS_SERVER} ..."
for i in $(seq 1 "${WAIT_SECS}"); do
    if (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null; then
        exec 3>&- 3<&-
        echo "[entrypoint] server reachable (after ${i}s)"
        break
    fi
    sleep 1
    if [ "${i}" -eq "${WAIT_SECS}" ]; then
        echo "[entrypoint] ERROR: certus-server at ${CERTUS_SERVER} not reachable in ${WAIT_SECS}s." >&2
        echo "[entrypoint]        Start the server on the host and/or check CERTUS_SERVER." >&2
        exit 1
    fi
done

echo "[entrypoint] running workload (NUM_CONVS=${NUM_CONVS}, MODEL=${MODEL}, server=${CERTUS_SERVER})"
exec python3 "${WORKLOAD}"
