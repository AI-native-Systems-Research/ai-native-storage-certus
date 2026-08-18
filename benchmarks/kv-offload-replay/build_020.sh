#!/bin/bash
# Build all four bench images against vLLM 0.26.0 (branch: VLLM_VERSION build-arg).
# Does NOT abort on a single failure — records per-image rc so we see which
# connectors survive the version jump.
cd /home/dwaddington/ai-native-storage-certus || exit 2
BA=(--build-arg VLLM_VERSION=0.20.0)
LOG=/mnt/certus1/kvprofile-build020
mkdir -p "$LOG"

build() {
  local name="$1"; shift
  echo "[build] $name starting $(date +%H:%M:%S)"
  "$@" > "$LOG/$name.log" 2>&1
  local rc=$?
  echo "[build] $name rc=$rc"
  return 0
}

# NoOffload + CPUOffload + Tiered all share one image (run_multiturn_offloading.py
# drives all three; backend picked at run time by OFFLOAD_MODE / SECONDARY_TIER).
# NB: Tiered (SECONDARY_TIER=fs) needs vLLM 0.26+, so at 0.20 only none/cpu work.
build offload \
  podman build "${BA[@]}" -f benchmarks/kv-offload-replay/Dockerfile.offload -t certus-offload-bench .

build sharedstorage \
  podman build "${BA[@]}" -f benchmarks/kv-offload-replay/Dockerfile.sharedstorage -t certus-sharedstorage-bench .

build grpc \
  podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run \
    build "${BA[@]}" -f certus-grpc-connector/Dockerfile -t certus-grpc-bench .

echo "[build] ALL DONE $(date +%H:%M:%S)"
