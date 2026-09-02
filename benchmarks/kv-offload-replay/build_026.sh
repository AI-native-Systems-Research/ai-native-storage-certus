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

# NoOffload + CPUOffload + Tiered all share one image (run_multiturn_offloading.py
# drives all three; backend picked at run time by OFFLOAD_MODE / SECONDARY_TIER).
# NB: Dockerfile.offload now bakes the tiering fix BY DEFAULT. This arm is the
# deliberate STOCK/crashing baseline for the head-to-head below, so it opts OUT
# explicitly (--build-arg VLLM_FIX_TIERING=0) to reproduce the upstream
# _req_state KeyError crash. (Everything that isn't this baseline should just
# take the default and get the fix.)
build offload \
  podman build "${BA[@]}" --build-arg VLLM_FIX_TIERING=0 \
    -f benchmarks/kv-offload-replay/Dockerfile.offload -t certus-offload-bench .

# Same image with the forked tiering fix baked in (VLLM_FIX_TIERING=1 — now the
# Dockerfile default; passed explicitly here for clarity). The "patched" arm in
# the head-to-head vs the stock certus-offload-bench above. Pure-Python overlay,
# so no vLLM source rebuild — see Dockerfile.offload + vllm-fix2/PROVENANCE.md.
build offload-fix026 \
  podman build "${BA[@]}" --build-arg VLLM_FIX_TIERING=1 \
    -f benchmarks/kv-offload-replay/Dockerfile.offload -t certus-offload-bench-fix026 .

build sharedstorage \
  podman build "${BA[@]}" -f benchmarks/kv-offload-replay/Dockerfile.sharedstorage -t certus-sharedstorage-bench .

build shmq \
  podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run \
    build "${BA[@]}" -f certus-shmq-connector/Dockerfile -t certus-shmq-bench .

echo "[build] ALL DONE $(date +%H:%M:%S)"
