# Sourced by every script here: where things are, and which cluster they run
# against. The defaults are the cluster the recorded runs used (node2 + node5);
# set any of these in the environment to point elsewhere.
ARMS_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# The checkout whose deploy/llm-d/install.sh and benchmarks/ the runs use. The
# recorded runs used tag backup/kv-offload-arms-tree; check it out to repeat
# them on exactly that code.
REPO=${REPO:-$(git -C "$ARMS_DIR" rev-parse --show-toplevel)}
# Run directories, warm-up records, install logs and sequence logs. Outside the
# repo: run_arm.sh replaces its run directory, and runs are tens of MB.
OUT_ROOT=${OUT_ROOT:-$HOME/kv-offload-arms-runs}
NS=${NS:-llm-d-quickstart}               # llm-d namespace
ROUTER=${ROUTER:-http://node2:30080}     # the router's NodePort
mkdir -p "$OUT_ROOT"
PREFILL_NODE=${PREFILL_NODE:-node5}      # short name of the node prefill is pinned to (pin_pd)
# A node's kubernetes.io/hostname label, which may be its full name, from its short name.
node_label(){ kubectl get nodes -o name | sed 's|^node/||' | grep -m1 -E "^$1(\.|$)"; }
