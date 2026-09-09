#!/bin/bash
# Client-side entrypoint for the kv-offload-otel-replay shmq image
# (certus-otel-shmq-bench). The OTel-corpus counterpart of
# certus-shmq-connector/docker-entrypoint.sh.
#
# Two run-time preconditions, checked in order:
#   1. The OTel corpus is bind-mounted on OTEL_DIR (it is NOT baked in, ~22 GB).
#   2. The external certus-server has published its /dev/shm mailbox (file present
#      + header READY). The server runs separately (owns SPDK/NVMe) and creates
#      SHM_PATH; this container shares the host's /dev/shm via --ipc=host, so the
#      same mailbox file is visible here. Ring(...) mmaps the file and spins on
#      the header READY magic, so a successful attach IS the readiness signal.
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/certus-shmq-connector/run_otel_shmq_certus.py}"
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"
OTEL_DIR="${OTEL_DIR:-/workspace/otel-corpus}"
WAIT_SECS="${WAIT_SECS:-120}"

# ── 1. Corpus bind-mount check (corpus is not baked in) ───────────────────────
if [ ! -d "${OTEL_DIR}" ] || [ -z "$(ls -A "${OTEL_DIR}" 2>/dev/null)" ]; then
    echo "[entrypoint] ERROR: OTEL_DIR=${OTEL_DIR} is empty or missing." >&2
    echo "[entrypoint]        The OTel corpus is NOT baked into this image (~22 GB)." >&2
    echo "[entrypoint]        Bind-mount it read-only, e.g.:" >&2
    echo "[entrypoint]          -v /mnt/certus1/inference-perf-syn-data/otel_1k:${OTEL_DIR}:ro" >&2
    exit 1
fi
n_files="$(ls -1 "${OTEL_DIR}"/*.json 2>/dev/null | wc -l | tr -d ' ')"
echo "[entrypoint] corpus ${OTEL_DIR}: ${n_files} json files  (NUM_CONVS=${NUM_CONVS:-ALL} TIME_SCALE=${TIME_SCALE:-1.0})"

# ── 2. Wait for the certus-server mailbox ─────────────────────────────────────
echo "[entrypoint] waiting up to ${WAIT_SECS}s for certus-server mailbox at ${SHM_PATH} ..."
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
    echo "[entrypoint]        Start certus-server on the host (it creates ${SHM_PATH})" >&2
    echo "[entrypoint]        and check --ipc=host so this container shares /dev/shm." >&2
    exit 1
fi
echo "[entrypoint] mailbox ready."

echo "[entrypoint] running workload (MODEL=${MODEL}, shm_path=${SHM_PATH}, OTEL_DIR=${OTEL_DIR})"
exec python3 "${WORKLOAD}"
