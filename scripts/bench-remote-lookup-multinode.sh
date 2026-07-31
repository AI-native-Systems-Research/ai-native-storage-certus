#!/usr/bin/env bash
#
# Multi-node RDMA *performance* test for the Certus remote-lookup cluster path.
#
# The sibling script test-full-remote-multinode.sh answers "does it work" with a
# boolean over 16 keys. This one answers "how fast", over hundreds of thousands
# of keys, in either tiering regime, across five holder/requester topologies.
#
# Because remote-lookup is symmetric, a node can hold one shard of the keyspace
# while requesting others. That is what lets this exercise full-duplex links and
# the dual-role concurrency (serving a peer while fetching from another) that the
# one-directional correctness test never reaches.
#
# The load generator is Rust (apps/remote-lookup-bench), not Python. With RDMA
# and SPDK behind it, a Python driver measures the interpreter: the correctness
# driver builds object bytes one at a time in a generator, sends one key per RPC,
# and shares a single GPU buffer so nothing overlaps.
#
# Lab test: needs real RDMA NICs, NVMe drives, GPUs, and machines that only exist
# in your environment. Not a `cargo test` target, and it hard-codes NO hostnames.
#
# Usage:
#   ./bench-remote-lookup-multinode.sh [options] <node> <node> [node ...]
#   CERTUS_TEST_NODES="a b c" ./bench-remote-lookup-multinode.sh [options]
#
# Options:
#   --topology T      uni | bi | all-to-all | fan-in | fan-out (default uni)
#                       uni         node0 holds everything, node1 requests.
#                                   One flow. Establishes the per-flow ceiling —
#                                   run this FIRST; if it is far from line rate
#                                   the client is the limit and every other
#                                   number is uninterpretable.
#                       bi          node0/node1 hold half each and request the
#                                   other's half. Two opposed flows: compare the
#                                   aggregate against 2x uni and that ratio IS
#                                   the full-duplex answer.
#                       all-to-all  every node holds a shard and requests the
#                                   rest. Scale + quorum + symmetry.
#                       fan-in      all but the last node hold shards; the last
#                                   requests everything. Finds the point where
#                                   one requester saturates.
#                       fan-out     node0 holds everything; every other node
#                                   requests it. Finds the point where one
#                                   responder saturates.
#   --tier T          memory | disk (default memory)
#                       memory  working set stays in the memory tier. Keep it
#                               under --memory-tier-size or it spills and you are
#                               no longer measuring the DRAM path.
#                       disk    after populate, FlushToSsd + ClearMemoryTier puts
#                               every value on NVMe (still findable). The
#                               responder then promotes on demand, so this
#                               measures NVMe read + promotion + RDMA.
#                               IMPORTANT: serving a key promotes it into DRAM,
#                               so the working set must OVER-SUBSCRIBE the memory
#                               tier or the run warms into a memory-tier test.
#                               Aim for >= 3x --memory-tier-size.
#   --keys SPEC       total keyspace: `N` (means 1-N) or `LO-HI` (default 200000)
#   --object-size SZ  per-key size, e.g. 64K/1M (default 64K)
#   --batch-size N    keys per gRPC request (default 64)
#   --workers N       gRPC connections per node (default 4)
#   --inflight N      concurrent requests per connection (default 4)
#   --iterations N    passes over each requester's key list (default 1)
#   --warmup-keys N   keys fetched before timing, to pay cold RDMA connect costs
#                     outside the measurement (default 1024)
#   --verify          check the key stamp in every returned object. Costs a
#                     device-to-host copy per batch — qualify with it, measure
#                     without it.
#   --dry-run         print the holder/requester plan and the exact bench
#                     invocations, then exit. Touches no node. Use it to confirm
#                     the sharding is what you intended before spending a run.
#   --server-args "…" args for every server (default "--drive-count 1 --format").
#                     --format is appended if absent: without it, keys from a
#                     previous run are recovered from the drives at startup and
#                     populate fails with "key already exists".
#   -h, --help        Show this help.
#
# Environment (shared with test-full-remote-multinode.sh where it overlaps):
#   CERTUS_SERVER_BIN        certus-server-yaml path, same on every node
#   CERTUS_BENCH_BIN         remote-lookup-bench path, same on every node
#                            (build: cargo build -p remote-lookup-bench --release)
#   CERTUS_LD_LIBRARY_PATH   dirs holding libzyre/libczmq/libzmq
#   CERTUS_TEST_GROUP        shared zyre group (default clusterbench_<uid>_<pid>)
#   CERTUS_TEST_GRPC_PORT    gRPC port on every node (default 50051)
#   CERTUS_RDMA_BIND_IP      RoCE IPv4 the responder binds (default auto-detect)
#   CERTUS_RL_OP_DEADLINE_MS overall op deadline (default 5000)
#   CERTUS_RL_PHASE1_MS      Phase-1 memory-quorum timeout (default 500)
#   CERTUS_RL_CALLER_WAIT_MS caller block before NotFound (default: coupled to
#                            op_deadline, which is what a throughput run wants)
#   CERTUS_RL_TEARDOWN_MS    orphaned-landing-slot reclaim grace (default 2000)
#   CERTUS_TEST_RUST_LOG     RUST_LOG for every server (default warn — `info`
#                            per-op logging distorts a throughput measurement)
#   CERTUS_TEST_SSH_OPTS     extra ssh options
#
# Prerequisites (per node):
#   - Passwordless SSH from here to every named node.
#   - certus-server-yaml AND remote-lookup-bench built at the same paths on every
#     node (scripts/build-certus-full-remote-spdk.sh, then cargo build
#     -p remote-lookup-bench --release).
#   - Hugepages/vfio for SPDK, an up RoCE/IB device, a CUDA GPU + libcudart.
#   - All nodes on one L2 subnet (zyre UDP-beacon discovery).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Cluster launch/wait/teardown is shared with test-full-remote-multinode.sh.
# shellcheck source=lib/cluster-launch.sh
. "$SCRIPT_DIR/lib/cluster-launch.sh"

# --- defaults (env-overridable) ---
SERVER_BIN="${CERTUS_SERVER_BIN:-$REPO_ROOT/target/release/certus-server-yaml}"
BENCH_BIN="${CERTUS_BENCH_BIN:-$REPO_ROOT/target/release/remote-lookup-bench}"
LIB_PATH="${CERTUS_LD_LIBRARY_PATH:-$REPO_ROOT/deps/zyre-build/lib:$REPO_ROOT/deps/zyre-build/lib64}"
GROUP="${CERTUS_TEST_GROUP:-clusterbench_${UID:-$(id -u)}_$$}"
GRPC_PORT="${CERTUS_TEST_GRPC_PORT:-50051}"
RDMA_BIND_IP="${CERTUS_RDMA_BIND_IP:-}"
OP_DEADLINE_MS="${CERTUS_RL_OP_DEADLINE_MS:-5000}"
PHASE1_MS="${CERTUS_RL_PHASE1_MS:-500}"
CALLER_WAIT_MS="${CERTUS_RL_CALLER_WAIT_MS-}"
TEARDOWN_MS="${CERTUS_RL_TEARDOWN_MS:-2000}"
SERVER_ARGS="${CERTUS_TEST_SERVER_ARGS:---drive-count 1}"

TOPOLOGY="uni"
TIER="memory"
KEYS_SPEC="200000"
OBJECT_SIZE="64K"
BATCH_SIZE=64
WORKERS=4
INFLIGHT=4
ITERATIONS=1
WARMUP_KEYS=1024
VERIFY=""
DRY_RUN=""
# shellcheck disable=SC2206
SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new ${CERTUS_TEST_SSH_OPTS:-})

usage() {
    awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "${BASH_SOURCE[0]}"
    exit "${1:-1}"
}

NODES=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --topology)    TOPOLOGY="$2"; shift 2 ;;
        --tier)        TIER="$2"; shift 2 ;;
        --keys)        KEYS_SPEC="$2"; shift 2 ;;
        --object-size) OBJECT_SIZE="$2"; shift 2 ;;
        --batch-size)  BATCH_SIZE="$2"; shift 2 ;;
        --workers)     WORKERS="$2"; shift 2 ;;
        --inflight)    INFLIGHT="$2"; shift 2 ;;
        --iterations)  ITERATIONS="$2"; shift 2 ;;
        --warmup-keys) WARMUP_KEYS="$2"; shift 2 ;;
        --verify)      VERIFY="--verify"; shift ;;
        --dry-run)     DRY_RUN=1; shift ;;
        --server-args) SERVER_ARGS="$2"; shift 2 ;;
        -h|--help)     usage 0 ;;
        --)            shift; while [[ $# -gt 0 ]]; do NODES+=("$1"); shift; done ;;
        -*)            echo "unknown option: $1" >&2; usage ;;
        *)             NODES+=("$1"); shift ;;
    esac
done

case " $SERVER_ARGS " in
    *" --format "*) ;;
    *) SERVER_ARGS="$SERVER_ARGS --format" ;;
esac

if [[ ${#NODES[@]} -eq 0 && -n "${CERTUS_TEST_NODES:-}" ]]; then
    read -r -a NODES <<< "$CERTUS_TEST_NODES"
fi
if [[ ${#NODES[@]} -lt 2 ]]; then
    echo "error: need at least 2 node names" >&2
    usage
fi

case "$TIER" in memory|disk) ;; *) echo "error: --tier must be memory|disk" >&2; exit 1 ;; esac

# Normalise the keyspace to LO-HI.
if [[ "$KEYS_SPEC" == *-* ]]; then
    KEY_RANGE="$KEYS_SPEC"
else
    KEY_RANGE="1-$KEYS_SPEC"
fi
KEY_LO="${KEY_RANGE%%-*}"
KEY_HI="${KEY_RANGE##*-}"
KEY_COUNT=$(( KEY_HI - KEY_LO + 1 ))

N=${#NODES[@]}
RUN_TAG="$$"

# ---------------------------------------------------------------------------
# Topology → per-node roles
#
# Sharding needs no coordination: with H holders, holder h takes the keys where
# `key % H == h`. A requester asks for `key % H != h` when it is also holder h,
# so it never requests a key it already has — which keeps the correctness signal
# (`local_read_ops_delta == 0`) meaningful alongside the throughput numbers.
# ---------------------------------------------------------------------------
case "$TOPOLOGY" in
    uni|bi|all-to-all) ;;
    fan-in)
        [[ $N -ge 3 ]] || { echo "error: fan-in needs at least 3 nodes (2 holders + 1 requester)" >&2; exit 1; } ;;
    fan-out)
        [[ $N -ge 3 ]] || { echo "error: fan-out needs at least 3 nodes (1 holder + 2 requesters)" >&2; exit 1; } ;;
    *)
        echo "error: --topology must be uni|bi|all-to-all|fan-in|fan-out" >&2; exit 1 ;;
esac

HOLD_ARGS=()   # per node index: bench populate/demote shard args, or "" if not a holder
REQ_ARGS=()    # per node index: bench lookup shard args, or "" if not a requester

for ((i = 0; i < N; i++)); do HOLD_ARGS+=(""); REQ_ARGS+=(""); done

case "$TOPOLOGY" in
    uni)
        HOLDERS=1
        HOLD_ARGS[0]="--shard-mod 1 --shard-eq 0"
        REQ_ARGS[1]="--shard-mod 1"
        ;;
    bi)
        HOLDERS=2
        HOLD_ARGS[0]="--shard-mod 2 --shard-eq 0"
        HOLD_ARGS[1]="--shard-mod 2 --shard-eq 1"
        REQ_ARGS[0]="--shard-mod 2 --shard-ne 0"
        REQ_ARGS[1]="--shard-mod 2 --shard-ne 1"
        ;;
    all-to-all)
        HOLDERS=$N
        for ((i = 0; i < N; i++)); do
            HOLD_ARGS[i]="--shard-mod $N --shard-eq $i"
            REQ_ARGS[i]="--shard-mod $N --shard-ne $i"
        done
        ;;
    fan-in)
        HOLDERS=$(( N - 1 ))
        for ((i = 0; i < N - 1; i++)); do
            HOLD_ARGS[i]="--shard-mod $HOLDERS --shard-eq $i"
        done
        REQ_ARGS[N-1]="--shard-mod 1"
        ;;
    fan-out)
        HOLDERS=1
        HOLD_ARGS[0]="--shard-mod 1 --shard-eq 0"
        for ((i = 1; i < N; i++)); do REQ_ARGS[i]="--shard-mod 1"; done
        ;;
esac

RESULT_DIR="$(mktemp -d "/tmp/certus-clusterbench-${RUN_TAG}.XXXXXX")"
# Only tear down a cluster we actually started — a --dry-run or an early argument
# error must not ssh out and pkill on nodes it never touched.
CLUSTER_UP=""
cleanup() {
    [[ -n "$CLUSTER_UP" ]] && cluster_cleanup
    rm -rf "$RESULT_DIR"
}
trap cleanup EXIT

BENCH_COMMON="--server http://127.0.0.1:$GRPC_PORT --keys $KEY_RANGE \
--object-size $OBJECT_SIZE --batch-size $BATCH_SIZE --workers $WORKERS \
--inflight $INFLIGHT"

# ---------------------------------------------------------------------------
# NIC counters
#
# An aggregate GB/s cannot distinguish "duplex-limited" from "half-limited", and
# these per-direction counters are the only way to tell. They are conventionally
# in 4-byte words, so the absolute values are cross-checked against the bench's
# own byte accounting in the summary rather than trusted outright.
# ---------------------------------------------------------------------------
nic_counters() {
    local node="$1"
    ssh "${SSH_OPTS[@]}" "$node" '
        xmit=0; rcv=0
        for p in /sys/class/infiniband/*/ports/*; do
            [ -r "$p/counters/port_xmit_data" ] || continue
            state=$(cat "$p/state" 2>/dev/null || echo "")
            case "$state" in *ACTIVE*) ;; *) continue ;; esac
            x=$(cat "$p/counters/port_xmit_data" 2>/dev/null || echo 0)
            r=$(cat "$p/counters/port_rcv_data" 2>/dev/null || echo 0)
            xmit=$((xmit + x)); rcv=$((rcv + r))
        done
        echo "$xmit $rcv"' 2>/dev/null || echo "0 0"
}

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------
echo "=== Certus remote-lookup multi-node performance test ==="
log "Nodes:       ${NODES[*]}  ($N nodes, $HOLDERS holder shard(s))"
log "Topology:    $TOPOLOGY      Tier: $TIER"
log "Group:       $GROUP"
log "Server:      $SERVER_BIN"
log "Bench:       $BENCH_BIN"
log "Keyspace:    $KEY_RANGE ($KEY_COUNT keys)   object-size: $OBJECT_SIZE"
log "Load:        batch=$BATCH_SIZE workers=$WORKERS inflight=$INFLIGHT iterations=$ITERATIONS warmup=$WARMUP_KEYS"
log "Server args: $SERVER_ARGS"

if [[ -n "$DRY_RUN" ]]; then
    echo ""
    echo "=== Role plan (dry run — nothing was contacted) ==="
    printf '%-20s %-8s %s\n' node role "bench invocation"
    for ((i = 0; i < N; i++)); do
        if [[ -n "${HOLD_ARGS[i]}" ]]; then
            printf '%-20s %-8s %s\n' "${NODES[i]}" holder \
                "populate $BENCH_COMMON ${HOLD_ARGS[i]}"
            if [[ "$TIER" == "disk" ]]; then
                printf '%-20s %-8s %s\n' "" "" \
                    "demote --keys $KEY_RANGE ${HOLD_ARGS[i]}"
            fi
        fi
        if [[ -n "${REQ_ARGS[i]}" ]]; then
            printf '%-20s %-8s %s\n' "${NODES[i]}" requester \
                "lookup $BENCH_COMMON ${REQ_ARGS[i]} --iterations $ITERATIONS --warmup-keys $WARMUP_KEYS $VERIFY"
        fi
        if [[ -z "${HOLD_ARGS[i]}" && -z "${REQ_ARGS[i]}" ]]; then
            printf '%-20s %-8s %s\n' "${NODES[i]}" "mesh" \
                "(server only — joins the group, holds and requests nothing)"
        fi
    done
    echo ""
    echo "Keyspace $KEY_RANGE ($KEY_COUNT keys) over $HOLDERS holder shard(s);"
    echo "each holder takes key % $HOLDERS == its index, each requester asks for the rest."
    exit 0
fi

# The bench binary must exist on every node before we start servers, so a missing
# build fails in seconds rather than after a full cluster bring-up.
for node in "${NODES[@]}"; do
    if ! ssh "${SSH_OPTS[@]}" "$node" "test -x '$BENCH_BIN'"; then
        echo "error: $BENCH_BIN missing or not executable on $node" >&2
        echo "       build it there: cargo build -p remote-lookup-bench --release" >&2
        exit 1
    fi
done

# --- 1. Launch the cluster ---
REMOTE_ENV="RUST_LOG=${CERTUS_TEST_RUST_LOG:-warn} LD_LIBRARY_PATH=\"$LIB_PATH:\${LD_LIBRARY_PATH:-}\" CERTUS_RL_OP_DEADLINE_MS=$OP_DEADLINE_MS CERTUS_RL_PHASE1_MS=$PHASE1_MS"
[[ -n "$CALLER_WAIT_MS" ]] && REMOTE_ENV="$REMOTE_ENV CERTUS_RL_CALLER_WAIT_MS=$CALLER_WAIT_MS"
[[ -n "$TEARDOWN_MS" ]] && REMOTE_ENV="$REMOTE_ENV CERTUS_RL_TEARDOWN_MS=$TEARDOWN_MS"
[[ -n "$RDMA_BIND_IP" ]] && REMOTE_ENV="$REMOTE_ENV CERTUS_RDMA_BIND_IP=$RDMA_BIND_IP"

cluster_launch
CLUSTER_UP=1
cluster_wait_ready || exit 1

# Run `bench <subcommand> <args>` on $1, tee-ing its JSON to $RESULT_DIR/$2.
run_bench() {
    local node="$1" tag="$2"; shift 2
    ssh "${SSH_OPTS[@]}" "$node" \
        "LD_LIBRARY_PATH=\"$LIB_PATH:\${LD_LIBRARY_PATH:-}\" '$BENCH_BIN' $*" \
        > "$RESULT_DIR/$tag" 2> "$RESULT_DIR/$tag.err"
}

# --- 2. Populate every holder, concurrently ---
log "Populating $KEY_COUNT keys across $HOLDERS holder shard(s) ..."
pids=(); failed=0
for ((i = 0; i < N; i++)); do
    [[ -n "${HOLD_ARGS[i]}" ]] || continue
    node="${NODES[i]}"
    log "  populate on $node  (${HOLD_ARGS[i]})"
    run_bench "$node" "populate-$i.json" \
        populate $BENCH_COMMON ${HOLD_ARGS[i]} &
    pids+=("$!")
done
for p in "${pids[@]}"; do wait "$p" || failed=1; done
if [[ $failed -ne 0 ]]; then
    echo "error: populate failed" >&2
    for ((i = 0; i < N; i++)); do
        [[ -n "${HOLD_ARGS[i]}" ]] || continue
        [[ -s "$RESULT_DIR/populate-$i.json.err" ]] && \
            { echo "--- ${NODES[i]} ---" >&2; cat "$RESULT_DIR/populate-$i.json.err" >&2; }
        cluster_dump_log "${NODES[i]}"
    done
    exit 1
fi
for ((i = 0; i < N; i++)); do
    [[ -n "${HOLD_ARGS[i]}" ]] || continue
    echo "  ${NODES[i]} populate: $(cat "$RESULT_DIR/populate-$i.json")"
done

# --- 3. Disk tier: flush write-through, then drain DRAM ---
if [[ "$TIER" == "disk" ]]; then
    log "Demoting holders to the disk tier (FlushToSsd -> ClearMemoryTier -> Check) ..."
    pids=(); failed=0
    for ((i = 0; i < N; i++)); do
        [[ -n "${HOLD_ARGS[i]}" ]] || continue
        run_bench "${NODES[i]}" "demote-$i.json" \
            demote --server "http://127.0.0.1:$GRPC_PORT" \
            --keys "$KEY_RANGE" ${HOLD_ARGS[i]} &
        pids+=("$!")
    done
    for p in "${pids[@]}"; do wait "$p" || failed=1; done
    for ((i = 0; i < N; i++)); do
        [[ -n "${HOLD_ARGS[i]}" ]] || continue
        echo "  ${NODES[i]} demote: $(cat "$RESULT_DIR/demote-$i.json" 2>/dev/null)"
        [[ -s "$RESULT_DIR/demote-$i.json.err" ]] && cat "$RESULT_DIR/demote-$i.json.err" >&2
    done
    if [[ $failed -ne 0 ]]; then
        echo "error: demote failed — entries may have been force-removed rather than" >&2
        echo "       demoted (a key with no SSD copy yet is dropped from both tiers)." >&2
        exit 1
    fi
fi

# --- 4. Sample NIC counters, run the lookup phase, sample again ---
declare -a NIC_BEFORE NIC_AFTER
for ((i = 0; i < N; i++)); do NIC_BEFORE[i]="$(nic_counters "${NODES[i]}")"; done

log "Looking up from $(for ((i=0;i<N;i++)); do [[ -n "${REQ_ARGS[i]}" ]] && printf '%s ' "${NODES[i]}"; done)..."
phase_start=$(date +%s.%N)
pids=(); failed=0
for ((i = 0; i < N; i++)); do
    [[ -n "${REQ_ARGS[i]}" ]] || continue
    run_bench "${NODES[i]}" "lookup-$i.json" \
        lookup $BENCH_COMMON ${REQ_ARGS[i]} \
        --iterations "$ITERATIONS" --warmup-keys "$WARMUP_KEYS" $VERIFY &
    pids+=("$!")
done
for p in "${pids[@]}"; do wait "$p" || failed=1; done
phase_end=$(date +%s.%N)

for ((i = 0; i < N; i++)); do NIC_AFTER[i]="$(nic_counters "${NODES[i]}")"; done

# --- 5. Report ---
echo ""
echo "=== Per-node results ==="
for ((i = 0; i < N; i++)); do
    [[ -n "${REQ_ARGS[i]}" ]] || continue
    echo "${NODES[i]}: $(cat "$RESULT_DIR/lookup-$i.json" 2>/dev/null)"
    [[ -s "$RESULT_DIR/lookup-$i.json.err" ]] && cat "$RESULT_DIR/lookup-$i.json.err" >&2
done

echo ""
echo "=== NIC per-direction deltas (units are 4-byte words on mlx5; see header) ==="
printf '%-20s %18s %18s\n' node xmit_words rcv_words
for ((i = 0; i < N; i++)); do
    read -r xb rb <<< "${NIC_BEFORE[i]}"
    read -r xa ra <<< "${NIC_AFTER[i]}"
    printf '%-20s %18s %18s\n' "${NODES[i]}" "$((xa - xb))" "$((ra - rb))"
done

# Aggregate across requesters. Reported as a bracket rather than one number: the
# phase span includes serial ssh spawn (understates), while the longest per-node
# elapsed assumes the requesters fully overlapped (overstates). The truth is
# between them, and they converge as the run gets longer — which is the argument
# for running long enough that they do.
echo ""
echo "=== Aggregate ==="
python3 - "$RESULT_DIR" "$phase_start" "$phase_end" <<'PY'
import glob, json, os, sys

result_dir, start, end = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
runs = []
for path in sorted(glob.glob(os.path.join(result_dir, "lookup-*.json"))):
    if path.endswith(".err"):
        continue
    try:
        with open(path) as f:
            runs.append(json.load(f))
    except (OSError, ValueError):
        pass

if not runs:
    print("no requester results parsed")
    sys.exit(0)

phase = max(end - start, 1e-9)
slowest = max(max(r["elapsed_s"] for r in runs), 1e-9)
ok = sum(r["keys_ok"] for r in runs)
bad = sum(r["keys_failed"] for r in runs)
by = sum(r["bytes"] for r in runs)
vf = sum(r.get("verify_failures", 0) for r in runs)
local_ops = sum(r.get("local_read_ops_delta", 0) for r in runs)


def human(n):
    for unit, div in (("GiB", 2**30), ("MiB", 2**20), ("KiB", 2**10)):
        if n >= div:
            return f"{n / div:.2f} {unit}"
    return f"{n} B"


print(f"  requesters:      {len(runs)}")
print(f"  keys ok/failed:  {ok} / {bad}")
print(f"  bytes:           {by} ({human(by)})")
# phase >= slowest in any real run (it brackets every requester), so bytes/phase
# is the lower bound. min/max rather than assuming the order, so a degenerate
# timing never prints a backwards range.
gbps_lo, gbps_hi = sorted((by / phase / 1e9, by / slowest / 1e9))
kps_lo, kps_hi = sorted((ok / phase, ok / slowest))
print(f"  aggregate:       {gbps_lo:.3f} .. {gbps_hi:.3f} GB/s")
print(f"                   ({kps_lo:.0f} .. {kps_hi:.0f} keys/s)")
print(f"                   lower = phase span ({phase:.3f} s, includes serial ssh")
print(f"                   spawn); upper = slowest requester ({slowest:.3f} s, assumes")
print("                   full overlap). Run longer to close the gap.")
print("  per-requester:   " + ", ".join(f"{r['gbps']:.3f}" for r in runs) + " GB/s")
print("  p50 / p99 (us):  " + ", ".join(
    f"{r['rpc_latency_us']['p50']:.0f}/{r['rpc_latency_us']['p99']:.0f}" for r in runs))

# A remote fetch lands in DRAM by RDMA, so a requester reading its own SSD means
# the value did not come over the fabric.
if local_ops:
    print(f"  WARNING: requesters did {local_ops} local SSD read op(s) — some hits")
    print(f"           may have been served locally rather than over RDMA")
if vf:
    print(f"  WARNING: {vf} verify failure(s) — wrong or corrupt data returned")
if bad:
    print(f"  NOTE: {bad} key(s) failed; see per-node first_error above")
PY

echo ""
if [[ $failed -ne 0 ]]; then
    echo "=== FAIL: at least one requester reported an error (see above) ===" >&2
    for ((i = 0; i < N; i++)); do
        [[ -n "${REQ_ARGS[i]}" ]] && cluster_dump_log "${NODES[i]}"
    done
    exit 1
fi
echo "=== DONE: $TOPOLOGY / $TIER ==="
