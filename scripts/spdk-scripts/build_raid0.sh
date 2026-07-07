#!/bin/bash
#
# build_raid0.sh - Create a Linux md RAID0 array from kernel-bound NVMe block devices.
#
# Discovers NVMe controllers currently bound to the 'nvme' kernel driver (i.e. NOT
# bound to vfio-pci for SPDK use) and assembles their namespaces into an md RAID0
# array using mdadm.
#
# Usage:
#   sudo ./build_raid0.sh                              # Interactive
#   sudo ./build_raid0.sh --yes                        # Non-interactive, all discovered devices
#   sudo ./build_raid0.sh --name <name>                # Array name (default: certus-raid0)
#   sudo ./build_raid0.sh --chunk <KiB>                # Chunk size KiB (default: 512)
#   sudo ./build_raid0.sh --count <n>                  # Number of devices to use (default: all)
#   sudo ./build_raid0.sh --wipe                       # Wipe GPT partition tables first
#   sudo ./build_raid0.sh --devices /dev/nvme0n1 ...   # Explicit device list
#
# Environment:
#   MDADM_EXTRA_OPTS   Extra flags appended to the mdadm --create command
#
set -euo pipefail

ARRAY_NAME="certus-raid0"
CHUNK_KIB=512
YES=false
WIPE=false
EXPLICIT_DEVS=()
DEVICE_COUNT=0

die()  { echo "ERROR: $*" >&2; exit 1; }
info() { echo "  $*"; }

check_root() {
    [[ $EUID -eq 0 ]] || die "This script must be run as root (use sudo)."
}

check_deps() {
    command -v mdadm  &>/dev/null || die "mdadm not found. Install with: dnf install mdadm"
    command -v lsblk  &>/dev/null || die "lsblk not found (util-linux package)."
    if $WIPE; then
        command -v wipefs &>/dev/null || die "wipefs not found (util-linux package)."
    fi
}

# Discover all NVMe block device nodes whose PCI controller is bound to the 'nvme' driver.
# Discover NVMe namespace block devices currently visible to the kernel.
# A device bound to vfio-pci has no /dev/nvmeXnY entry, so iterating /dev
# directly is both simpler and correct — no sysfs traversal needed.
discover_kernel_nvme_devs() {
    KERNEL_DEVS=()
    for dev in /dev/nvme*n*; do
        [[ -b "$dev" ]] || continue
        KERNEL_DEVS+=("$dev")
    done
}

print_devices() {
    local devs=("$@")
    if [[ ${#devs[@]} -eq 0 ]]; then
        echo "  (none)"
        return
    fi
    printf '  %-5s %-16s %s\n' "#" "Device" "Size"
    printf '  %-5s %-16s %s\n' "---" "----------------" "--------"
    local i
    for i in "${!devs[@]}"; do
        local size
        size=$(lsblk -dno SIZE "${devs[$i]}" 2>/dev/null || echo "?")
        printf '  %-5s %-16s %s\n' "[$i]" "${devs[$i]}" "$size"
    done
}

check_superblocks() {
    local devs=("$@")
    local dirty=()
    for dev in "${devs[@]}"; do
        if mdadm --examine "$dev" &>/dev/null 2>&1; then
            dirty+=("$dev")
        fi
    done

    if [[ ${#dirty[@]} -eq 0 ]]; then
        return 0
    fi

    echo
    echo "  WARNING: The following device(s) carry existing md superblocks:"
    for dev in "${dirty[@]}"; do
        echo "    $dev"
    done
    echo "  These will be overwritten. All data will be lost."

    if $YES; then
        echo "  --yes specified: proceeding."
        return 0
    fi

    echo
    read -rp "  Overwrite superblocks and continue? [y/N] " confirm
    [[ "$confirm" =~ ^[yY] ]] || { echo "Aborted."; exit 1; }
}

wipe_partition_tables() {
    local devs=("$@")
    echo
    echo "  Wiping partition tables..."
    for dev in "${devs[@]}"; do
        # Wipe all signatures repeatedly until none remain (layered signatures
        # such as partition + filesystem require multiple passes).
        while wipefs --all --force "$dev" 2>/dev/null | grep -q .; do
            :
        done
        # Zero the first and last 1 MiB to catch any remaining GPT headers
        dd if=/dev/zero of="$dev" bs=1M count=1 conv=notrunc 2>/dev/null
        dd if=/dev/zero of="$dev" bs=1M seek=$(( $(blockdev --getsize64 "$dev") / 1048576 - 1 )) count=1 conv=notrunc 2>/dev/null
        info "Wiped: $dev"
    done
}

create_array() {
    local devs=("$@")
    local n=${#devs[@]}
    [[ $n -ge 2 ]] || die "RAID0 requires at least 2 devices; got $n."

    local md_dev="/dev/md/${ARRAY_NAME}"

    if [[ -b "$md_dev" ]]; then
        die "Array device $md_dev already exists. Run teardown_raid0.sh first."
    fi

    check_superblocks "${devs[@]}"

    if $WIPE; then
        wipe_partition_tables "${devs[@]}"
    fi

    echo
    echo "  Array device : $md_dev"
    echo "  Array name   : $ARRAY_NAME"
    echo "  Members      : ${devs[*]}"
    echo "  Chunk size   : ${CHUNK_KIB} KiB"
    echo "  Layout       : RAID0 — striped, NO redundancy"
    echo

    # shellcheck disable=SC2086
    mdadm --create "$md_dev" \
        --level=0 \
        --raid-devices="$n" \
        --chunk="${CHUNK_KIB}" \
        --name="${ARRAY_NAME}" \
        --run \
        --force \
        ${MDADM_EXTRA_OPTS:-} \
        "${devs[@]}"

    echo
    mdadm --detail "$md_dev"
    echo
    info "Array created: $md_dev"
    echo
    info "To persist the array across reboots:"
    info "  mdadm --detail --scan >> /etc/mdadm.conf"
    info "  dracut --force"
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --name)
                shift; [[ $# -gt 0 ]] || die "--name requires an argument"
                ARRAY_NAME="$1"
                ;;
            --chunk)
                shift; [[ $# -gt 0 ]] || die "--chunk requires an argument"
                [[ "$1" =~ ^[0-9]+$ ]] || die "--chunk value must be a number (KiB)"
                CHUNK_KIB="$1"
                ;;
            --count)
                shift; [[ $# -gt 0 ]] || die "--count requires an argument"
                [[ "$1" =~ ^[0-9]+$ ]] || die "--count value must be a number"
                DEVICE_COUNT="$1"
                ;;
            --wipe)
                WIPE=true
                ;;
            --yes|-y)
                YES=true
                ;;
            --devices)
                shift
                while [[ $# -gt 0 && "${1:0:2}" != "--" ]]; do
                    EXPLICIT_DEVS+=("$1")
                    shift
                done
                continue
                ;;
            --help|-h)
                echo "Usage: sudo $0 [options]"
                echo
                echo "Options:"
                echo "  --name <name>          Array name (default: certus-raid0)"
                echo "  --chunk <KiB>          Chunk size in KiB (default: 512)"
                echo "  --count <n>            Number of devices to use (default: all)"
                echo "  --wipe                 Wipe GPT/partition tables before creating array"
                echo "  --yes, -y              Non-interactive (use all discovered devices)"
                echo "  --devices <dev>...     Specify block devices explicitly"
                echo "  --help                 Show this help"
                echo
                echo "Environment:"
                echo "  MDADM_EXTRA_OPTS       Extra flags passed to mdadm --create"
                exit 0
                ;;
            *)
                die "Unknown option: $1 (try --help)"
                ;;
        esac
        shift
    done
}

# --- Main ---

check_root
check_deps
parse_args "$@"

if [[ ${#EXPLICIT_DEVS[@]} -gt 0 ]]; then
    echo
    echo "Using explicitly specified devices:"
    for dev in "${EXPLICIT_DEVS[@]}"; do
        [[ -b "$dev" ]] || die "Not a block device: $dev"
    done
    print_devices "${EXPLICIT_DEVS[@]}"
    if ! $YES; then
        echo
        echo "  WARNING: RAID0 provides NO data redundancy. All data on these devices will be DESTROYED."
        echo
        read -rp "  Proceed? [y/N] " confirm
        [[ "$confirm" =~ ^[yY] ]] || { echo "Aborted."; exit 0; }
    fi
    create_array "${EXPLICIT_DEVS[@]}"
    exit 0
fi

echo
echo "Discovering kernel-bound NVMe block devices..."
discover_kernel_nvme_devs

if [[ ${#KERNEL_DEVS[@]} -eq 0 ]]; then
    echo
    echo "No kernel-bound NVMe block devices found."
    echo "Use bind_vfio.sh reset-all to return vfio-bound devices to the nvme driver."
    exit 1
fi

echo
echo "Kernel-bound NVMe devices:"
print_devices "${KERNEL_DEVS[@]}"

# Select subset of devices if --count was specified
SELECTED_DEVS=("${KERNEL_DEVS[@]}")
if [[ $DEVICE_COUNT -gt 0 ]]; then
    if [[ $DEVICE_COUNT -gt ${#KERNEL_DEVS[@]} ]]; then
        die "--count $DEVICE_COUNT exceeds available devices (${#KERNEL_DEVS[@]})"
    fi
    SELECTED_DEVS=("${KERNEL_DEVS[@]:0:$DEVICE_COUNT}")
    echo
    echo "Using first $DEVICE_COUNT device(s):"
    print_devices "${SELECTED_DEVS[@]}"
fi

if ! $YES; then
    echo
    echo "  WARNING: RAID0 provides NO data redundancy. All data on these devices will be DESTROYED."
    echo
    read -rp "  Create RAID0 '${ARRAY_NAME}' from ${#SELECTED_DEVS[@]} device(s)? [y/N] " confirm
    [[ "$confirm" =~ ^[yY] ]] || { echo "Aborted."; exit 0; }
fi

create_array "${SELECTED_DEVS[@]}"
