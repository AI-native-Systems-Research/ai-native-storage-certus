#!/bin/bash
# CPUOffload — vLLM OffloadingConnector -> host RAM (unified image certus-offload-bench,
# default OFFLOAD_MODE => CPU-only offload).
#   ./run-docker-cpu.sh
#   CPU_BYTES=$((32*(1<<30))) ./run-docker-cpu.sh   # larger host-RAM KV pool
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-offload-bench}"
CPU_BYTES="${CPU_BYTES:-$((16 * (1 << 30)))}"   # host-RAM KV pool (bytes)
LOG="${LOG:-${SCRIPT_DIR}/cpu_offload_$(stamp).log}"

require_image "$IMAGE"
run_container "$LOG" "$IMAGE" -e "CPU_BYTES=${CPU_BYTES}"
