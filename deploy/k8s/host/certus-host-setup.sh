#!/bin/bash
#
# certus-host-setup.sh - per-boot host setup for a Certus worker node.
#
# Binds every NVMe drive to vfio-pci, except a drive that holds anything the
# host itself uses, and opens /dev/vfio and /dev/hugepages for SPDK. Run once
# per boot by certus-host-setup.service, after local filesystems are mounted
# and before kubelet, so certus-server pods never start against drives the
# kernel still owns.
#
# A drive is left on the kernel nvme driver when any of its namespaces or
# partitions:
#   - is mounted, or is active swap (this covers the root filesystem);
#   - is named, directly or underneath, by an /etc/fstab entry;
#   - is held by another block device (an active LVM volume, an assembled MD
#     array, dm-crypt, multipath);
#   - carries an LVM physical-volume or MD RAID signature, so a volume group or
#     array that is not active yet is still caught.
#
# Usage:
#   certus-host-setup            # as root: bind and set permissions
#   certus-host-setup --dry-run  # report each drive's decision; change nothing
#
# A drive already on vfio-pci is left as it is; one on no driver, or on a
# driver other than nvme, is skipped, since its contents cannot be inspected.
# Exits 1 if any drive failed to bind, 0 otherwise.

set -uo pipefail

TAG="certus-host-setup"
NVME_CLASS="0x010802"
DRY_RUN=0

log() { echo "${TAG}: $*"; }

case "${1:-}" in
    --dry-run) DRY_RUN=1 ;;
    "") ;;
    *) echo "usage: $0 [--dry-run]" >&2; exit 2 ;;
esac

if [[ ${DRY_RUN} = 0 && ${EUID} -ne 0 ]]; then
    echo "${TAG}: must run as root (or use --dry-run)" >&2
    exit 2
fi

# Wait for udev to finish probing, so namespaces, partitions, holders and
# signatures are all visible.
udevadm settle --timeout=60 2>/dev/null || log "udevadm settle timed out; continuing"

# Kernel names of every block device that an fstab entry sits on, at any depth
# (an fstab LV -> its PV partition -> its disk).
declare -A FSTAB_DEVS=()
while read -r src; do
    real="$(readlink -f "${src}" 2>/dev/null)" || continue
    [[ -b "${real}" ]] || continue
    while read -r k; do
        FSTAB_DEVS["${k}"]="${src}"
    done < <(lsblk -sno KNAME "${real}" 2>/dev/null)
done < <(findmnt --fstab --evaluate -rno SOURCE 2>/dev/null)

# Block devices (namespaces, then their partitions) of the NVMe controller at
# PCI address $1. With native NVMe multipath a controller's path device is
# nvme<S>c<C>n<N>; the device the host uses is the head, nvme<S>n<N>.
blockdevs_of() {
    local ctrl ns name
    for ctrl in "/sys/bus/pci/devices/$1"/nvme/nvme*; do
        for ns in "${ctrl}"/nvme*n*; do
            [[ -e "${ns}" ]] || continue
            name="$(basename "${ns}")"
            if [[ ${name} =~ ^nvme([0-9]+)c[0-9]+n([0-9]+)$ ]]; then
                name="nvme${BASH_REMATCH[1]}n${BASH_REMATCH[2]}"
            fi
            [[ -e "/sys/class/block/${name}" ]] || continue
            echo "${name}"
            for part in "/sys/class/block/${name}/${name}"p*; do
                [[ -e "${part}" ]] && basename "${part}"
            done
        done
    done | sort -u
}

# Why block device $1 must stay with the host; prints nothing if it is free.
in_use_reason() {
    local dev="$1" holders h type
    holders=""
    for h in "/sys/class/block/${dev}/holders"/*; do
        [[ -e "${h}" ]] || continue
        h="$(basename "${h}")"
        if [[ -r "/sys/class/block/${h}/dm/name" ]]; then
            h="${h} ($(cat "/sys/class/block/${h}/dm/name"))"
        fi
        holders+="${holders:+, }${h}"
    done
    if [[ -n "${holders}" ]]; then
        echo "${dev} is held by ${holders}"
        return
    fi
    local mounts
    mounts="$(findmnt -rno TARGET -S "/dev/${dev}" 2>/dev/null)"
    if [[ -n "${mounts}" ]]; then
        # kubelet bind-mounts the root filesystem many times; name the first.
        local n
        n="$(wc -l <<< "${mounts}")"
        echo "${dev} is mounted at $(head -1 <<< "${mounts}")$( ((n > 1)) && echo " (+$((n - 1)) more)")"
        return
    fi
    if awk 'NR > 1 { print $1 }' /proc/swaps | xargs -r readlink -f | grep -qx "/dev/${dev}"; then
        echo "${dev} is active swap"
        return
    fi
    if [[ -n "${FSTAB_DEVS[${dev}]:-}" ]]; then
        echo "${dev} is under /etc/fstab entry ${FSTAB_DEVS[${dev}]}"
        return
    fi
    # A low-level probe reads the device itself (needs root); udev's record of
    # the same probe is the fallback for --dry-run as a user.
    type="$(blkid -p -o value -s TYPE "/dev/${dev}" 2>/dev/null)" ||
        type="$(lsblk -dno FSTYPE "/dev/${dev}" 2>/dev/null)"
    case "${type}" in
        LVM2_member) echo "${dev} is an LVM physical volume" ;;
        *_raid_member) echo "${dev} is an MD RAID member (${type})" ;;
    esac
}

driver_of() {
    local link="/sys/bus/pci/devices/$1/driver"
    if [[ -L "${link}" ]]; then basename "$(readlink "${link}")"; else echo "(none)"; fi
}

bind_vfio() {
    local bdf="$1" dev="/sys/bus/pci/devices/$1" attempts=0
    echo "${bdf}" > "${dev}/driver/unbind" || return 1
    echo vfio-pci > "${dev}/driver_override" || return 1
    while ! echo "${bdf}" > /sys/bus/pci/drivers_probe 2>/dev/null && ((attempts++ < 10)); do
        sleep 0.5
    done
    echo "" > "${dev}/driver_override"
    [[ "$(driver_of "${bdf}")" = vfio-pci ]]
}

if [[ ${DRY_RUN} = 0 ]] && ! modprobe vfio-pci; then
    log "cannot load vfio-pci (is the IOMMU enabled?); no drives bound"
    exit 1
fi

status=0
for d in /sys/bus/pci/devices/*; do
    [[ "$(cat "${d}/class" 2>/dev/null)" = "${NVME_CLASS}"* ]] || continue
    bdf="$(basename "${d}")"
    driver="$(driver_of "${bdf}")"
    case "${driver}" in
        vfio-pci) log "${bdf}: already on vfio-pci"; continue ;;
        nvme) ;;
        *) log "${bdf}: on ${driver}, not nvme; contents cannot be inspected, skipped"; continue ;;
    esac
    reasons=""
    while read -r blk; do
        r="$(in_use_reason "${blk}")"
        [[ -n "${r}" ]] && reasons+="${reasons:+; }${r}"
    done < <(blockdevs_of "${bdf}")
    if [[ -n "${reasons}" ]]; then
        log "${bdf}: kept on nvme: ${reasons}"
        continue
    fi
    if [[ ${DRY_RUN} = 1 ]]; then
        log "${bdf}: would bind to vfio-pci"
    elif bind_vfio "${bdf}"; then
        log "${bdf}: bound to vfio-pci"
    else
        log "${bdf}: FAILED to bind to vfio-pci (now on $(driver_of "${bdf}"))"
        status=1
    fi
done

# SPDK, run as a user outside Kubernetes, opens the vfio group devices and
# creates its hugepage files itself (scripts/spdk-scripts/cfg_user_spdk.sh).
if [[ ${DRY_RUN} = 0 ]]; then
    chmod a+rw /dev/vfio/* 2>/dev/null
    chmod a+rwx /dev/hugepages 2>/dev/null || log "/dev/hugepages is not mounted"
fi

exit "${status}"
