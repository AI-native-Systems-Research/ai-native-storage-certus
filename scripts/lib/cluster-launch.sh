# shellcheck shell=bash
#
# Shared multi-node cluster orchestration for the remote-lookup lab tests.
#
# Sourced by:
#   scripts/test-full-remote-multinode.sh          (correctness gate)
#   scripts/bench-remote-lookup-multinode.sh       (performance test)
#
# Both need the same five things: start a `certus-server-yaml` on every node in
# one shared zyre group, wait for each gRPC endpoint, let the UDP beacon settle,
# dump a node's log on failure, and tear the cluster down without touching
# another tester's servers. This file owns that and nothing else — key
# populating, measurement and verdicts belong to the callers.
#
# Contract for the caller, all set *before* sourcing or before the first call:
#   NODES            array of node names; NODES[0] is conventionally the holder
#   GROUP            shared zyre group (must be unique per tester)
#   SERVER_BIN       certus-server-yaml path, same on every node
#   GRPC_PORT        gRPC listen port on every node
#   SERVER_ARGS      device-selection args passed to every server
#   REMOTE_ENV       `VAR=val ...` string prefixed to the remote launch
#   SSH_OPTS         array of ssh options
#   RUN_TAG          unique per-run id, used in remote paths
#
# Provides: remote_log, log, cluster_launch, cluster_wait_ready,
#           cluster_dump_log, cluster_cleanup.

# Per-node server log path for this run.
remote_log() { echo "/tmp/certus-clustertest-${RUN_TAG}-$1.log"; }

log() { echo "[$(printf '%(%H:%M:%S)T' -1)] $*"; }

# Launch a server on every node, all joining $GROUP so the UDP beacon discovers
# them. LD_LIBRARY_PATH inside $REMOTE_ENV is expanded by the REMOTE shell, so
# the caller escapes `$` there.
cluster_launch() {
    local node
    for node in "${NODES[@]}"; do
        log "Launching server on $node ..."
        ssh "${SSH_OPTS[@]}" "$node" \
            "nohup env $REMOTE_ENV '$SERVER_BIN' --rl-group '$GROUP' \
                 --listen 0.0.0.0:$GRPC_PORT $SERVER_ARGS \
                 > '$(remote_log "$node")' 2>&1 </dev/null & echo launched" \
            >/dev/null
    done
}

# Block until every node's gRPC port accepts a connection, then allow the zyre
# beacon time to form the mesh. Returns non-zero (after dumping that node's log)
# if a server never came up.
cluster_wait_ready() {
    local node
    for node in "${NODES[@]}"; do
        log "Waiting for gRPC on $node:$GRPC_PORT ..."
        if ! ssh "${SSH_OPTS[@]}" "$node" \
            "for _ in \$(seq 1 120); do (exec 3<>/dev/tcp/127.0.0.1/$GRPC_PORT) 2>/dev/null && exit 0; sleep 0.5; done; exit 1"
        then
            echo "error: server on $node did not become ready" >&2
            cluster_dump_log "$node"
            return 1
        fi
    done
    log "All servers up; allowing ${CLUSTER_DISCOVERY_WAIT:-5}s for zyre peer discovery ..."
    sleep "${CLUSTER_DISCOVERY_WAIT:-5}"
}

cluster_dump_log() {
    local node="$1"
    echo "----- server log ($node) -----" >&2
    ssh "${SSH_OPTS[@]}" "$node" "cat '$(remote_log "$node")' 2>/dev/null" >&2 || true
    echo "-------------------------------" >&2
}

# Kill this run's servers and remove its remote scratch files. Scoped to OUR
# group via the `rl-group` match so a concurrent tester's servers survive.
# Extra paths to delete on every node may be passed as arguments.
cluster_cleanup() {
    local node extra=("$@")
    log "Tearing down cluster (group $GROUP)..."
    for node in "${NODES[@]}"; do
        ssh "${SSH_OPTS[@]}" "$node" \
            "pkill -f 'rl-group ${GROUP}' 2>/dev/null || true; \
             rm -f '$(remote_log "$node")' ${extra[*]} 2>/dev/null || true" \
            2>/dev/null || true
    done
}
