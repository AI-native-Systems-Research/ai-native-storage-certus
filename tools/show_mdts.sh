#!/bin/bash
#
# show_mdts.sh - Show the Maximum Data Transfer Size (MDTS) of NVMe kernel devices.
#
# Reads from sysfs (no root required). Falls back to nvme-cli if sysfs is unavailable.
#
# Usage:
#   ./show_mdts.sh              # All NVMe devices
#   ./show_mdts.sh nvme0        # Specific controller
#   ./show_mdts.sh -q           # Quiet: bytes only (one per line)
#
set -euo pipefail

quiet=false
target=""

for arg in "$@"; do
    case "$arg" in
        -q) quiet=true ;;
        -h|--help)
            echo "Usage: $0 [-q] [nvmeN ...]"
            echo "  -q   Quiet mode: print MDTS in bytes only"
            exit 0
            ;;
        *) target="$arg" ;;
    esac
done

get_mdts() {
    local ctrl_name="$1"
    # Strip /dev/ prefix if provided
    ctrl_name="${ctrl_name#/dev/}"

    # Find the first namespace for this controller to read queue limits
    local ns=""
    for candidate in /sys/class/nvme/"$ctrl_name"/"${ctrl_name}"n*; do
        if [[ -d "$candidate" ]]; then
            ns=$(basename "$candidate")
            break
        fi
    done

    if [[ -z "$ns" ]]; then
        echo "  No namespace found for $ctrl_name" >&2
        return 1
    fi

    local max_hw_kb
    max_hw_kb=$(cat /sys/block/"$ns"/queue/max_hw_sectors_kb 2>/dev/null || true)

    if [[ -z "$max_hw_kb" ]]; then
        # Fallback: try nvme-cli (needs root)
        if command -v nvme &>/dev/null; then
            local mdts_raw
            mdts_raw=$(nvme id-ctrl "/dev/$ctrl_name" 2>/dev/null | grep -i "^mdts" | awk '{print $3}')
            if [[ -n "$mdts_raw" && "$mdts_raw" -eq 0 ]]; then
                max_hw_kb="unlimited"
            elif [[ -n "$mdts_raw" ]]; then
                local mpsmin
                mpsmin=$(nvme id-ctrl "/dev/$ctrl_name" 2>/dev/null | grep -i "^mpsmin" | awk '{print $3}')
                mpsmin=${mpsmin:-0}
                local page_size=$(( 1 << (12 + mpsmin) ))
                max_hw_kb=$(( (page_size * (1 << mdts_raw)) / 1024 ))
            fi
        fi
    fi

    if [[ -z "$max_hw_kb" ]]; then
        echo "  Could not determine MDTS for $ctrl_name" >&2
        return 1
    fi

    local mdts_bytes
    if [[ "$max_hw_kb" == "unlimited" ]]; then
        if $quiet; then
            echo "unlimited"
        else
            printf '  %-16s MDTS: unlimited (controller imposes no limit)\n' "$ctrl_name"
        fi
        return 0
    fi

    mdts_bytes=$(( max_hw_kb * 1024 ))

    if $quiet; then
        echo "$mdts_bytes"
    else
        local display
        if [[ $max_hw_kb -ge 1024 ]]; then
            display="$(( max_hw_kb / 1024 )) MiB"
        else
            display="${max_hw_kb} KiB"
        fi
        printf '  %-16s MDTS: %d bytes (%s)\n' "$ctrl_name" "$mdts_bytes" "$display"
    fi
}

if [[ -n "$target" ]]; then
    get_mdts "$target"
else
    if ! $quiet; then
        echo
        echo "NVMe Controller MDTS (Maximum Data Transfer Size)"
        echo "---------------------------------------------------"
    fi

    found=0
    for ctrl_path in /sys/class/nvme/nvme[0-9]*; do
        [[ -d "$ctrl_path" ]] || continue
        ctrl_name=$(basename "$ctrl_path")
        get_mdts "$ctrl_name"
        found=$((found + 1))
    done

    if [[ $found -eq 0 ]]; then
        echo "No NVMe controllers found." >&2
        exit 1
    fi

    if ! $quiet; then
        echo
    fi
fi
