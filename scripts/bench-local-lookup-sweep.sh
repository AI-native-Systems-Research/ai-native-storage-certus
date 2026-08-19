#!/usr/bin/env bash
#
# bench-local-lookup-sweep.sh - Single-node lookup sweep, same tool and same
#                               config matrix as bench-remote-lookup-multinode.sh
#                               --sweep, minus the fabric leg.
#
# This exists to make the local number comparable to the remote one. Comparing
# the remote bench against apps/python/certus-api-bench_v2.py measures two
# different programs: different byte accounting, different timed window, and the
# python driver re-reads one small key set into one set of buffers while the Rust
# driver streams a large distinct keyspace. Running THE SAME BINARY with the same
# --object-size/--batch-size/--workers/--inflight on one node is the apples-to-
# apples local leg, and the difference that remains is the fabric plus whatever
# the remote path serializes.
#
# Local lookups need no --cleanup and must not get one: the keys are already
# local, so `lookup` mutates nothing and repeated invocations are free
# replicates. --cleanup would delete the dataset the populate just built.
#
# Usage:
#   ./bench-local-lookup-sweep.sh [options]
#
# Options:
#   --keys SPEC        keyspace, `N` or `LO-HI` (default 112000)
#   --object-size SZ   per-key size, e.g. 64K / 4M (default 64K)
#   --sweep SPECS      `;`-separated `batch:workers:inflight` configs; an empty
#                      field keeps the default. Default is the same matrix the
#                      multi-node script is usually run with:
#                        bytes-in-flight axis   64:4:4;64:4:8;64:4:16;64:4:32
#                        fixed-footprint axis   32:4:8;16:4:16;8:4:32
#                        channel axis           64:8:4;64:16:4
#                        batch axis             128:4:4;256:4:4
#   --replicates N     runs per config (default 4). Run-to-run sd is 5-16% here,
#                      so one sample per config cannot rank two configs.
#   --results-dir D    keep the JSON and TSV here instead of /tmp.
#   --mem SIZE         server --memory-tier-size (default 12G — must fit the
#                      smaller node's hugepages if you intend to compare).
#   --drive-count N    server --drive-count (default 1). Also selects which NVMe,
#                      and therefore which NUMA node the memory tier lands on.
#   --max-flight-mib M skip any config whose GPU landing buffer
#                      (workers*inflight*batch*object_size) exceeds M MiB
#                      (default 8192). Skips are printed, never silent.
#   --no-server        assume a server is already serving its shmq mailbox; do
#                      not launch or stop one. Populate still runs. You are then
#                      responsible for its --channels: it must be >= the largest
#                      workers*inflight in --sweep, or the bench errors out.
#   -h, --help         show this help.
#
# Requires: certus-server-yaml and remote-lookup-bench built at
# target/release/, a CUDA GPU, hugepages/vfio for SPDK.
#
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

SERVER_BIN="${CERTUS_SERVER_BIN:-$REPO_ROOT/target/release/certus-server-yaml}"
BENCH_BIN="${CERTUS_BENCH_BIN:-$REPO_ROOT/target/release/remote-lookup-bench}"
LIB_PATH="${CERTUS_LD_LIBRARY_PATH:-$REPO_ROOT/deps/zyre-build/lib:$REPO_ROOT/deps/zyre-build/lib64}"
SHM_PATH="${CERTUS_TEST_SHM_PATH:-/dev/shm/certus-shmq}"

KEYS_SPEC="112000"
OBJECT_SIZE="64K"
SWEEP="64:4:4;64:4:8;64:4:16;64:4:32;32:4:8;16:4:16;8:4:32;64:8:4;64:16:4;128:4:4;256:4:4"
REPLICATES=4
RESULTS_DIR=""
MEM_SIZE="12G"
DRIVE_COUNT=1
MAX_FLIGHT_MIB=8192
NO_SERVER=0

usage() {
    awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "${BASH_SOURCE[0]}"
    exit "${1:-1}"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --keys)            KEYS_SPEC="$2"; shift 2 ;;
        --object-size)     OBJECT_SIZE="$2"; shift 2 ;;
        --sweep)           SWEEP="$2"; shift 2 ;;
        --replicates)      REPLICATES="$2"; shift 2 ;;
        --results-dir)     RESULTS_DIR="$2"; shift 2 ;;
        --mem)             MEM_SIZE="$2"; shift 2 ;;
        --drive-count)     DRIVE_COUNT="$2"; shift 2 ;;
        --max-flight-mib)  MAX_FLIGHT_MIB="$2"; shift 2 ;;
        --no-server)       NO_SERVER=1; shift ;;
        -h|--help)         usage 0 ;;
        *)                 echo "unknown option: $1" >&2; usage ;;
    esac
done

if [[ "$KEYS_SPEC" == *-* ]]; then
    KEY_RANGE="$KEYS_SPEC"
else
    KEY_RANGE="1-$KEYS_SPEC"
fi
KEY_COUNT=$(( ${KEY_RANGE##*-} - ${KEY_RANGE%%-*} + 1 ))

[[ "$REPLICATES" =~ ^[0-9]+$ && "$REPLICATES" -ge 1 ]] || \
    { echo "error: --replicates must be a positive integer" >&2; exit 1; }
[[ -x "$SERVER_BIN" ]] || { echo "error: no server binary at $SERVER_BIN" >&2; exit 1; }
[[ -x "$BENCH_BIN" ]]  || { echo "error: no bench binary at $BENCH_BIN" >&2; exit 1; }

export LD_LIBRARY_PATH="$LIB_PATH:${LD_LIBRARY_PATH:-}"
export RUST_LOG="${CERTUS_TEST_RUST_LOG:-warn}"
LABEL="local:$(hostname -s)"

RESULT_DIR="$(mktemp -d "/tmp/certus-localsweep-$$.XXXXXX")"
SWEEP_TSV="$RESULT_DIR/sweep.tsv"
: > "$SWEEP_TSV"

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*"; }

SRV_PID=""
stop_server() {
    [[ -z "$SRV_PID" ]] && return 0
    log "Stopping server (pid $SRV_PID) ..."
    kill "$SRV_PID" 2>/dev/null
    # Until the process is reaped it still holds its VFIO groups and hugepages,
    # and the next launch either dies with "Device or resource busy" or silently
    # binds a different NVMe — which shows up as bimodal throughput, not as an
    # error. Waiting for actual exit is not optional.
    for _ in $(seq 1 120); do kill -0 "$SRV_PID" 2>/dev/null || break; sleep 0.5; done
    kill -9 "$SRV_PID" 2>/dev/null
    wait "$SRV_PID" 2>/dev/null
    SRV_PID=""
    sleep 3
}

cleanup() {
    stop_server
    if [[ -n "$RESULTS_DIR" ]]; then
        mkdir -p "$RESULTS_DIR"
        cp -a "$RESULT_DIR"/. "$RESULTS_DIR"/ 2>/dev/null || true
        log "Results copied to $RESULTS_DIR"
    fi
    rm -rf "$RESULT_DIR"
}
trap cleanup EXIT

start_server() {
    log "Launching server: --drive-count $DRIVE_COUNT --memory-tier-size $MEM_SIZE --channels $MAX_SLOTS"
    # Drop any stale mailbox from a prior crash so the readiness check below can
    # only pass on the file this server creates.
    rm -f "$SHM_PATH"
    "$SERVER_BIN" --drive-count "$DRIVE_COUNT" --format \
        --memory-tier-size "$MEM_SIZE" --shm-path "$SHM_PATH" --channels "$MAX_SLOTS" \
        > "$RESULT_DIR/server.log" 2>&1 &
    SRV_PID=$!
    # shmq has no TCP port; the mailbox file appears once Server::create has run.
    # The bench's attach() then spins for the ready magic, so a client that races
    # create by a few ms simply waits — no log-level dependence here.
    for _ in $(seq 1 240); do
        if [[ -e "$SHM_PATH" ]]; then
            log "Server ready (pid $SRV_PID)."
            return 0
        fi
        kill -0 "$SRV_PID" 2>/dev/null || {
            echo "error: server died during startup" >&2
            tail -30 "$RESULT_DIR/server.log" >&2
            return 1
        }
        sleep 0.5
    done
    echo "error: server not ready within 120s" >&2
    tail -30 "$RESULT_DIR/server.log" >&2
    return 1
}

objbytes() {
    python3 -c '
import sys
s = sys.argv[1]
m = {"K": 2**10, "M": 2**20, "G": 2**30}
print(int(s[:-1]) * m[s[-1].upper()] if s[-1].upper() in m else int(s))' "$1"
}
OBJ_BYTES="$(objbytes "$OBJECT_SIZE")"

# --- Parse the matrix, dropping configs that will not fit in GPU memory ------
# MAX_SLOTS = the largest workers*inflight kept, i.e. the most concurrent lanes
# any config drives. shmq lanes are OS threads each holding one mailbox channel,
# so the launched server must expose at least this many --channels or the bench
# rejects the run up front.
SPECS=(); SKIPPED=(); MAX_SLOTS=1
IFS=';' read -r -a raw_specs <<< "$SWEEP"
for s in "${raw_specs[@]}"; do
    [[ -n "$s" ]] || continue
    b="${s%%:*}"; rest="${s#*:}"; w="${rest%%:*}"; f="${rest##*:}"
    b="${b:-64}"; w="${w:-4}"; f="${f:-4}"
    flight=$(( w * f * b * OBJ_BYTES / 1048576 ))
    if [[ "$flight" -gt "$MAX_FLIGHT_MIB" ]]; then
        SKIPPED+=("batch=$b workers=$w inflight=$f needs ${flight} MiB > ${MAX_FLIGHT_MIB}")
        continue
    fi
    slots=$(( w * f ))
    (( slots > MAX_SLOTS )) && MAX_SLOTS=$slots
    SPECS+=("$b:$w:$f")
done
[[ ${#SPECS[@]} -gt 0 ]] || { echo "error: every config was skipped or --sweep was empty" >&2; exit 1; }

echo "=== Certus local lookup sweep ==="
log "Host:        $(hostname -s)   label: $LABEL"
log "Server:      $SERVER_BIN"
log "Bench:       $BENCH_BIN"
log "Keyspace:    $KEY_RANGE ($KEY_COUNT keys)   object-size: $OBJECT_SIZE"
log "Configs:     ${#SPECS[@]} x $REPLICATES replicate(s) = $(( ${#SPECS[@]} * REPLICATES )) rounds"
if [[ ${#SKIPPED[@]} -gt 0 ]]; then
    log "SKIPPED ${#SKIPPED[@]} config(s) over the --max-flight-mib=$MAX_FLIGHT_MIB GPU budget:"
    for s in "${SKIPPED[@]}"; do log "  $s"; done
fi

if [[ "$NO_SERVER" -eq 0 ]]; then
    start_server || exit 1
fi

# --- Populate once. Local lookups do not mutate, so the whole matrix reuses it.
log "Populating $KEY_COUNT keys at $OBJECT_SIZE ..."
if ! "$BENCH_BIN" populate --shm-path "$SHM_PATH" --keys "$KEY_RANGE" \
        --object-size "$OBJECT_SIZE" --batch-size 64 --workers 4 --inflight 4 \
        > "$RESULT_DIR/populate.json" 2> "$RESULT_DIR/populate.err"; then
    echo "error: populate failed" >&2
    cat "$RESULT_DIR/populate.err" >&2
    exit 1
fi
echo "  populate: $(cat "$RESULT_DIR/populate.json")"

# Let background write-through drain. Values stay DRAM-resident, so this does not
# turn the run into a disk-tier measurement; it just keeps writeback out of the
# timed window.
sleep 5

# --- Run the matrix ----------------------------------------------------------
failed=0
round=0
TOTAL=$(( ${#SPECS[@]} * REPLICATES ))
for ci in "${!SPECS[@]}"; do
    spec="${SPECS[ci]}"
    b="${spec%%:*}"; rest="${spec#*:}"; w="${rest%%:*}"; f="${rest##*:}"
    for ((r = 1; r <= REPLICATES; r++)); do
        round=$((round + 1))
        tag="c${ci}r${r}"
        printf '  [%d/%d] batch=%-4s workers=%-3s inflight=%-3s rep %d ... ' \
            "$round" "$TOTAL" "$b" "$w" "$f" "$r"
        # No --cleanup, and --iterations 1 --warmup-keys 0: both removal paths
        # would delete the dataset this sweep is reading.
        if "$BENCH_BIN" lookup --shm-path "$SHM_PATH" --keys "$KEY_RANGE" \
                --object-size "$OBJECT_SIZE" --batch-size "$b" \
                --workers "$w" --inflight "$f" \
                --iterations 1 --warmup-keys 0 \
                > "$RESULT_DIR/lookup-$tag.json" 2> "$RESULT_DIR/lookup-$tag.err"; then
            python3 "$SCRIPT_DIR/lib/sweep-row.py" \
                "$RESULT_DIR/lookup-$tag.json" \
                "$b" "$w" "$f" "$r" "$LABEL" "$OBJECT_SIZE" >> "$SWEEP_TSV"
            tail -n 1 "$SWEEP_TSV" | \
                awk -F'\t' '{printf "%8s GB/s  p50 %8s us\n", $7, $8}'
        else
            echo "FAILED"
            failed=1
            [[ -s "$RESULT_DIR/lookup-$tag.err" ]] && \
                sed 's/^/      /' "$RESULT_DIR/lookup-$tag.err" >&2
        fi
    done
done

echo ""
echo "=== Sweep summary ==="
python3 "$SCRIPT_DIR/lib/sweep-summary.py" "$SWEEP_TSV"
echo ""
log "Raw sweep rows: $SWEEP_TSV (use --results-dir to keep them)"
log "To put both legs in one table, pass this TSV and the remote one to"
log "  python3 $SCRIPT_DIR/lib/sweep-summary.py local.tsv remote.tsv"

if [[ $failed -ne 0 ]]; then
    echo "=== FAIL: at least one round errored (see above) ===" >&2
    exit 1
fi
echo "=== DONE: local sweep, $OBJECT_SIZE ==="
