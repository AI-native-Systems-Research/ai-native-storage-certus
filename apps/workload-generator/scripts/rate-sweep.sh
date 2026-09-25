#!/usr/bin/env bash
# A rate sweep: offered load against schedule lateness (FR-080, T088e).
#
# WHAT THIS MEASURES, AND WHY IT IS NOT THE WORK-CONSERVING CEILING
#
# `--rate` is the load knob. Offered load scales with it while the workload's
# *shape* does not — the same keys in the same order with the same virtual
# interleaving, played faster — so sweeping it and watching where lateness leaves
# zero gives the rate at which this machine stops serving this workload on time.
# That is a load-versus-latency curve, which answers "can this machine serve this
# workload" far better than a single ceiling number does. The work-conserving mode
# is still the right instrument for a pure bandwidth ceiling with no schedule to
# keep; it just reports a saturated queue's latency, which no real workload forms.
#
# The reading to take is the LAST rate whose run is valid. Beyond it the generator
# stopped keeping its own schedule, so the latency percentiles from those runs
# describe a queue the workload would not have produced.
#
# WHY THIS IS A SCRIPT AND NOT A TEST
#
# It needs a live Certus, an accelerator and real wallclock time: a sweep at rate
# 1.0 costs its virtual span at every rung. The parts that can be checked without
# hardware are in `crates/workload-gen/tests/pacing.rs`, which is most of the
# arithmetic; this is the part that is a statement about the machine.
#
# MEASUREMENT HYGIENE, ALL FROM RECORDED INCIDENTS ON THIS HARDWARE
#
#   * One server only. Two `certus-server-yaml` processes on one host produce
#     numbers from whichever one the mailbox path happens to reach.
#   * Never quote a percentile without its n. The report carries the turn count
#     beside the lateness percentiles; this script prints both.
#   * Restart the server before quoting throughput.
#   * Rebuild the agent after any `src/` edit — the handshake refuses a stale
#     binary, but only if the binary is genuinely older, and a rebuilt generator
#     against an unrebuilt agent is the case that bites.
#   * n >= 8 for anything hit-dependent. Lateness is *not* hit-dependent — it is a
#     property of the schedule — so a single run per rung is meaningful here in a
#     way a hit rate never is. Pass REPEATS to raise it anyway.
#
# USAGE
#
#   scripts/rate-sweep.sh DESCRIPTION.yml [RATE...]
#
# Environment:
#   UNTIL     virtual seconds per run (default 60)
#   SEED      seed, held fixed across the sweep so every rung plays one workload
#   LANES     lanes per node (default 4)
#   INSTANCES space-separated instance list, each host[:port][:mailbox];
#             empty means the hardware file's list, or this host alone
#   HARDWARE  a hardware file to pass as --hardware; empty means ./cluster.yml
#             if it exists (the generator announces an implicit pickup)
#   OUT       directory for the JSON reports (default a fresh mktemp -d)
#   REPEATS   runs per rate (default 1)
#   GEN       path to the workload-gen binary
#
# Example:
#   UNTIL=120 scripts/rate-sweep.sh chat.yml 1 2 5 10 20 50
set -u -o pipefail

if [ "$#" -lt 1 ]; then
    sed -n '/^# USAGE/,/^set /p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi

DESCRIPTION="$1"
shift
RATES=("$@")
if [ "${#RATES[@]}" -eq 0 ]; then
    # A decade and a half, in steps that roughly double: enough to bracket the
    # knee without spending a run on every integer.
    #
    # `inf` is deliberately NOT in the default list. It is a different
    # measurement — the ceiling, judged on the plan queue rather than on
    # lateness — so its rows are not comparable with the paced ones and putting
    # it in the same table invites reading them as one curve. Pass it
    # explicitly to get a ceiling row, knowing that is what you asked for.
    RATES=(1 2 5 10 20 50 100)
fi

UNTIL="${UNTIL:-60}"
SEED="${SEED:-1}"
LANES="${LANES:-4}"
INSTANCES="${INSTANCES:-}"
HARDWARE="${HARDWARE:-}"
REPEATS="${REPEATS:-1}"
OUT="${OUT:-$(mktemp -d)}"
GEN="${GEN:-$(dirname "$0")/../../../target/release/workload-gen}"

if [ ! -x "$GEN" ]; then
    echo "no workload-gen at $GEN. Build it with:" >&2
    echo "  cargo build --release -p workload-gen -p workload-node-agent" >&2
    echo "or set GEN to its path." >&2
    exit 2
fi

# One server only, checked rather than assumed.
servers=$(ps -eo args | awk '$1 ~ /certus-server-yaml$/' | wc -l)
if [ "$servers" -gt 1 ]; then
    echo "refusing to sweep: $servers certus-server-yaml processes are running." >&2
    echo "Numbers from a host with two servers belong to whichever one the mailbox" >&2
    echo "path reached, which is not a fact about either." >&2
    exit 2
fi

# The deployment is held fixed while the rate varies, which is exactly the case
# the command line's per-field override of the hardware file exists for: --rate
# below wins over the file's rate without the file being edited.
target_args=()
for n in $INSTANCES; do
    target_args+=(--instance "$n")
done
if [ -n "$HARDWARE" ]; then
    target_args+=(--hardware "$HARDWARE")
fi

mkdir -p "$OUT"
echo "# rate sweep: $DESCRIPTION, until=$UNTIL seed=$SEED lanes=$LANES repeats=$REPEATS"
echo "# reports in $OUT"
printf '%-8s %-6s %-8s %-10s %-10s %-10s %-10s %s\n' \
    rate run valid turns late_p50 late_p99 late_max keys_per_s

for rate in "${RATES[@]}"; do
    for run in $(seq 1 "$REPEATS"); do
        report="$OUT/rate-$rate-run-$run.json"
        # Projected cost, printed by the generator itself before it starts: at rate
        # r a run costs UNTIL/r wallclock seconds, so a low rate on a long span is a
        # long wait rather than a hang.
        "$GEN" run "$DESCRIPTION" \
            --until "$UNTIL" \
            --seed "$SEED" \
            --lanes "$LANES" \
            --rate "$rate" \
            --report "$report" \
            "${target_args[@]}" >"$OUT/rate-$rate-run-$run.txt" 2>&1
        code=$?

        if [ ! -f "$report" ]; then
            printf '%-8s %-6s %-8s %s\n' "$rate" "$run" "ERROR" \
                "no report written; exit $code, see $OUT/rate-$rate-run-$run.txt"
            continue
        fi
        # Read from the structured report, never scraped from the terminal text —
        # that is what FR-065 requires the JSON for.
        python3 - "$report" "$rate" "$run" <<'PY'
import json, sys
report, rate, run = sys.argv[1], sys.argv[2], sys.argv[3]
with open(report) as f:
    r = json.load(f)
s = r.get("schedule") or {}
late = s.get("lateness_us") or {}
print("%-8s %-6s %-8s %-10s %-10s %-10s %-10s %.0f" % (
    rate, run,
    "yes" if r.get("valid") else "NO",
    s.get("turns", 0),
    late.get("p50", 0), late.get("p99", 0), late.get("max", 0),
    r.get("keys_per_second", 0.0),
))
if not r.get("valid"):
    why = r.get("invalid_reason") or "(no reason recorded)"
    print("         reason: %s" % why)
PY
    done
done

cat <<'EOF'

# How to read this
#
# The capacity figure is the LAST rate whose runs are all valid. At and below it
# this machine served this workload on time; above it the schedule was missed, so
# those runs' latency describes the generator's backlog rather than Certus.
#
# A rate whose lateness is non-zero but inside tolerance is the interesting
# neighbourhood — that is the knee, and it is where a policy or configuration
# change will show up first.
#
# If EVERY rate is invalid, check the projection line each run printed before it
# started: a rate far above what the machine can serve makes every turn due
# immediately, which is work-conserving wearing a rate's name.
EOF
