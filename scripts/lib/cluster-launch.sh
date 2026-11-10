# shellcheck shell=bash
#
# Shared multi-node cluster orchestration for the remote-lookup lab tests.
#
# Sourced by:
#   scripts/test-full-remote-multinode.sh          (correctness gate)
#   scripts/bench-remote-lookup-multinode.sh       (performance test)
#
# Both need the same five things: start a `certus-server-yaml` on every node in
# one shared zyre group, wait for each node's shmq mailbox, let the UDP beacon
# settle, dump a node's log on failure, and tear the cluster down without
# touching another tester's servers. This file owns that and nothing else — key
# populating, measurement and verdicts belong to the callers.
#
# Contract for the caller, all set *before* sourcing or before the first call:
#   NODES            array of node names; NODES[0] is conventionally the holder
#   GROUP            shared zyre group (must be unique per tester)
#   SERVER_BIN       certus-server-yaml path, same on every node
#   SHM_PATH         /dev/shm mailbox path each server serves (same on every node)
#   CHANNELS         mailbox channels per server; must be >= the largest
#                    workers*inflight any client will drive against it
#   SERVER_ARGS      device-selection args passed to every server
#   REMOTE_ENV       `VAR=val ...` string prefixed to the remote launch
#   SSH_OPTS         array of ssh options
#   RUN_TAG          unique per-run id, used in remote paths
#
# Provides: remote_log, log, cluster_launch, cluster_wait_ready,
#           cluster_dump_log, cluster_cleanup.

# Per-node server log path for this run.
remote_log() { echo "/tmp/certus-clustertest-${RUN_TAG}-$1.log"; }

# node -> remote server PID, recorded at launch so teardown can wait for the
# exact process instead of pattern-matching (a `pkill -f` pattern also matches
# the remote shell running the cleanup).
declare -A CLUSTER_PIDS=()

log() { echo "[$(printf '%(%H:%M:%S)T' -1)] $*"; }

# Launch a server on every node, all joining $GROUP so the UDP beacon discovers
# them. LD_LIBRARY_PATH inside $REMOTE_ENV is expanded by the REMOTE shell, so
# the caller escapes `$` there.
cluster_launch() {
    local node pid
    for node in "${NODES[@]}"; do
        log "Launching server on $node ..."
        # rm the stale mailbox first so cluster_wait_ready's file-existence probe
        # can only pass on the file THIS server creates, not a prior crash's.
        pid=$(ssh "${SSH_OPTS[@]}" "$node" \
            "rm -f '$SHM_PATH'; \
             nohup env $REMOTE_ENV '$SERVER_BIN' --rl-group '$GROUP' \
                 --shm-path '$SHM_PATH' --channels $CHANNELS $SERVER_ARGS \
                 > '$(remote_log "$node")' 2>&1 </dev/null & echo \$!")
        CLUSTER_PIDS["$node"]="${pid//[^0-9]/}"
    done
}

# Block until every node's shmq mailbox file exists, then allow the zyre beacon
# time to form the mesh. shmq has no TCP port; the mailbox appears once
# Server::create has run, and a client's attach() then spins for the ready magic,
# so a client racing create by a few ms simply waits. Returns non-zero (after
# dumping that node's log) if a server never came up.
cluster_wait_ready() {
    local node
    for node in "${NODES[@]}"; do
        log "Waiting for shmq mailbox on $node:$SHM_PATH ..."
        if ! ssh "${SSH_OPTS[@]}" "$node" \
            "for _ in \$(seq 1 120); do [ -e '$SHM_PATH' ] && exit 0; sleep 0.5; done; exit 1"
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

# Kill this run's servers, WAIT FOR THEM TO ACTUALLY EXIT, and remove this run's
# remote scratch files. Scoped to OUR group so a concurrent tester survives.
# Extra paths to delete on every node may be passed as arguments.
#
# The wait is load-bearing, not politeness. Until the server is reaped it still
# holds its VFIO groups and its hugepages, so a following run either dies with
# `EAL: Cannot open /dev/vfio/N: Device or resource busy` or silently binds a
# *different* NVMe drive than the one asked for. That made consecutive benchmark
# runs incomparable: measured throughput split into two modes depending on
# whether the previous server had finished dying, with a spread bigger than the
# code changes being measured.
#
# Waits on the PID recorded at launch rather than a `pkill -f` pattern, because
# that pattern also matches the remote shell doing the cleanup. PID reuse is
# ruled out by re-checking the group string in /proc/<pid>/cmdline.
cluster_cleanup() {
    local node extra=("$@") pid
    log "Tearing down cluster (group $GROUP)..."
    for node in "${NODES[@]}"; do
        pid="${CLUSTER_PIDS[$node]:-}"
        ssh "${SSH_OPTS[@]}" "$node" \
            "bash -s -- '$GROUP' '$pid' '$(remote_log "$node")' \
                 '${CLUSTER_TEARDOWN_WAIT:-40}' ${extra[*]}" \
            2>/dev/null <<'REMOTE_CLEANUP' || true
group="$1"; pid="$2"; logfile="$3"; waitsecs="$4"; shift 4

# True only while $pid is a live process whose cmdline still names our group,
# so a recycled PID cannot be mistaken for the server.
alive() {
    [ -n "$pid" ] || return 1
    [ -r "/proc/$pid/cmdline" ] || return 1
    tr -d '\0' < "/proc/$pid/cmdline" 2>/dev/null | grep -q -- "$group"
}

if alive; then
    kill "$pid" 2>/dev/null || true
else
    # No usable PID (older caller, or already gone): fall back to the pattern.
    pkill -f "rl-group $group" 2>/dev/null || true
    pid=""
fi

for _ in $(seq 1 "$waitsecs"); do
    alive || break
    sleep 1
done
if alive; then
    echo "cleanup: $pid ignored SIGTERM after ${waitsecs}s, sending SIGKILL" >&2
    kill -9 "$pid" 2>/dev/null || true
    for _ in $(seq 1 10); do
        alive || break
        sleep 1
    done
fi
alive && echo "cleanup: WARNING $pid still alive; VFIO/hugepages may be held" >&2

rm -f "$logfile" "$@" 2>/dev/null || true
exit 0
REMOTE_CLEANUP
    done
    CLUSTER_PIDS=()
}
