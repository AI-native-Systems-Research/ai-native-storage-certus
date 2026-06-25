#!/bin/bash
#
# teardown_raid0.sh - Stop the certus md RAID0 array and release member devices.
#
# Usage:
#   sudo ./teardown_raid0.sh                   # Stop certus-raid0 (default)
#   sudo ./teardown_raid0.sh --name <name>     # Specify a different array name
#   sudo ./teardown_raid0.sh --device <dev>    # Specify md device directly (e.g. /dev/md127)
#   sudo ./teardown_raid0.sh --zero            # Also zero md superblocks on members
#   sudo ./teardown_raid0.sh --force           # Unmount automatically if mounted
#
set -euo pipefail

ARRAY_NAME="certus-raid0"
ARRAY_DEVICE=""
ZERO_SUPERBLOCKS=false
FORCE_UNMOUNT=false

die()  { echo "ERROR: $*" >&2; exit 1; }
info() { echo "  $*"; }

check_root() {
    [[ $EUID -eq 0 ]] || die "This script must be run as root (use sudo)."
}

check_deps() {
    command -v mdadm &>/dev/null || die "mdadm not found. Install with: dnf install mdadm"
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --name)
                shift; [[ $# -gt 0 ]] || die "--name requires an argument"
                ARRAY_NAME="$1"
                ;;
            --device)
                shift; [[ $# -gt 0 ]] || die "--device requires an argument"
                ARRAY_DEVICE="$1"
                ;;
            --zero|-z)
                ZERO_SUPERBLOCKS=true
                ;;
            --force|-f)
                FORCE_UNMOUNT=true
                ;;
            --help|-h)
                echo "Usage: sudo $0 [options]"
                echo
                echo "Options:"
                echo "  --name <name>     Array name to tear down (default: certus-raid0)"
                echo "  --device <dev>    Specify md device directly (e.g. /dev/md127)"
                echo "  --zero, -z        Zero md superblocks on member devices after stopping"
                echo "  --force, -f       Unmount the array automatically if it is mounted"
                echo "  --help            Show this help"
                exit 0
                ;;
            *)
                die "Unknown option: $1 (try --help)"
                ;;
        esac
        shift
    done
}

# Resolve the block device path for the named array or find any RAID0.
find_array_device() {
    # If --device was specified, use it directly
    if [[ -n "$ARRAY_DEVICE" ]]; then
        if [[ -b "$ARRAY_DEVICE" ]]; then
            echo "$ARRAY_DEVICE"
            return 0
        else
            die "--device '$ARRAY_DEVICE' is not a block device"
        fi
    fi

    # Try the named device
    local named="/dev/md/${ARRAY_NAME}"
    if [[ -b "$named" ]]; then
        echo "$named"
        return 0
    fi

    # Scan assembled arrays for one matching the name
    local md_dev
    md_dev=$(mdadm --detail --scan 2>/dev/null \
        | awk -v name=":${ARRAY_NAME}" '$0 ~ name {print $2}' \
        | head -1)
    if [[ -n "$md_dev" && -b "$md_dev" ]]; then
        echo "$md_dev"
        return 0
    fi

    # Fall back: find any active RAID0 array
    local raid0_devs=()
    while IFS= read -r line; do
        if [[ $line =~ ^(md[0-9]+)\ :\ active\ raid0 ]]; then
            raid0_devs+=("/dev/${BASH_REMATCH[1]}")
        fi
    done < /proc/mdstat

    if [[ ${#raid0_devs[@]} -eq 1 ]]; then
        echo "${raid0_devs[0]}"
        return 0
    elif [[ ${#raid0_devs[@]} -gt 1 ]]; then
        echo "  Multiple RAID0 arrays found. Use --device to specify which one:" >&2
        printf "    %s\n" "${raid0_devs[@]}" >&2
        return 1
    fi

    return 1
}

# Extract active member device paths from mdadm --detail output.
get_members() {
    local md_dev=$1
    mdadm --detail "$md_dev" 2>/dev/null \
        | awk 'NF >= 7 && /\/dev\// { print $NF }'
}

# Unmount the array if mounted; fail (or force-unmount) as configured.
handle_mounts() {
    local md_dev=$1
    local mounts
    mounts=$(findmnt -rno TARGET "$md_dev" 2>/dev/null || true)
    [[ -z "$mounts" ]] && return 0

    if ! $FORCE_UNMOUNT; then
        echo
        echo "  Array is currently mounted:"
        while IFS= read -r mp; do
            echo "    $mp"
        done <<< "$mounts"
        die "Unmount manually first, or re-run with --force."
    fi

    echo "  Unmounting ${md_dev}..."
    while IFS= read -r mp; do
        umount "$mp"
        info "Unmounted: $mp"
    done <<< "$mounts"
}

# --- Main ---

check_root
check_deps
parse_args "$@"

echo
echo "Looking for array: ${ARRAY_NAME}"

md_dev=$(find_array_device) \
    || die "Array '${ARRAY_NAME}' not found. Is it assembled? Check: cat /proc/mdstat"

echo "  Found: $md_dev"
echo
mdadm --detail "$md_dev"

# Collect members before stopping — detail is unavailable after stop.
mapfile -t members < <(get_members "$md_dev")

if [[ ${#members[@]} -eq 0 ]]; then
    echo
    echo "  WARNING: could not identify member devices from mdadm --detail output."
    echo "  Superblock zeroing (--zero) will be skipped."
fi

handle_mounts "$md_dev"

echo
echo "Stopping ${md_dev}..."
mdadm --stop "$md_dev"
info "Stopped."

if $ZERO_SUPERBLOCKS; then
    if [[ ${#members[@]} -eq 0 ]]; then
        echo
        echo "  WARNING: no member devices recorded; cannot zero superblocks."
    else
        echo
        echo "Zeroing md superblocks on member devices..."
        for dev in "${members[@]}"; do
            if [[ -b "$dev" ]]; then
                mdadm --zero-superblock "$dev"
                info "Zeroed: $dev"
            else
                echo "  WARNING: $dev not accessible, skipping."
            fi
        done
    fi
fi

echo
echo "Array '${ARRAY_NAME}' torn down."

if ! $ZERO_SUPERBLOCKS && [[ ${#members[@]} -gt 0 ]]; then
    echo
    info "Member devices retain md superblocks. Re-run with --zero to clear them"
    info "before using these drives with SPDK or another md array."
fi
