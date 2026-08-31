#!/usr/bin/env bash
# Entrypoint for the inference-perf synthetic_agentic client image.
#
#   run      (default) render the config and drive load against ${BASE_URL}
#   generate               dump the deterministic replay graph for a session index
#                          (offline inspection / audit; NOT required for a run)
#
# The config is a template; BASE_URL + MODEL are injected here (envsubst) so the
# same committed config drives every backend unchanged.
set -euo pipefail

CONFIG_TMPL=${CONFIG_TMPL:-/config/synthetic_agentic.yaml}
RESULTS_DIR=${RESULTS_DIR:-/results}
MODEL=${MODEL:-NousResearch/Meta-Llama-3-8B}
BASE_URL=${BASE_URL:-http://localhost:8000}
# How many sessions to (deterministically) generate + replay. Default 200; lower
# it for a quick smoke. Because all randomness derives from (seed, session_index),
# NUM_SESSIONS=N replays sessions 0..N-1 — a byte-identical PREFIX of the full run
# (session 0 of a 10-run == session 0 of a 200-run), so a short run is a faithful
# subset. NB a small N shrinks the aggregate KV working set and may stop it
# overflowing the GPU KV budget, in which case the offload tier stays cold and all
# backends look alike — keep N large enough to spill when comparing backends.
NUM_SESSIONS=${NUM_SESSIONS:-200}

mkdir -p "${RESULTS_DIR}"
rendered="${RESULTS_DIR}/effective-config.yaml"
# Substitute ONLY our placeholders (leave any other $... in the yaml intact).
BASE_URL="${BASE_URL}" MODEL="${MODEL}" NUM_SESSIONS="${NUM_SESSIONS}" \
    envsubst '${BASE_URL} ${MODEL} ${NUM_SESSIONS}' < "${CONFIG_TMPL}" > "${rendered}"

mode=${1:-run}
case "${mode}" in
    run)
        echo ">> inference-perf: MODEL=${MODEL} BASE_URL=${BASE_URL}" >&2
        echo ">> effective config -> ${rendered}" >&2
        cd "${RESULTS_DIR}"    # any report files inference-perf writes land here
        exec python -m inference_perf.main --config "${rendered}"
        ;;
    generate)
        # Offline: build the replay graph for one session and write it out, sized
        # with the real tokenizer. Determinism is by (seed, session-index), so this
        # is exactly what a run replays for that session.
        si=${SESSION_INDEX:-0}
        out="${RESULTS_DIR}/graph-s${si}.json"
        echo ">> inference-perf: generating replay graph session=${si} -> ${out}" >&2
        exec python -m inference_perf.datagen.synthetic_agentic_to_replay_graph \
            --config "${rendered}" \
            --session-index "${si}" \
            --output "${out}" \
            --summary
        ;;
    *)
        echo "usage: entrypoint [run|generate]" >&2
        exit 2
        ;;
esac
