#!/bin/bash
# run-serve-cputier-mixtral-2gpu.sh — SERVER side of the Mixtral-8x7B (MoE)
# KV-offload test on the CPUTIER (vLLM-native tiering) backend, sharded across
# BOTH A100s. TEXT-ONLY (Mixtral is not multimodal).
#
# This is the cputier counterpart of run-serve-certus-shmq-mixtral-2gpu.sh: same
# model, parallelism and window, but the KV-offload tier is vLLM 0.26's in-tree
# OffloadingConnector -> TieringOffloadingSpec (a CPU primary tier + an "fs" disk
# secondary tier) instead of the Certus-SHMQ connector. Run the two wrappers side
# by side to compare backends over an identical synthetic text workload.
#
# Thin wrapper over run-serve-cputier.sh — it only sets the model / parallelism /
# window env and inherits all the tiering + container plumbing (CPU tier mmap via
# --shm-size, fs disk tier bind-mount, fix026 image, port publishing) from that
# base script. NOTE: unlike the Qwen3-VL wrappers there is NO --limit-mm-per-prompt
# (EXTRA_SERVE_ARGS is left unset) — Mixtral takes text only.
#
# Why these values (kept in lockstep with the shmq wrapper for comparability):
#   MODEL=RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8
#                         Mixtral-8x7B is a sparse MoE (8 experts, top-2; 46.7B
#                         total params, ~12.9B active/token). The bf16 weights are
#                         ~93G and do NOT fit 2x40G A100. The RedHatAI FP8
#                         checkpoint (compressed-tensors, static per-tensor FP8;
#                         router gates + lm_head kept in bf16) is ~47G -> ~23.5G
#                         per GPU at TP=2, leaving real KV headroom. Ungated
#                         re-upload of mistralai/Mixtral-8x7B-Instruct-v0.1.
#                         Override MODEL= + DTYPE= to serve a different one.
#   DTYPE=auto            read the compressed-tensors FP8 config from the
#                         checkpoint; do NOT force float16 (that would try to load
#                         bf16 weights and OOM).
#   TENSOR_PARALLEL=2     shard the experts + attention across both A100s.
#                         Requires GPU=all so both devices are visible.
#   GPU=all               expose both A100s to the container.
#   MAX_MODEL_LEN=32768   Mixtral's native window. The synthetic multi-turn
#                         workload accumulates ~19K tokens of history, which fits.
#                         (No YaRN — the base script only rope-scales qwen2 models,
#                         and 32768 is native here anyway.)
#   GPU_MEM_UTIL=0.85     higher than the VL wrapper's 0.80: Mixtral is text-only,
#                         so there is no vision-encoder activation spike to leave
#                         HBM for. ~23.5G weights/GPU + this cap leaves ~8G/GPU for
#                         the KV pool (~100K tokens), plenty for 32 streams @ 32K.
#
# cputier tiers (inherited from run-serve-cputier.sh, all env-overridable):
#   CPU_BYTES=30G         CPU primary tier (pinned host RAM), matching the shmq
#                         server's 30G memory tier for a fair comparison.
#   FS_TIER=1             fs disk secondary tier under ${FS_TIER_HOST}
#                         (default /mnt/certus1/kv-fs-tier). FS_TIER=0 = CPU-only.
#   The base sizes the container /dev/shm to CPU_BYTES + 4G for the tier mmap.
#
# Prereqs: the certus-offload-bench-fix026 image in the DEFAULT podman store
# (build_026.sh), both A100s free, enough host RAM for the CPU tier, and the
# Mixtral FP8 checkpoint reachable (HF cache on /mnt/certus1). Unlike the shmq
# server there is NO external certus-server and NO mailbox — cputier is
# self-contained. Pair with the text guidellm driver once the server is up.
#
#   ./run-serve-cputier-mixtral-2gpu.sh
#   PORT=9000 ./run-serve-cputier-mixtral-2gpu.sh
#   CPU_BYTES=$((16*(1<<30))) ./run-serve-cputier-mixtral-2gpu.sh   # smaller CPU tier
#   FS_TIER=0 ./run-serve-cputier-mixtral-2gpu.sh                   # CPU-only offload (no disk tier)
#   GPU_MEM_UTIL=0.80 ./run-serve-cputier-mixtral-2gpu.sh           # more conservative HBM
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

CPU_BYTES=$((30*(1<<30))) # 30GiB — match the shmq memory tier
MODEL="${MODEL:-RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8}" \
DTYPE="${DTYPE:-auto}" \
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-mixtral-8x7b}" \
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}" \
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}" \
GPU="${GPU:-all}" \
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.85}" \
CPU_BYTES="${CPU_BYTES}" \
  exec "${SCRIPT_DIR}/run-serve-cputier.sh"
