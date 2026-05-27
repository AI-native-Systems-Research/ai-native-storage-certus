#!/bin/bash
#
# show_spdk_devices.sh - List all PCI devices currently bound to the vfio-pci driver.
#
# Usage:
#   ./show_spdk_devices.sh          # Table of vfio-pci devices
#   ./show_spdk_devices.sh -q       # Quiet: BDFs only (one per line)
#
set -euo pipefail

quiet=false
[[ "${1:-}" == "-q" ]] && quiet=true

found=0

if ! $quiet; then
    printf '\n%-14s %-8s %-8s %-6s %-10s %s\n' \
        "BDF" "Vendor" "Device" "NUMA" "IOMMU Grp" "Device Name"
    printf '%-14s %-8s %-8s %-6s %-10s %s\n' \
        "--------------" "--------" "--------" "------" "----------" "-----------"
fi

for dev_path in /sys/bus/pci/devices/*; do
    # Skip devices without a driver symlink
    [[ -L "$dev_path/driver" ]] || continue

    driver=$(basename "$(readlink "$dev_path/driver")")
    [[ "$driver" == "vfio-pci" ]] || continue

    bdf=$(basename "$dev_path")
    found=$((found + 1))

    if $quiet; then
        echo "$bdf"
        continue
    fi

    vendor=$(cat "$dev_path/vendor" 2>/dev/null || echo "????")
    device=$(cat "$dev_path/device" 2>/dev/null || echo "????")

    numa=$(cat "$dev_path/numa_node" 2>/dev/null || echo "?")
    [[ "$numa" == "-1" ]] && numa="n/a"

    iommu_grp="-"
    if [[ -L "$dev_path/iommu_group" ]]; then
        iommu_grp=$(basename "$(readlink "$dev_path/iommu_group")")
    fi

    # Resolve human-readable name via lspci if available
    dev_name=""
    if command -v lspci &>/dev/null; then
        dev_name=$(lspci -s "$bdf" -mm 2>/dev/null | awk -F'"' '{print $6}' || true)
    fi
    [[ -z "$dev_name" ]] && dev_name="-"

    printf '%-14s %-8s %-8s %-6s %-10s %s\n' \
        "$bdf" "$vendor" "$device" "$numa" "$iommu_grp" "$dev_name"
done

if ! $quiet; then
    echo
    if [[ $found -eq 0 ]]; then
        echo "No devices bound to vfio-pci."
    else
        echo "$found device(s) bound to vfio-pci."
    fi
fi
