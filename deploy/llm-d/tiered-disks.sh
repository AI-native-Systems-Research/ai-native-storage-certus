#!/bin/bash
#
# tiered-disks.sh - hand a node's NVMe drives between certus and the generic
# tiered stack (profiles/<gpu>/agg-tiered), so both use the same drives.
#
#   sudo deploy/llm-d/tiered-disks.sh fs        drives -> kernel nvme, one RAID0
#                                               + XFS per NUMA node, mounted at
#                                               /mnt/kv-tier/numa<N>
#   sudo deploy/llm-d/tiered-disks.sh certus    unmount, stop the arrays, wipe
#                                               them, drives -> vfio-pci again
#   deploy/llm-d/tiered-disks.sh status         what each drive is doing now
#
# Stop certus on the node first (deploy/k8s/certus-nodes.sh release true
# <node>): it holds the drives through vfio while it runs. Each NUMA node gets
# the drives on that node, as each certus instance uses its own NUMA-local
# drives, with RAID0 at a 512 KiB chunk as tools/configure-bench.sh builds the
# Tiered-CPU-FS bench's array. Both directions DESTROY the drives' contents.
#
# The boot unit (deploy/k8s/host/certus-host-setup) skips drives in an MD array
# or under a mounted filesystem, so while this node is in `fs` mode a reboot
# leaves them alone; the arrays are not in /etc/fstab or mdadm.conf, so after a
# reboot run `fs` again (or `certus`, which then just rebinds).

set -euo pipefail

TAG="tiered-disks"
NVME_CLASS="0x010802"
MNT=/mnt/kv-tier
CHUNK=512K

log() { echo "${TAG}: $*"; }
die() { echo "${TAG}: $*" >&2; exit 1; }

driver_of() {
    local link="/sys/bus/pci/devices/$1/driver"
    if [[ -L "${link}" ]]; then basename "$(readlink "${link}")"; else echo "(none)"; fi
}

nvme_bdfs_on_numa() {    # BDFs of NVMe controllers on NUMA node $1, sorted
    local d
    for d in /sys/bus/pci/devices/*; do
        [[ "$(cat "${d}/class" 2>/dev/null)" = "${NVME_CLASS}"* ]] || continue
        [[ "$(cat "${d}/numa_node")" = "$1" ]] && basename "${d}"
    done | sort
}

numa_nodes() {
    local d
    for d in /sys/bus/pci/devices/*; do
        [[ "$(cat "${d}/class" 2>/dev/null)" = "${NVME_CLASS}"* ]] && cat "${d}/numa_node"
    done | sort -u
}

rebind() {               # rebind <bdf> <driver>
    local bdf="$1" drv="$2" dev="/sys/bus/pci/devices/$1" n=0
    [[ "$(driver_of "${bdf}")" = "${drv}" ]] && return 0
    [[ -L "${dev}/driver" ]] && echo "${bdf}" > "${dev}/driver/unbind"
    echo "${drv}" > "${dev}/driver_override"
    while ! echo "${bdf}" > /sys/bus/pci/drivers_probe 2>/dev/null && ((n++ < 10)); do sleep 0.5; done
    echo "" > "${dev}/driver_override"
    [[ "$(driver_of "${bdf}")" = "${drv}" ]] || die "${bdf}: failed to bind to ${drv} (on $(driver_of "${bdf}"))"
}

namespace_of() {         # the block device the kernel made for <bdf>, waiting for it
    local bdf="$1" i ns
    for i in $(seq 1 40); do
        for ns in /sys/bus/pci/devices/"${bdf}"/nvme/nvme*/nvme*n1; do
            [[ -e "${ns}" ]] || continue
            ns="$(basename "${ns}")"
            # Native multipath: nvme<S>c<C>n1 is a path; the device is nvme<S>n1.
            [[ ${ns} =~ ^nvme([0-9]+)c[0-9]+n([0-9]+)$ ]] && ns="nvme${BASH_REMATCH[1]}n${BASH_REMATCH[2]}"
            [[ -b "/dev/${ns}" ]] && { echo "/dev/${ns}"; return 0; }
        done
        sleep 0.5
    done
    die "${bdf}: no block device appeared on the nvme driver"
}

md_for_numa() { echo "/dev/md/kv-tier-numa$1"; }

to_fs() {
    command -v mdadm >/dev/null || die "mdadm is not installed"
    if pgrep -f certus-server-yaml >/dev/null; then
        die "certus-server is running here; release the node first (deploy/k8s/certus-nodes.sh release true $(hostname -s))"
    fi
    modprobe nvme
    local numa bdf devs md
    for numa in $(numa_nodes); do
        md="$(md_for_numa "${numa}")"
        if mountpoint -q "${MNT}/numa${numa}"; then
            log "NUMA ${numa}: already mounted at ${MNT}/numa${numa}"
            continue
        fi
        devs=()
        for bdf in $(nvme_bdfs_on_numa "${numa}"); do
            rebind "${bdf}" nvme
            devs+=("$(namespace_of "${bdf}")")
        done
        [[ ${#devs[@]} -gt 0 ]] || continue
        log "NUMA ${numa}: RAID0 over ${devs[*]} -> ${md}"
        for d in "${devs[@]}"; do
            wipefs -a "${d}" >/dev/null 2>&1 || true
            mdadm --zero-superblock "${d}" >/dev/null 2>&1 || true
        done
        if [[ ${#devs[@]} -eq 1 ]]; then
            md="${devs[0]}"
        else
            mdadm --create "${md}" --level=0 --raid-devices=${#devs[@]} --chunk=${CHUNK} \
                --metadata=1.2 --name="kv-tier-numa${numa}" --run "${devs[@]}" <<<"y"
        fi
        mkfs.xfs -f -q -L "kvtier${numa}" "${md}"
        mkdir -p "${MNT}/numa${numa}"
        mount -o noatime "${md}" "${MNT}/numa${numa}"
        log "NUMA ${numa}: $(df -h --output=source,size "${MNT}/numa${numa}" | tail -1)"
    done
}

to_certus() {
    local numa bdf md
    for numa in $(numa_nodes); do
        md="$(md_for_numa "${numa}")"
        if mountpoint -q "${MNT}/numa${numa}"; then
            umount "${MNT}/numa${numa}"
            log "NUMA ${numa}: unmounted ${MNT}/numa${numa}"
        fi
        if [[ -e "${md}" ]]; then
            local members
            members="$(mdadm --detail "${md}" 2>/dev/null | awk '/\/dev\/nvme/{print $NF}')"
            mdadm --stop "${md}"
            for d in ${members}; do
                mdadm --zero-superblock "${d}" >/dev/null 2>&1 || true
                wipefs -a "${d}" >/dev/null 2>&1 || true
            done
            log "NUMA ${numa}: stopped ${md}, cleared ${members//$'\n'/ }"
        fi
        for bdf in $(nvme_bdfs_on_numa "${numa}"); do
            # A single-drive tier has no array: clear the filesystem directly.
            if [[ "$(driver_of "${bdf}")" = nvme ]]; then
                d="$(namespace_of "${bdf}")"
                wipefs -a "${d}" >/dev/null 2>&1 || true
            fi
        done
    done
    # The boot unit's own rules decide what to bind; with the arrays gone it binds them all.
    if [[ -x /usr/local/sbin/certus-host-setup ]]; then
        /usr/local/sbin/certus-host-setup
    else
        modprobe vfio-pci
        for numa in $(numa_nodes); do
            for bdf in $(nvme_bdfs_on_numa "${numa}"); do rebind "${bdf}" vfio-pci; done
        done
        chmod a+rw /dev/vfio/* 2>/dev/null || true
    fi
}

status() {
    local numa bdf
    for numa in $(numa_nodes); do
        printf 'NUMA %s:' "${numa}"
        for bdf in $(nvme_bdfs_on_numa "${numa}"); do printf ' %s=%s' "${bdf}" "$(driver_of "${bdf}")"; done
        echo
        mountpoint -q "${MNT}/numa${numa}" && echo "  $(df -h --output=source,size,used,target "${MNT}/numa${numa}" | tail -1)"
    done
}

case "${1:-}" in
    fs)     [[ ${EUID} -eq 0 ]] || die "must run as root"; to_fs; status ;;
    certus) [[ ${EUID} -eq 0 ]] || die "must run as root"; to_certus; status ;;
    status) status ;;
    *) echo "usage: $0 fs|certus|status" >&2; exit 2 ;;
esac
