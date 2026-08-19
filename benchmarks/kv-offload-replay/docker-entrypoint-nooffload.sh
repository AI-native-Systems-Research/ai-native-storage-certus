#!/bin/bash
# Entrypoint for the NO-OFFLOAD (GPU-only) baseline image.
#
# This baseline is fully self-contained: no external server, no shmq mailbox, and no
# offload tier at all. vLLM runs with no kv_transfer_config, so evicted KV is
# recomputed on the GPU. There is nothing to wait for and nothing to size — we
# just log the config, then exec the driver.
set -euo pipefail

WORKLOAD="${WORKLOAD:-/workspace/bench/run_multiturn_nooffload.py}"

echo "[entrypoint] NO-OFFLOAD run: NUM_CONVS=${NUM_CONVS:-?} MODEL=${MODEL:-?} DATASET_PATH=${DATASET_PATH:-?}"
exec python3 "${WORKLOAD}"
