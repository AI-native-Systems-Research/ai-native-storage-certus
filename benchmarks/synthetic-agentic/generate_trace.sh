#!/usr/bin/env bash
# Generate the deterministic synthetic_agentic replay graphs for inspection/audit.
#
# NOTE on "generate then replay": inference-perf's synthetic_agentic builds its
# sessions PROCEDURALLY at run time — all randomness derives from (seed,
# session_index), so a normal run does NOT consume a pre-generated trace file. The
# fixed `seed` in configs/synthetic_agentic.yaml is therefore what guarantees
# every backend replays a byte-identical workload ("generate once, replay for fair
# comparison"). This script materialises the graph for a few sessions via
# inference-perf's offline `synthetic_agentic_to_replay_graph` tool so you can
# inspect/diff exactly what will be replayed — it is an AUDIT artifact, not an
# input to the run.
#
# Output graphs are bench artifacts -> written under OUT_DIR (default ./results),
# never committed.
#
# Usage:
#   ./generate_trace.sh                       # sessions 0..4 -> ./results/graph-s*.json
#   NUM_SESSIONS=10 OUT_DIR=/mnt/certus1/sa ./generate_trace.sh
#   MODEL=... IMAGE=synthetic-agentic-client:latest ./generate_trace.sh
set -euo pipefail
cd "$(dirname "$0")"

ENGINE=${ENGINE:-podman}
IMAGE=${IMAGE:-synthetic-agentic-client:latest}
MODEL=${MODEL:-NousResearch/Meta-Llama-3-8B}
OUT_DIR=${OUT_DIR:-$PWD/results}
NUM_SESSIONS=${NUM_SESSIONS:-5}                 # how many session graphs to dump
HF_CACHE=${HF_CACHE:-$HOME/.cache/huggingface}

mkdir -p "${OUT_DIR}"

if ! ${ENGINE} image exists "${IMAGE}" 2>/dev/null; then
    echo ">> building ${IMAGE}" >&2
    ${ENGINE} build -f Dockerfile.inference-perf -t "${IMAGE}" .
fi

echo ">> generating ${NUM_SESSIONS} replay graphs (MODEL=${MODEL}) -> ${OUT_DIR}" >&2
for ((si=0; si<NUM_SESSIONS; si++)); do
    ${ENGINE} run --rm \
        -e MODEL="${MODEL}" \
        -e SESSION_INDEX="${si}" \
        -v "${HF_CACHE}:/root/.cache/huggingface:z" \
        -v "${OUT_DIR}:/results:z" \
        "${IMAGE}" generate
done
echo ">> done: $(ls -1 "${OUT_DIR}"/graph-s*.json 2>/dev/null | wc -l) graph(s) in ${OUT_DIR}" >&2
