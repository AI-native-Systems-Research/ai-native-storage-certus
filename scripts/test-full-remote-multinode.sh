#!/usr/bin/env bash
#
# Multi-node RDMA hardware test for the Certus remote-lookup cluster path.
#
# Starts a full-remote `certus-server-yaml` on each named node (all sharing one
# zyre group), populates cache entries on the first node (the holder), then
# looks them up from the second node (the requester) and PROVES the value
# crossed the fabric via one-sided RDMA rather than being served locally.
#
# This is a lab test: it needs real RDMA NICs, GPUs, and machines that only
# exist in your environment. It is intentionally NOT a `cargo test` target and
# hard-codes NO machine names — you pass them in at invocation.
#
# Usage:
#   ./test-full-remote-multinode.sh [options] <holder> <requester> [extra-node ...]
#   CERTUS_TEST_NODES="h r ..." ./test-full-remote-multinode.sh [options]
#
# Options (all also settable via the env vars in brackets):
#   --keys A-B|list      Keys to populate/lookup (default 1-16).
#   --object-size SIZE   Per-key size, e.g. 64K/4M (default 1M).
#   --server-args "..."  Args passed to every server, esp. device selection
#                        (default "--drive-count 1"). [CERTUS_TEST_SERVER_ARGS]
#                        NOTE: --format is ALWAYS appended (whatever you pass
#                        here) — it wipes the extent managers on startup so each
#                        run gets a clean keyspace. Without it, keys populated by
#                        a prior run persist on the NVMe drives (recovered at
#                        startup) and populate fails with "key already exists".
#                        The test cannot run correctly without it, so it is not
#                        optional.
#   --verify             Check DMA'd bytes match the holder's per-key pattern.
#   --python PATH        Python interpreter on each node (default python3).
#   -h, --help           Show this help.
#
# Environment:
#   CERTUS_SERVER_BIN        certus-server-yaml path on each node
#                            (default <repo>/target/release/certus-server-yaml)
#   CERTUS_LD_LIBRARY_PATH   dirs holding libzyre/libczmq/libzmq on each node
#                            (default <repo>/deps/zyre-build/{lib,lib64}); the
#                            server binary is not rpath'd against them.
#   CERTUS_TEST_GROUP        shared zyre group (default clustertest_<uid>_<pid>,
#                            unique so concurrent testers do not collide)
#   CERTUS_TEST_GRPC_PORT    gRPC listen port on every node (default 50051)
#   CERTUS_RDMA_BIND_IP      RoCE IPv4 the responder binds (default: auto-detect)
#   CERTUS_RL_OP_DEADLINE_MS overall op deadline (default 5000; the built-in
#                            50ms is too low for cold RDMA connects). This is how
#                            long the actor keeps fetching/retrying + publishing,
#                            NOT how long the caller blocks (see CALLER_WAIT_MS).
#   CERTUS_RL_PHASE1_MS      Phase-1 memory-quorum timeout (default 500)
#   CERTUS_RL_CALLER_WAIT_MS how long batch_lookup blocks the caller before
#                            returning NotFound (default: empty = coupled to
#                            op_deadline, so this single-pass test waits for each
#                            key). Set a short value to exercise the async-fill
#                            path (caller returns fast, background op fills the
#                            cache); a single-pass run then reports cold-connect
#                            misses, which is expected, not a failure.
#   CERTUS_RL_TEARDOWN_MS    grace after finalize before an orphaned landing slot
#                            is force-reclaimed (peer QP torn down, then freed)
#                            (default 2000). Empty = server default (1000).
#   CERTUS_TEST_RUST_LOG     RUST_LOG level for every launched server
#                            (default info; set debug for per-write RDMA traces)
#   CERTUS_TEST_SSH_OPTS     extra ssh options
#
# Prerequisites (per node, same as tools/rdma-test/scripts/launch.sh style):
#   - Passwordless SSH to every named node.
#   - certus-server-yaml built and present at $CERTUS_SERVER_BIN (same path on
#     every node); build with scripts/build-certus-full-remote-spdk.sh.
#   - Hugepages/vfio configured if using SPDK NVMe (scripts/spdk-scripts/
#     cfg_user_spdk.sh, bind_vfio.sh); an up RoCE/IB device; a CUDA GPU + libcudart.
#   - python3 with grpcio (apps/python/requirements.txt).
#   - All nodes on one L2 subnet (zyre UDP-beacon discovery).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PYDIR="$REPO_ROOT/apps/python"

# Cluster launch/wait/teardown is shared with bench-remote-lookup-multinode.sh.
# shellcheck source=lib/cluster-launch.sh
. "$SCRIPT_DIR/lib/cluster-launch.sh"

# --- defaults (env-overridable) ---
SERVER_BIN="${CERTUS_SERVER_BIN:-$REPO_ROOT/target/release/certus-server-yaml}"
# The binary is NOT rpath'd against the in-tree zyre/czmq/zmq build, so every
# launched server needs these on its LD_LIBRARY_PATH. Split across lib/ (zyre,
# czmq) and lib64/ (zmq). Assumes the same repo layout on every node, exactly
# like the SERVER_BIN same-path assumption above.
LIB_PATH="${CERTUS_LD_LIBRARY_PATH:-$REPO_ROOT/deps/zyre-build/lib:$REPO_ROOT/deps/zyre-build/lib64}"
GROUP="${CERTUS_TEST_GROUP:-clustertest_${UID:-$(id -u)}_$$}"
GRPC_PORT="${CERTUS_TEST_GRPC_PORT:-50051}"
RDMA_BIND_IP="${CERTUS_RDMA_BIND_IP:-}"
OP_DEADLINE_MS="${CERTUS_RL_OP_DEADLINE_MS:-5000}"
PHASE1_MS="${CERTUS_RL_PHASE1_MS:-500}"
# Caller-wait (short) and connection-teardown grace (long), decoupled from the
# op deadline. CALLER_WAIT defaults to EMPTY so the caller stays coupled to
# op_deadline — this test verifies the data path synchronously in a single pass,
# so it must wait for each key. Set CERTUS_RL_CALLER_WAIT_MS to a short value to
# exercise the async-fill path (the caller returns fast, the background op fills
# the cache; a single-pass run then reports misses during a cold connect — that
# is expected, not a failure). `${VAR-}` keeps an explicitly-empty value empty.
CALLER_WAIT_MS="${CERTUS_RL_CALLER_WAIT_MS-}"
TEARDOWN_MS="${CERTUS_RL_TEARDOWN_MS:-2000}"
SERVER_ARGS="${CERTUS_TEST_SERVER_ARGS:---drive-count 1}"
# --format is mandatory (clean keyspace per run) and always appended to whatever
# device-selection args the caller chose. Append only if absent so a caller who
# includes it explicitly does not pass a duplicate flag.
case " $SERVER_ARGS " in
    *" --format "*) ;;
    *) SERVER_ARGS="$SERVER_ARGS --format" ;;
esac
KEYS="1-16"
OBJECT_SIZE="1M"
VERIFY=""
PYTHON="python3"
# shellcheck disable=SC2206
SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new ${CERTUS_TEST_SSH_OPTS:-})

# Print the leading comment block (everything from line 2 until the first
# non-comment line), stripped of the leading "# ".
usage() {
    awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "${BASH_SOURCE[0]}"
    exit "${1:-1}"
}

NODES=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --keys)        KEYS="$2"; shift 2 ;;
        --object-size) OBJECT_SIZE="$2"; shift 2 ;;
        --server-args) SERVER_ARGS="$2"; shift 2 ;;
        --verify)      VERIFY="--verify"; shift ;;
        --python)      PYTHON="$2"; shift 2 ;;
        -h|--help)     usage 0 ;;
        --)            shift; while [[ $# -gt 0 ]]; do NODES+=("$1"); shift; done ;;
        -*)            echo "unknown option: $1" >&2; usage ;;
        *)             NODES+=("$1"); shift ;;
    esac
done

# Node names: positional args, or CERTUS_TEST_NODES fallback. Never hard-coded.
if [[ ${#NODES[@]} -eq 0 && -n "${CERTUS_TEST_NODES:-}" ]]; then
    read -r -a NODES <<< "$CERTUS_TEST_NODES"
fi
if [[ ${#NODES[@]} -lt 2 ]]; then
    echo "error: need at least 2 node names (holder + requester)" >&2
    usage
fi

HOLDER="${NODES[0]}"
REQUESTER="${NODES[1]}"
RUN_TAG="$$"
DRIVER="$PYDIR/remote-lookup-clustertest.py"
STUBS=("$PYDIR/dispatcher_pb2.py" "$PYDIR/dispatcher_pb2_grpc.py")
REMOTE_DRIVER="/tmp/remote-lookup-clustertest-${RUN_TAG}.py"

for f in "$DRIVER" "${STUBS[@]}"; do
    [[ -f "$f" ]] || { echo "error: missing $f" >&2; exit 1; }
done

# Also remove this run's driver and the (untagged) generated stubs.
cleanup() {
    cluster_cleanup "'$REMOTE_DRIVER'" \
        "/tmp/dispatcher_pb2.py" "/tmp/dispatcher_pb2_grpc.py"
}
trap cleanup EXIT

dump_log() { cluster_dump_log "$1"; }

echo "=== Certus remote-lookup multi-node RDMA test ==="
log "Nodes:     ${NODES[*]}  (holder=$HOLDER, requester=$REQUESTER)"
log "Group:     $GROUP"
log "Binary:    $SERVER_BIN"
log "Keys:      $KEYS   object-size: $OBJECT_SIZE"
log "Server args: $SERVER_ARGS"

# --- 1. Launch a server on every node, all sharing $GROUP (beacon discovery) ---
# LD_LIBRARY_PATH is expanded by the REMOTE shell (\$ is escaped here) so we
# prepend our deps dirs without discarding whatever the node already sets.
REMOTE_ENV="RUST_LOG=${CERTUS_TEST_RUST_LOG:-info} LD_LIBRARY_PATH=\"$LIB_PATH:\${LD_LIBRARY_PATH:-}\" CERTUS_RL_OP_DEADLINE_MS=$OP_DEADLINE_MS CERTUS_RL_PHASE1_MS=$PHASE1_MS"
[[ -n "$CALLER_WAIT_MS" ]] && REMOTE_ENV="$REMOTE_ENV CERTUS_RL_CALLER_WAIT_MS=$CALLER_WAIT_MS"
[[ -n "$TEARDOWN_MS" ]] && REMOTE_ENV="$REMOTE_ENV CERTUS_RL_TEARDOWN_MS=$TEARDOWN_MS"
[[ -n "$RDMA_BIND_IP" ]] && REMOTE_ENV="$REMOTE_ENV CERTUS_RDMA_BIND_IP=$RDMA_BIND_IP"

cluster_launch

# --- 2. Wait for every gRPC endpoint, then let zyre beacon peers discover ---
cluster_wait_ready || exit 1

# --- 3. Ship the driver + stubs to holder and requester ---
for node in "$HOLDER" "$REQUESTER"; do
    scp -q "${SSH_OPTS[@]}" "$DRIVER" "$node:$REMOTE_DRIVER"
    scp -q "${SSH_OPTS[@]}" "${STUBS[@]}" "$node:/tmp/"
done

# --- 4. Populate on the holder ---
log "Populating keys $KEYS on holder $HOLDER ..."
if ! ssh "${SSH_OPTS[@]}" "$HOLDER" \
    "$PYTHON '$REMOTE_DRIVER' populate --server localhost:$GRPC_PORT \
         --keys '$KEYS' --object-size '$OBJECT_SIZE'"
then
    echo "error: populate failed on $HOLDER" >&2
    dump_log "$HOLDER"
    exit 1
fi

# --- 5. Look up from the requester and prove remoteness ---
log "Looking up keys $KEYS from requester $REQUESTER ..."
if ssh "${SSH_OPTS[@]}" "$REQUESTER" \
    "$PYTHON '$REMOTE_DRIVER' lookup --server localhost:$GRPC_PORT \
         --keys '$KEYS' --object-size '$OBJECT_SIZE' $VERIFY"
then
    echo ""
    echo "=== PASS: remote-lookup RDMA path confirmed across nodes ==="
    exit 0
else
    echo "" >&2
    echo "=== FAIL: remote lookup not confirmed (see verdict + logs above) ===" >&2
    dump_log "$HOLDER"
    dump_log "$REQUESTER"
    exit 1
fi
