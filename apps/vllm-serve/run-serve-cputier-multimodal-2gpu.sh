#!/bin/bash
# run-serve-cputier-multimodal-2gpu.sh — SERVER side of the Qwen3-VL-32B
# multimodal KV-offload test on the CPUTIER (vLLM-native tiering) backend,
# sharded across BOTH A100s.
#
# This is the cputier counterpart of run-serve-certus-shmq-multimodal-2gpu.sh:
# same model, parallelism, window and multimodal limits, but the KV-offload tier
# is vLLM 0.26's in-tree OffloadingConnector -> TieringOffloadingSpec (a CPU
# primary tier + an "fs" disk secondary tier) instead of the Certus-SHMQ
# connector. Run the two wrappers side by side to compare backends over the same
# /v1/chat/completions surface with an identical synthetic multimodal workload.
#
# Thin wrapper over run-serve-cputier.sh — it only sets the model / parallelism /
# multimodal env and inherits all the tiering + container plumbing (CPU tier mmap
# via --shm-size, fs disk tier bind-mount, fix026 image, port publishing) from
# that base script.
#
# Why these values (kept in lockstep with the shmq wrapper for comparability):
#   MODEL=Qwen/Qwen3-VL-32B-Instruct-FP8
#                         FP8 checkpoint + DTYPE=auto. The fp16 32B weights
#                         (~64G) do NOT fit 2x40G A100 once you add KV + vision
#                         activations, so use the quantized checkpoint. Override
#                         MODEL= + DTYPE= to serve a different one.
#   TENSOR_PARALLEL=2     shard the 32B weights across both A100s (~half each).
#                         Requires GPU=all so both devices are visible.
#   GPU=all               expose both A100s to the container.
#   MAX_MODEL_LEN=32768   modest window to keep KV pressure down given the large
#                         weights; a single 720p image already costs many tokens.
#                         Qwen3-VL's native window is far larger (256K) so no YaRN
#                         is needed — bump this if prompts/history or image counts
#                         grow (watch GPU memory).
#   GPU_MEM_UTIL=0.80     leave real HBM headroom for the vision-encoder
#                         activation spike. vLLM profiles peak activation
#                         text-only at startup and hands the rest to the KV pool;
#                         at 0.92 that leaves ~0 free and a 720p vision forward
#                         OOMs mid-run. 0.80 caps KV so ~7G/GPU stays free.
#                         (Verified on the shmq wrapper; same model/vision path.)
#
# cputier tiers (inherited from run-serve-cputier.sh, all env-overridable):
#   CPU_BYTES=30G         CPU primary tier (pinned host RAM), matching the shmq
#                         server's 30G memory tier for a fair comparison.
#   FS_TIER=1             fs disk secondary tier under ${FS_TIER_HOST}
#                         (default /mnt/certus1/kv-fs-tier). FS_TIER=0 = CPU-only.
#   The base sizes the container /dev/shm to CPU_BYTES + 4G for the tier mmap.
#
# Multimodal: --limit-mm-per-prompt allows up to MM_IMAGES images per prompt,
# passed through the base script's EXTRA_SERVE_ARGS hook. JSON is compact (no
# spaces) so it survives the base word-split.
#
# Prereqs: the certus-offload-bench-fix026 image in the DEFAULT podman store
# (build_026.sh), both A100s free, and enough host RAM for the CPU tier. Unlike
# the shmq server there is NO external certus-server and NO mailbox — cputier is
# self-contained. Pair with the multimodal guidellm driver once the server is up.
#
#   ./run-serve-cputier-multimodal-2gpu.sh
#   PORT=9000 ./run-serve-cputier-multimodal-2gpu.sh
#   CPU_BYTES=$((16*(1<<30))) ./run-serve-cputier-multimodal-2gpu.sh   # smaller CPU tier
#   FS_TIER=0 ./run-serve-cputier-multimodal-2gpu.sh                   # CPU-only offload (no disk tier)
#   MM_IMAGES=1 ./run-serve-cputier-multimodal-2gpu.sh                 # cap images/prompt
#   MODEL=Qwen/Qwen2.5-VL-7B-Instruct DTYPE=auto ./run-serve-cputier-multimodal-2gpu.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Images allowed per prompt (compact JSON — no spaces — for the base word-split).
MM_IMAGES="${MM_IMAGES:-2}"
MM_ARGS="--limit-mm-per-prompt {\"image\":${MM_IMAGES}}"

CPU_BYTES=$((30*(1<<30))) # 30GiB
MODEL="${MODEL:-Qwen/Qwen3-VL-32B-Instruct-FP8}" \
DTYPE="${DTYPE:-auto}" \
SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen3-vl-32b}" \
MAX_MODEL_LEN="${MAX_MODEL_LEN:-32768}" \
TENSOR_PARALLEL="${TENSOR_PARALLEL:-2}" \
GPU="${GPU:-all}" \
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.80}" \
EXTRA_SERVE_ARGS="${EXTRA_SERVE_ARGS:-${MM_ARGS}}" \
  exec "${SCRIPT_DIR}/run-serve-cputier.sh"
