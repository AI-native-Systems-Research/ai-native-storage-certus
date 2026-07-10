#!/bin/bash
# Launch certus-server on the configured NVMe drives, wait for it to accept
# gRPC, then run the multi-turn workload through the gRPC connector.
#
# Server config mirrors certus-connector/run_multiturn_certus.py:
#   drives  0000:c1-c4:00.0   (DEVICE_PCI)
#   slab    2 MiB             (SLAB_SIZE_BYTES, passed to the workload)
#   tier    44 GiB            (MEMORY_TIER_SIZE)
#   pollers pinned from NUMA-1 base core (POLLER_BASE_CPU)
set -euo pipefail

LISTEN="${LISTEN:-0.0.0.0:50051}"
SERVER_BIN=/src/target/release/certus-server
LOG=/tmp/certus-server.log

# --device-pci is repeatable; expand the space-separated DEVICE_PCI list.
pci_args=()
for a in ${DEVICE_PCI}; do pci_args+=(--device-pci "$a"); done

echo "[entrypoint] starting certus-server on ${LISTEN}"
echo "[entrypoint]   drives: ${DEVICE_PCI}"
echo "[entrypoint]   memory-tier: ${MEMORY_TIER_SIZE}  poller-base-cpu: ${POLLER_BASE_CPU}"

"${SERVER_BIN}" \
    --listen "${LISTEN}" \
    "${pci_args[@]}" \
    --memory-tier-size "${MEMORY_TIER_SIZE}" \
    --poller-base-cpu "${POLLER_BASE_CPU}" \
    --format \
    > "${LOG}" 2>&1 &
SERVER_PID=$!

cleanup() {
    echo "[entrypoint] stopping certus-server (pid ${SERVER_PID})"
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
}
trap cleanup EXIT

# Wait for the server to bind and start accepting connections. Fail fast if the
# process dies (SPDK/NVMe/hugepage misconfig shows up here).
host="${CERTUS_SERVER%%:*}"
port="${CERTUS_SERVER##*:}"
echo "[entrypoint] waiting for ${CERTUS_SERVER} ..."
for i in $(seq 1 120); do
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
        echo "[entrypoint] ERROR: certus-server exited during startup. Log:" >&2
        tail -40 "${LOG}" >&2
        exit 1
    fi
    if (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null; then
        exec 3>&- 3<&-
        echo "[entrypoint] server is up (after ${i}s)"
        break
    fi
    sleep 1
    if [ "${i}" -eq 120 ]; then
        echo "[entrypoint] ERROR: server did not become ready in 120s. Log:" >&2
        tail -40 "${LOG}" >&2
        exit 1
    fi
done

echo "[entrypoint] running workload (NUM_CONVS=${NUM_CONVS}, MODEL=${MODEL})"
exec python3 /src/certus-grpc-connector/run_multiturn_grpc_certus.py
