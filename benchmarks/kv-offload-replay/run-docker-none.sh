#!/bin/bash
# NoOffload — GPU-only baseline (unified image certus-offload-bench, OFFLOAD_MODE=none).
#   ./run-docker-none.sh                 # defaults (450 convs, Llama-3-8B)
#   NUM_CONVS=100 ./run-docker-none.sh   # override any workload var
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-offload-bench}"
LOG="${LOG:-${SCRIPT_DIR}/nooffload_$(stamp).log}"

require_image "$IMAGE"
run_container "$LOG" "$IMAGE" -e "OFFLOAD_MODE=none"
