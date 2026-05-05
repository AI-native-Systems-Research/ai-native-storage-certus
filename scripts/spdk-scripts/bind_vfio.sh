#!/bin/bash
#
# bind_vfio.sh - Detach NVMe SSD devices from the kernel driver and bind to vfio-pci.
#
# Usage:
#   sudo ./bind_vfio.sh                  # Interactive: list NVMe devices, pick which to bind
#   sudo ./bind_vfio.sh status           # Show NVMe devices and their current driver
#   sudo ./bind_vfio.sh bind <BDF>...    # Bind specific devices to vfio-pci
#   sudo ./bind_vfio.sh reset <BDF>...   # Rebind specific devices back to nvme kernel driver
#   sudo ./bind_vfio.sh bind-all         # Bind ALL NVMe devices to vfio-pci
#   sudo ./bind_vfio.sh reset-all        # Rebind ALL vfio-bound NVMe devices to nvme
#
# Environment variables:
#   DRIVER_OVERRIDE   Override target driver (default: vfio-pci)
#
set -euo pipefail

DRIVER=${DRIVER_OVERRIDE:-vfio-pci}
NVME_CLASS="0x010802"  # NVMe controller PCI class code

die() { echo "ERROR: $*" >&2; exit 1; }

check_root() {
    [[ $EUID -eq 0 ]] || die "This script must be run as root (use sudo)."
}

# Ensure the vfio-pci module is loaded and IOMMU is available.
ensure_vfio() {
    if [[ "$DRIVER" == "vfio-pci" ]]; then
        if ! modprobe vfio-pci; then
            die "Failed to load vfio-pci module. Is IOMMU enabled in BIOS/kernel (intel_iommu=on or amd_iommu=on)?"
        fi
        if [[ ! -d /sys/kernel/iommu_groups/ ]] || [[ -z "$(ls -A /sys/kernel/iommu_groups/ 2>/dev/null)" ]]; then
            die "No IOMMU groups found. Enable IOMMU in BIOS and kernel (intel_iommu=on / amd_iommu=on)."
        fi
    else
        modprobe "$DRIVER" || die "Failed to load driver module: $DRIVER"
    fi
}

# Discover all NVMe PCI devices by class code.
# Populates parallel arrays: NVME_BDFS, NVME_VENDORS, NVME_DEVICES, NVME_DRIVERS, NVME_NUMAS, NVME_BLKDEVS
discover_nvme() {
    NVME_BDFS=()
    NVME_VENDORS=()
    NVME_DEVICES=()
    NVME_DRIVERS=()
    NVME_NUMAS=()
    NVME_BLKDEVS=()

    for dev_path in /sys/bus/pci/devices/*; do
        local bdf class vendor device driver numa blkdevs
        bdf=$(basename "$dev_path")
        class=$(cat "$dev_path/class" 2>/dev/null) || continue

        # Match NVMe controllers (class 0x010802)
        [[ "${class:0:8}" == "$NVME_CLASS" ]] || continue

        vendor=$(cat "$dev_path/vendor" 2>/dev/null)
        device=$(cat "$dev_path/device" 2>/dev/null)

        if [[ -L "$dev_path/driver" ]]; then
            driver=$(basename "$(readlink "$dev_path/driver")")
        else
            driver="(none)"
        fi

        numa=$(cat "$dev_path/numa_node" 2>/dev/null)
        [[ "$numa" == "-1" ]] && numa="n/a"

        # Find associated block devices
        blkdevs=""
        if [[ -d "$dev_path/nvme" ]]; then
            for ctrl in "$dev_path"/nvme/nvme*; do
                for ns in "$ctrl"/nvme*n*; do
                    [[ -d "$ns" ]] && blkdevs="${blkdevs:+$blkdevs,}$(basename "$ns")"
                done
            done
        fi
        [[ -z "$blkdevs" ]] && blkdevs="-"

        NVME_BDFS+=("$bdf")
        NVME_VENDORS+=("$vendor")
        NVME_DEVICES+=("$device")
        NVME_DRIVERS+=("$driver")
        NVME_NUMAS+=("$numa")
        NVME_BLKDEVS+=("$blkdevs")
    done
}

# Check if a block device is mounted or in use.
is_device_in_use() {
    local bdf=$1
    local dev_path="/sys/bus/pci/devices/$bdf"

    if [[ -d "$dev_path/nvme" ]]; then
        for ctrl in "$dev_path"/nvme/nvme*; do
            for ns in "$ctrl"/nvme*n*; do
                [[ -d "$ns" ]] || continue
                local blkname
                blkname=$(basename "$ns")
                if findmnt -rno TARGET "/dev/$blkname" &>/dev/null; then
                    echo "  WARNING: /dev/$blkname is mounted!" >&2
                    return 0
                fi
            done
        done
    fi
    return 1
}

# Print table of NVMe devices.
print_status() {
    discover_nvme
    if [[ ${#NVME_BDFS[@]} -eq 0 ]]; then
        echo "No NVMe devices found."
        return
    fi

    printf '\n%-5s %-14s %-8s %-8s %-6s %-16s %s\n' \
        "#" "BDF" "Vendor" "Device" "NUMA" "Driver" "Block Devices"
    printf '%-5s %-14s %-8s %-8s %-6s %-16s %s\n' \
        "---" "--------------" "--------" "--------" "------" "----------------" "-------------"

    for i in "${!NVME_BDFS[@]}"; do
        printf '%-5s %-14s %-8s %-8s %-6s %-16s %s\n' \
            "[$i]" "${NVME_BDFS[$i]}" "${NVME_VENDORS[$i]}" "${NVME_DEVICES[$i]}" \
            "${NVME_NUMAS[$i]}" "${NVME_DRIVERS[$i]}" "${NVME_BLKDEVS[$i]}"
    done
    echo
}

# Unbind a device from its current driver and bind to the target driver.
bind_device() {
    local bdf=$1
    local target_driver=$2
    local dev_path="/sys/bus/pci/devices/$bdf"

    [[ -d "$dev_path" ]] || die "PCI device $bdf not found."

    local current_driver="(none)"
    if [[ -L "$dev_path/driver" ]]; then
        current_driver=$(basename "$(readlink "$dev_path/driver")")
    fi

    if [[ "$current_driver" == "$target_driver" ]]; then
        echo "  $bdf: already bound to $target_driver"
        return 0
    fi

    # Unbind from current driver
    if [[ "$current_driver" != "(none)" ]]; then
        echo "  $bdf: unbinding from $current_driver"
        echo "$bdf" > "$dev_path/driver/unbind"
    fi

    # Use driver_override to bind to target
    echo "  $bdf: binding to $target_driver"
    echo "$target_driver" > "$dev_path/driver_override"

    local attempts=0
    while ! echo "$bdf" > /sys/bus/pci/drivers_probe 2>/dev/null && ((attempts++ < 10)); do
        sleep 0.5
    done

    # Clear the override
    echo "" > "$dev_path/driver_override"

    # Verify
    if [[ -L "$dev_path/driver" ]]; then
        local bound_driver
        bound_driver=$(basename "$(readlink "$dev_path/driver")")
        if [[ "$bound_driver" == "$target_driver" ]]; then
            echo "  $bdf: OK ($current_driver -> $target_driver)"
        else
            echo "  $bdf: FAILED (bound to $bound_driver instead of $target_driver)" >&2
            return 1
        fi
    else
        echo "  $bdf: FAILED (no driver bound)" >&2
        return 1
    fi
}

# Fix up /dev/vfio permissions for the device's IOMMU group.
fixup_vfio_permissions() {
    local bdf=$1
    local iommu_group_link="/sys/bus/pci/devices/$bdf/iommu_group"

    if [[ -L "$iommu_group_link" ]]; then
        local group
        group=$(basename "$(readlink "$iommu_group_link")")
        if [[ -e "/dev/vfio/$group" ]]; then
            chmod a+rw "/dev/vfio/$group"
            echo "  $bdf: set permissions on /dev/vfio/$group"
        fi
    fi
}

# Interactive mode: show devices and let user pick.
interactive() {
    discover_nvme
    if [[ ${#NVME_BDFS[@]} -eq 0 ]]; then
        echo "No NVMe devices found."
        exit 0
    fi

    print_status

    echo "Actions:"
    echo "  b <#> [#...]   Bind device(s) to $DRIVER (detach from kernel)"
    echo "  r <#> [#...]   Reset device(s) back to nvme kernel driver"
    echo "  B              Bind ALL devices to $DRIVER"
    echo "  R              Reset ALL devices to nvme"
    echo "  q              Quit"
    echo

    while true; do
        read -rp "> " action args
        case "$action" in
            b)
                [[ -n "$args" ]] || { echo "Specify device number(s), e.g.: b 0 1"; continue; }
                ensure_vfio
                for idx in $args; do
                    if [[ "$idx" =~ ^[0-9]+$ ]] && ((idx < ${#NVME_BDFS[@]})); then
                        local bdf="${NVME_BDFS[$idx]}"
                        if is_device_in_use "$bdf"; then
                            read -rp "  $bdf has mounted filesystems. Continue anyway? [y/N] " yn
                            [[ "$yn" =~ ^[yY] ]] || continue
                        fi
                        bind_device "$bdf" "$DRIVER"
                        fixup_vfio_permissions "$bdf"
                    else
                        echo "  Invalid index: $idx"
                    fi
                done
                # Refresh
                discover_nvme
                print_status
                ;;
            r)
                [[ -n "$args" ]] || { echo "Specify device number(s), e.g.: r 0 1"; continue; }
                for idx in $args; do
                    if [[ "$idx" =~ ^[0-9]+$ ]] && ((idx < ${#NVME_BDFS[@]})); then
                        bind_device "${NVME_BDFS[$idx]}" "nvme"
                    else
                        echo "  Invalid index: $idx"
                    fi
                done
                discover_nvme
                print_status
                ;;
            B)
                ensure_vfio
                for i in "${!NVME_BDFS[@]}"; do
                    local bdf="${NVME_BDFS[$i]}"
                    if is_device_in_use "$bdf"; then
                        echo "  Skipping $bdf (mounted filesystems)"
                        continue
                    fi
                    bind_device "$bdf" "$DRIVER"
                    fixup_vfio_permissions "$bdf"
                done
                discover_nvme
                print_status
                ;;
            R)
                for i in "${!NVME_BDFS[@]}"; do
                    if [[ "${NVME_DRIVERS[$i]}" == "$DRIVER" ]]; then
                        bind_device "${NVME_BDFS[$i]}" "nvme"
                    fi
                done
                discover_nvme
                print_status
                ;;
            q|Q|"")
                break
                ;;
            *)
                echo "Unknown action: $action"
                ;;
        esac
    done
}

# --- Main ---

check_root

case "${1:-}" in
    status)
        print_status
        ;;
    bind)
        shift
        [[ $# -gt 0 ]] || die "Usage: $0 bind <BDF> [BDF...]"
        ensure_vfio
        for bdf in "$@"; do
            if is_device_in_use "$bdf"; then
                echo "WARNING: $bdf has mounted filesystems - skipping (use interactive mode to override)"
                continue
            fi
            bind_device "$bdf" "$DRIVER"
            fixup_vfio_permissions "$bdf"
        done
        ;;
    reset)
        shift
        [[ $# -gt 0 ]] || die "Usage: $0 reset <BDF> [BDF...]"
        for bdf in "$@"; do
            bind_device "$bdf" "nvme"
        done
        ;;
    bind-all)
        ensure_vfio
        discover_nvme
        for i in "${!NVME_BDFS[@]}"; do
            if is_device_in_use "${NVME_BDFS[$i]}"; then
                echo "Skipping ${NVME_BDFS[$i]} (mounted filesystems)"
                continue
            fi
            bind_device "${NVME_BDFS[$i]}" "$DRIVER"
            fixup_vfio_permissions "${NVME_BDFS[$i]}"
        done
        ;;
    reset-all)
        discover_nvme
        for i in "${!NVME_BDFS[@]}"; do
            if [[ "${NVME_DRIVERS[$i]}" == "$DRIVER" || "${NVME_DRIVERS[$i]}" == "vfio-pci" ]]; then
                bind_device "${NVME_BDFS[$i]}" "nvme"
            fi
        done
        ;;
    help|-h|--help)
        echo "Usage: sudo $0 [status|bind|reset|bind-all|reset-all|help]"
        echo
        echo "Commands:"
        echo "  (none)          Interactive mode - list devices and choose"
        echo "  status          Show NVMe devices and their current driver"
        echo "  bind <BDF>...   Bind specific PCI devices to ${DRIVER}"
        echo "  reset <BDF>...  Rebind specific devices to the nvme kernel driver"
        echo "  bind-all        Bind all NVMe devices to ${DRIVER}"
        echo "  reset-all       Rebind all vfio-bound NVMe devices to nvme"
        echo "  help            Show this help"
        echo
        echo "Environment:"
        echo "  DRIVER_OVERRIDE  Override target driver (default: vfio-pci)"
        ;;
    "")
        interactive
        ;;
    *)
        die "Unknown command: $1 (try '$0 help')"
        ;;
esac
