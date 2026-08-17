#!/bin/bash
# Client-side entrypoint: wait for the external certus-shmq-server to publish its
# /dev/shm mailbox (file present + header READY), then run the multi-turn
# workload through the shared-memory connector.
#
# The server runs separately (owns SPDK/NVMe) and creates SHM_PATH. This
# container shares the host's /dev/shm via --ipc=host, so the same mailbox file
# is visible here. Unlike the gRPC entrypoint (which polled a TCP port), we wait
# on the mailbox itself: Ring(...) mmaps the file and spins on the header READY
# magic, so a successful attach IS the readiness signal — same check the real
# connector performs, no separate probe to drift out of sync.
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/certus-shmq-connector/run_multiturn_shmq_certus.py}"
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
WAIT_SECS="${WAIT_SECS:-120}"

echo "[entrypoint] waiting up to ${WAIT_SECS}s for certus-shmq-server mailbox at ${SHM_PATH} ..."
if ! python3 - "$SHM_PATH" "$WAIT_SECS" <<'PY'
import sys
from certus_shmq_connector.ring import Ring, RingError

path, wait = sys.argv[1], float(sys.argv[2])
try:
    # ready_timeout spins on the header READY magic; a clean attach means the
    # server has created + published the mailbox. Close immediately (no channel
    # is claimed until a request is issued, so this leaves no state behind).
    Ring(path, ready_timeout=wait).close()
except RingError as e:
    print(f"[entrypoint] ERROR: mailbox at {path} not ready: {e}", file=sys.stderr)
    sys.exit(1)
PY
then
    echo "[entrypoint]        Start certus-shmq-server on the host (it creates ${SHM_PATH})" >&2
    echo "[entrypoint]        and check --ipc=host so this container shares /dev/shm." >&2
    exit 1
fi
echo "[entrypoint] mailbox ready."

echo "[entrypoint] running workload (NUM_CONVS=${NUM_CONVS}, MODEL=${MODEL}, shm_path=${SHM_PATH})"
exec python3 "${WORKLOAD}"
