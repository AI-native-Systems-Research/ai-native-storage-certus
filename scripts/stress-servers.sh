#!/bin/sh
# Start or stop this host's stress-run Certus servers, one per NUMA domain.
#
#   stress-servers.sh stop    SIGTERM each, then wait for VFIO release
#   stress-servers.sh start   cold-format each and wait until it is serving
#
# One server per NUMA domain, pinned with numactl and given only that domain's
# NVMe drives, so a run measures a NUMA-local path rather than a mixture. Both
# join one remote-lookup group by default, which is what lets peers serve each
# other; SOLO puts each in its own group instead, which is the arm that measures
# the same workload with no peers reachable.
#
# Hostnames are never hardcoded: drives and NUMA domains are discovered from
# sysfs, and the repository root is derived from this script's own location.
#
# ENVIRONMENT
#   DOMAINS    NUMA domains to start (default: every domain with vfio-bound NVMe)
#   MT_SIZE    --memory-tier-size per instance (default 16G)
#   MT_EVICT   --memory-tier-eviction-threshold (default 0.0 = background evictor off)
#   SBP        --store-backpressure-ms (default 5000)
#   RLGROUP    remote-lookup group name (default stress_a)
#   SOLO       non-empty: each server gets its own group, so it has no peers
#   DEVICES_<n>  override the PCI list for domain <n>, space-separated BDFs
#   METRICS_BASE  first metrics port (default 9400; domain n uses BASE+n)
#   BIN        path to certus-server-yaml
#
# TRAPS THIS EXISTS TO AVOID, each from a recorded incident
#
#   * `pgrep -x certus-server-yaml` NEVER matches -- comm truncates at 15 chars
#     to `certus-server-y`. Match on that, or on the full path with ps.
#   * `pkill -f <pattern>` run over ssh self-kills the ssh, aborting the rest of
#     a caller's host loop. Kill by PID only.
#   * Restarting within seconds of a stop fails with `EAL: Cannot open
#     /dev/vfio/N: Device or resource busy`. Poll until the process is gone.
#   * `resource busy` on the SECOND instance is BENIGN: EAL scans every vfio-pci
#     device and fails on the other domain's groups before proceeding. Wait for
#     "shmq: serving" instead of treating stderr as failure.
#   * LD_LIBRARY_PATH is MANDATORY: libzyre.so.2 and libczmq.so.4 are not on the
#     loader path and not in /usr/lib.
#   * Do NOT end a branch with an informational `grep`. Its "nothing matched"
#     becomes the script's exit status, so a healthy start reports failure and
#     any caller running `set -e` dies. Every such grep here ends with `|| true`.
set -u

R="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$R/target/release/certus-server-yaml}"
export LD_LIBRARY_PATH="$R/deps/zyre-build/lib:$R/deps/zyre-build/lib64:$R/deps/spdk-build/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
METRICS_BASE="${METRICS_BASE:-9400}"

# --- discovery -------------------------------------------------------------
# NVMe drives this host has handed to userspace, grouped by the NUMA node the
# device itself reports. A kernel-bound drive is deliberately ignored: SPDK
# cannot claim it, so listing it would only produce an EAL failure at startup.
vfio_nvme_for_node() {
    node="$1"
    for d in /sys/bus/pci/drivers/vfio-pci/0000:*; do
        [ -e "$d" ] || continue
        bdf="$(basename "$d")"
        # 0x0108 = NVM Express controller
        cls="$(cat "$d/class" 2>/dev/null || echo)"
        case "$cls" in 0x010802|0x0108*) ;; *) continue ;; esac
        n="$(cat "$d/numa_node" 2>/dev/null || echo -1)"
        [ "$n" = "$node" ] && printf '%s ' "$bdf"
    done
}

nodes_with_drives() {
    for nd in /sys/devices/system/node/node*; do
        [ -d "$nd" ] || continue
        n="${nd##*/node}"
        [ -n "$(vfio_nvme_for_node "$n")" ] && printf '%s ' "$n"
    done
}

devices_for_node() {
    node="$1"
    eval "override=\${DEVICES_$node:-}"
    if [ -n "$override" ]; then
        echo "$override"
    else
        vfio_nvme_for_node "$node"
    fi
}

pids() { ps -eo pid,comm | awk '/certus-server-y/{print $1}'; }

DOMAINS="${DOMAINS:-$(nodes_with_drives)}"

case "${1:-}" in
stop)
    P=$(pids)
    [ -z "$P" ] && { echo "$(hostname -s): no servers running"; exit 0; }
    echo "$(hostname -s): stopping $P"
    for p in $P; do kill -TERM "$p" 2>/dev/null || true; done
    i=0
    while [ -n "$(pids)" ]; do
        i=$((i + 1))
        if [ "$i" -gt 120 ]; then
            echo "$(hostname -s): still alive after 60s: $(pids); sending KILL" >&2
            for p in $(pids); do kill -9 "$p" 2>/dev/null || true; done
        fi
        [ "$i" -gt 160 ] && { echo "$(hostname -s): will not die" >&2; exit 1; }
        sleep 0.5
    done
    echo "$(hostname -s): stopped, VFIO released"
    ;;
start)
    [ -x "$BIN" ] || { echo "no certus-server-yaml at $BIN. Build with: CERTUS_PROFILE=full-remote scripts/build-certus-full-remote-spdk.sh" >&2; exit 2; }
    [ -n "$(pids)" ] && { echo "$(hostname -s): servers already running: $(pids)" >&2; exit 1; }
    [ -n "$DOMAINS" ] || { echo "$(hostname -s): no NUMA domain has vfio-bound NVMe; bind drives or set DEVICES_<n>" >&2; exit 2; }

    want=0
    logs=""
    for n in $DOMAINS; do
        dev="$(devices_for_node "$n")"
        [ -n "$dev" ] || { echo "$(hostname -s): domain $n has no drives; set DEVICES_$n" >&2; exit 2; }
        devargs=""
        for b in $dev; do devargs="$devargs --device-pci $b"; done
        if [ -n "${SOLO:-}" ]; then
            group="solo-$(hostname -s)-$n"
        else
            group="${RLGROUP:-stress_a}"
        fi
        log="/tmp/certus-n$n.log"
        : > "$log"
        # shellcheck disable=SC2086
        setsid numactl --cpunodebind="$n" --membind="$n" "$BIN" \
            --rl-group "$group" \
            --shm-path "/dev/shm/certus-stress-n$n" \
            --channels 16 \
            $devargs \
            --format \
            --memory-tier-size "${MT_SIZE:-16G}" \
            --memory-tier-eviction-threshold "${MT_EVICT:-0.0}" \
            --store-backpressure-ms "${SBP:-5000}" \
            --metrics-port "$((METRICS_BASE + n))" \
            > "$log" 2>&1 &
        echo "$(hostname -s) numa$n: pid $! group $group drives $(echo "$dev" | wc -w) log $log"
        logs="$logs $log"
        want=$((want + 1))
    done

    # Wait for each STARTED domain to reach "shmq: serving" -- counted from the
    # domains actually started, not hardcoded, so a single-domain host does not
    # wait for a server that was never launched.
    i=0
    # shellcheck disable=SC2086
    while [ "$(grep -l 'shmq: serving' $logs 2>/dev/null | wc -l)" -lt "$want" ]; do
        i=$((i + 1))
        [ "$i" -gt 600 ] && { echo "$(hostname -s): servers did not come up in 300s; see$logs" >&2; exit 1; }
        sleep 0.5
    done
    echo "$(hostname -s): $want serving"
    # Informational only, and deliberately NON-FATAL -- see the grep trap above.
    # shellcheck disable=SC2086
    grep -hE "cold-load staging pool ready|cold pool started" $logs || true
    ;;
*)
    sed -n '/^# ENVIRONMENT/,/^#$/p' "$0" | sed 's/^# \{0,1\}//'
    echo "usage: $0 start|stop" >&2
    exit 2
    ;;
esac
