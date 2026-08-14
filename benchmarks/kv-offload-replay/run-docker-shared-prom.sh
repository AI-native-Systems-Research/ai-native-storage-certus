#!/bin/bash
# SharedStorage + Prometheus — llmd_fs_backend on a host filesystem (image
# certus-sharedstorage-bench) with the bench's vLLM engine exposing Prometheus
# metrics on port 8000.
#
# The bench drives vLLM through the offline LLM(...) engine (no OpenAI server),
# so there is no /metrics endpoint unless the driver opens one. This variant
# sets LOG_STATS=1 + PROM_PORT=8000, publishes -p 8000:8000, and bind-mounts the
# repo driver over the baked one so the exporter works WITHOUT rebuilding.
#
# Scrape from the host at:  http://127.0.0.1:8000/metrics  (IPv4, not localhost)
#
#   ./run-docker-shared-prom.sh
#   SHARED_FS=/mnt/ss-kv DISK_DEV=md0 ./run-docker-shared-prom.sh
#
# NOTE (vLLM 0.26.0): the sharedstorage image build currently FAILS (upstream
# removed vllm.v1.kv_offload.abstract, which llmd_fs_backend imports), so the
# certus-sharedstorage-bench tag is still the 0.20 image. prometheus_client is
# present there (run_fs_bench_450.py already reads its REGISTRY), so the exporter
# works; only the vLLM metric names differ from newer releases.
#
# NOTE (perms): the container runs as a rootless-mapped uid; SHARED_FS and its
# shared-kv/ subdir must be writable by that uid or the run silently degrades to
# no-offload. See tools/configure-bench.sh for the RAID0/XFS + ownership setup.
source "$(dirname "${BASH_SOURCE[0]}")/run-docker-common.sh"

IMAGE="${IMAGE:-certus-sharedstorage-bench}"
SHARED_FS="${SHARED_FS:-/mnt/ss-kv}"
DRAM="${DRAM:-$((32 * (1 << 30)))}"                 # host-RAM staging pool (bytes)
DISK_DEV="${DISK_DEV:-$(findmnt -no SOURCE --target "$SHARED_FS" 2>/dev/null | xargs -r basename)}"
[[ -z "$DISK_DEV" ]] && DISK_DEV="md0"
LOG="${LOG:-${SCRIPT_DIR}/sharedstorage_prom_$(stamp).log}"
DRIVER="${DRIVER:-${SCRIPT_DIR}/run_fs_bench_450.py}"

# ── Prometheus wiring (the only substantive difference from the base script) ──
PROM_PORT="${PROM_PORT:-8000}"        # host port the exporter is published on
LOG_STATS="${LOG_STATS:-1}"           # 1 = register vLLM metrics (empty /metrics otherwise)

require_image "$IMAGE"
[[ -f "$DRIVER" ]] || { echo "error: driver not found at ${DRIVER}" >&2; exit 1; }
mkdir -p "${SHARED_FS}/shared-kv"

echo "[sharedstorage] prometheus exporter -> http://127.0.0.1:${PROM_PORT}/metrics"
run_container "$LOG" "$IMAGE" \
  -p "${PROM_PORT}:${PROM_PORT}" \
  -e "PROM_PORT=${PROM_PORT}" \
  -e "LOG_STATS=${LOG_STATS}" \
  -v "${SHARED_FS}:/mnt/fs-backend-bench:z" \
  -v "${DRIVER}:/workspace/bench/run_fs_bench_450.py:z" \
  -e "DRAM=${DRAM}" \
  -e "DISK_DEV=${DISK_DEV}" \
  -e "SKIP_PREFLIGHT=1"
