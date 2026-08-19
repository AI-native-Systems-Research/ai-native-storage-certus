#!/bin/bash
# Build all four bench images against vLLM 0.26.0 (branch: VLLM_VERSION build-arg).
# Does NOT abort on a single failure — records per-image rc so we see which
# connectors survive the version jump.
cd /home/dwaddington/ai-native-storage-certus || exit 2
BA=(--build-arg VLLM_VERSION=0.26.0)
LOG=/mnt/certus1/kvprofile-build026
mkdir -p "$LOG"

build() {
  local name="$1"; shift
  echo "[build] $name starting $(date +%H:%M:%S)"
  "$@" > "$LOG/$name.log" 2>&1
  local rc=$?
  echo "[build] $name rc=$rc"
  return 0
}

build nooffload \
  podman build "${BA[@]}" -f benchmarks/kv-offload-replay/Dockerfile.nooffload -t certus-nooffload-bench .

build cpuoffload \
  podman build "${BA[@]}" -f benchmarks/kv-offload-replay/Dockerfile.cpu-offload -t certus-cpu-offload-bench .

build sharedstorage \
  podman build "${BA[@]}" -f benchmarks/kv-offload-replay/Dockerfile.sharedstorage -t certus-sharedstorage-bench .

build shmq \
  podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run \
    build "${BA[@]}" -f certus-shmq-connector/Dockerfile -t certus-shmq-bench .

echo "[build] ALL DONE $(date +%H:%M:%S)"
