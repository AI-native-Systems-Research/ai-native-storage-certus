#!/bin/bash
#
# configure-bench.sh — Configure system for Certus or SharedStorage benchmarks.
#
# Ensures all resources (NVMe, memory, CPUs) are co-located on the GPU's NUMA
# node. Sets kernel boot parameters via grubby and configures NVMe devices at
# runtime (vfio-pci for Certus, RAID0+XFS for SharedStorage).
#
# Usage:
#   sudo ./configure-bench.sh certus         # Certus NVMe mode
#   sudo ./configure-bench.sh sharedstorage  # SharedStorage (llm-d fs) mode
#   sudo ./configure-bench.sh status         # Show current configuration
#
set -euo pipefail

# ============================================================================
# Configuration — edit these if hardware changes
# ============================================================================

# GPU NUMA node (all resources pinned here)
GPU_NUMA=1

# NVMe devices on the GPU's NUMA node
NVME_BDFS=("0000:c1:00.0" "0000:c2:00.0" "0000:c3:00.0" "0000:c4:00.0")

# CPUs on the GPU's NUMA node
NUMA_CPUS="16-31,48-63"

# RAID / filesystem
MD_DEVICE="/dev/md0"
MOUNT_POINT="/mnt/fs-backend-bench"
XFS_LABEL="fs-bench"

# Memory layout — derived from NUMA physical address ranges:
#   Node 0: 0x0000000000 – 0x4000000000 (256 GiB)
#   Node 1: 0x4000000000 – 0x8000000000 (258 GiB)
#
# memmap=254G$0x80000000  → reserve node 0 from 2G to 256G (keep 2G for boot)
# mem=320G                → truncate at 320G (keeps 64G from node 1: 256G–320G)
NODE0_RESERVE="254G\$0x80000000"   # escaped $ for grubby
MEM_LIMIT="320G"
TOTAL_USABLE_NODE1="64"  # GiB available on node 1 after memmap+mem

# Hugepages (1 GiB pages)
CERTUS_HUGEPAGES=56      # 56 GiB for SPDK DRAM tier, leaves 8G regular
SS_HUGEPAGES=0           # all regular memory for page cache

# ============================================================================
# Helpers
# ============================================================================

BLUE='\033[0;34m'
CYAN='\033[0;36m'
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

tag_certus="${BLUE}[certus]${NC}"
tag_ss="${CYAN}[sharedstorage]${NC}"
tag_empty="${YELLOW}[]${NC}"

header() { echo -e "\n${BOLD}=== $* ===${NC}"; }

check_root() {
    if [[ $EUID -ne 0 ]]; then
        error "This script must be run as root (use sudo)."
        exit 1
    fi
}

# Get current driver for a PCI BDF
get_driver() {
    local bdf=$1
    local dev_path="/sys/bus/pci/devices/$bdf"
    if [[ -L "$dev_path/driver" ]]; then
        basename "$(readlink "$dev_path/driver")"
    else
        echo "none"
    fi
}

# Get block device name for a PCI NVMe controller
get_blkdev() {
    local bdf=$1
    local dev_path="/sys/bus/pci/devices/$bdf"
    if [[ -d "$dev_path/nvme" ]]; then
        for ctrl in "$dev_path"/nvme/nvme*; do
            for ns in "$ctrl"/nvme*n*; do
                [[ -d "$ns" ]] && basename "$ns" && return
            done
        done
    fi
    echo "-"
}

# ============================================================================
# Status
# ============================================================================

show_status() {
    local issues=0

    # --- Gather state ---
    local cmdline
    cmdline=$(cat /proc/cmdline)

    local nvme_count=0 vfio_count=0
    for bdf in "${NVME_BDFS[@]}"; do
        local d
        d=$(get_driver "$bdf")
        if [[ "$d" == "nvme" ]]; then ((++nvme_count)); fi
        if [[ "$d" == "vfio-pci" ]]; then ((++vfio_count)); fi
    done

    local hp_total hp_free hp_node
    hp_total=$(grep HugePages_Total /proc/meminfo | awk '{print $2}')
    hp_free=$(grep HugePages_Free /proc/meminfo | awk '{print $2}')
    hp_node=0
    if [[ -f /sys/devices/system/node/node${GPU_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages ]]; then
        hp_node=$(cat /sys/devices/system/node/node${GPU_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages)
    fi

    local total_mem node0_mem node1_mem
    total_mem=$(free -g | awk '/Mem:/{print $2}')
    node0_mem=$(numactl -H 2>/dev/null | grep "node 0 size" | awk '{print $4}')
    node1_mem=$(numactl -H 2>/dev/null | grep "node 1 size" | awk '{print $4}')

    # --- Display checks ---
    # Each check is tagged with which connector(s) it currently SATISFIES.
    # Empty [] = satisfies neither (problem). Colored tag = meets that connector's requirement.

    header "GPU (determines NUMA node)"
    local gpu_numa
    gpu_numa=$(cat /sys/bus/pci/devices/0000:a1:00.0/numa_node 2>/dev/null || echo "?")
    if [[ "$gpu_numa" == "$GPU_NUMA" ]]; then
        echo -e "  ${tag_certus}${tag_ss} GPU on NUMA node $gpu_numa — all resources pinned here"
    else
        echo -e "  ${tag_empty} GPU on NUMA node $gpu_numa, expected $GPU_NUMA"
        ((++issues))
    fi
    echo

    header "Memory"
    echo "  Total: ${total_mem} GiB (node 0: ${node0_mem} MB, node 1: ${node1_mem} MB)"

    if [[ $total_mem -le 100 ]]; then
        echo -e "  ${tag_certus}${tag_ss} RAM limited to ~${total_mem} GiB"
    else
        echo -e "  ${tag_empty} ${total_mem} GiB — not limited (need mem=${MEM_LIMIT})"
        ((++issues))
    fi
    if [[ -n "$node0_mem" && $node0_mem -le 10000 ]]; then
        echo -e "  ${tag_certus}${tag_ss} node 0 reserved (${node0_mem} MB)"
    elif [[ -n "$node0_mem" && $node0_mem -gt 10000 ]]; then
        echo -e "  ${tag_empty} node 0 has ${node0_mem} MB — memmap reservation not active"
        ((++issues))
    fi
    echo

    header "Hugepages"
    echo "  Total 1G pages: $hp_total (free: $hp_free, node $GPU_NUMA: $hp_node)"

    if [[ $hp_total -eq $CERTUS_HUGEPAGES && $hp_node -ge $CERTUS_HUGEPAGES ]]; then
        echo -e "  ${tag_certus} $hp_total × 1G on node $GPU_NUMA"
    elif [[ $hp_total -eq $SS_HUGEPAGES ]]; then
        echo -e "  ${tag_ss} no hugepages — all RAM available for page cache"
    else
        local hp_tag=""
        if [[ $hp_total -eq $CERTUS_HUGEPAGES ]]; then hp_tag="${tag_certus}"; fi
        if [[ $hp_total -eq $SS_HUGEPAGES ]]; then hp_tag="${tag_ss}"; fi
        if [[ -z "$hp_tag" ]]; then
            echo -e "  ${tag_empty} $hp_total hugepages — certus needs $CERTUS_HUGEPAGES, sharedstorage needs $SS_HUGEPAGES"
            ((++issues))
        else
            echo -e "  ${hp_tag} $hp_total hugepages"
        fi
        if [[ $hp_total -gt 0 && $hp_node -eq 0 ]]; then
            echo -e "  ${tag_empty} hugepages not on node $GPU_NUMA — cross-NUMA penalty"
            ((++issues))
        fi
    fi
    echo

    header "Kernel"

    # mem= — both require it (NUMA isolation + page cache limit)
    if echo "$cmdline" | grep -q "mem=${MEM_LIMIT}"; then
        echo -e "  ${tag_certus}${tag_ss} mem=${MEM_LIMIT}"
    else
        echo -e "  ${tag_empty} mem=${MEM_LIMIT} MISSING — page cache not limited"
        ((++issues))
    fi

    # memmap= — both require it (NUMA isolation)
    if echo "$cmdline" | grep -q "memmap="; then
        echo -e "  ${tag_certus}${tag_ss} memmap= present — memory isolated to NUMA node $GPU_NUMA"
    else
        echo -e "  ${tag_empty} memmap= MISSING — memory not isolated to NUMA node $GPU_NUMA"
        ((++issues))
    fi

    # hugepagesz=1G — certus requires it
    if echo "$cmdline" | grep -q "hugepagesz=1G"; then
        echo -e "  ${tag_certus} hugepagesz=1G"
    else
        echo -e "  ${tag_empty} hugepagesz=1G MISSING"
        ((++issues))
    fi

    # hugepages count
    if echo "$cmdline" | grep -qP "hugepages=\d+"; then
        local cmdline_hp
        cmdline_hp=$(echo "$cmdline" | grep -oP 'hugepages=\K\d+')
        local hp_tag=""
        if [[ $cmdline_hp -eq $CERTUS_HUGEPAGES ]]; then hp_tag="${tag_certus}"; fi
        if [[ $cmdline_hp -eq $SS_HUGEPAGES ]]; then hp_tag="${hp_tag}${tag_ss}"; fi
        if [[ -z "$hp_tag" ]]; then
            echo -e "  ${tag_empty} hugepages=$cmdline_hp — certus needs $CERTUS_HUGEPAGES, sharedstorage needs $SS_HUGEPAGES"
            ((++issues))
        else
            echo -e "  ${hp_tag} hugepages=$cmdline_hp"
        fi
    fi

    # iommu=pt — certus requires it
    if echo "$cmdline" | grep -q "iommu=pt"; then
        echo -e "  ${tag_certus} iommu=pt"
    else
        echo -e "  ${tag_empty} iommu=pt MISSING — required for vfio-pci"
        ((++issues))
    fi
    echo

    header "NVMe Devices (NUMA node $GPU_NUMA)"
    printf "  %-14s %-12s %-10s %-6s\n" "BDF" "Driver" "Block Dev" "Status"
    printf "  %-14s %-12s %-10s %-6s\n" "--------------" "------------" "----------" "------"
    for bdf in "${NVME_BDFS[@]}"; do
        local drv blk status
        drv=$(get_driver "$bdf")
        blk=$(get_blkdev "$bdf")
        local dev_numa
        dev_numa=$(cat "/sys/bus/pci/devices/$bdf/numa_node" 2>/dev/null || echo "?")
        if [[ "$dev_numa" != "$GPU_NUMA" ]]; then
            status="WRONG NUMA($dev_numa)"
            ((++issues))
        else
            status="ok"
        fi
        printf "  %-14s %-12s %-10s %-6s\n" "$bdf" "$drv" "$blk" "$status"
    done

    if [[ $nvme_count -gt 0 && $vfio_count -gt 0 ]]; then
        echo -e "  ${tag_empty} MIXED drivers: $vfio_count vfio-pci + $nvme_count nvme"
        ((++issues))
    elif [[ $vfio_count -eq ${#NVME_BDFS[@]} ]]; then
        echo -e "  ${tag_certus} all drives bound to vfio-pci"
    elif [[ $nvme_count -eq ${#NVME_BDFS[@]} ]]; then
        echo -e "  ${tag_ss} all drives bound to nvme"
    fi
    echo

    if [[ $nvme_count -gt 0 ]]; then
        header "RAID"
        if [[ -e "$MD_DEVICE" ]] && mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
            echo -e "  ${tag_ss} $MD_DEVICE mounted at $MOUNT_POINT"
            df -h "$MOUNT_POINT" | tail -1 | awk '{printf "  Usage: %s / %s (%s)\n", $3, $2, $5}'
        elif [[ -e "$MD_DEVICE" ]]; then
            echo -e "  ${tag_empty} $MD_DEVICE exists but NOT mounted"
            ((++issues))
        else
            echo -e "  ${tag_empty} no RAID configured"
            ((++issues))
        fi
        echo
    fi


    header "Run"
    echo "  numactl --cpunodebind=$GPU_NUMA --membind=$GPU_NUMA <command>"
    echo "  CPUs: $NUMA_CPUS"
    echo

    # --- Summary ---
    header "Summary"
    if [[ $issues -eq 0 ]]; then
        echo -e "  ${GREEN}All checks passed.${NC}"
    else
        echo -e "  ${RED}$issues issue(s)${NC} — fix with: sudo $0 {certus|sharedstorage}"
    fi
}

# ============================================================================
# Kernel Parameters
# ============================================================================

set_kernel_params() {
    local mode=$1
    local hugepages

    if [[ "$mode" == "certus" ]]; then
        hugepages=$CERTUS_HUGEPAGES
    else
        hugepages=$SS_HUGEPAGES
    fi

    header "Kernel Boot Parameters ($mode)"

    # Remove old conflicting params
    local remove_args="hugepages hugepagesz default_hugepagesz mem memmap"
    echo "  Removing old params: $remove_args"
    grubby --update-kernel=ALL --remove-args="$remove_args" 2>/dev/null || true

    # Set new params
    local new_args="default_hugepagesz=1G hugepagesz=1G hugepages=${hugepages} mem=${MEM_LIMIT} memmap=${NODE0_RESERVE}"
    echo "  Setting: $new_args"
    grubby --update-kernel=ALL --args="$new_args"

    # Verify
    echo
    echo "  Effective kernel args for next boot:"
    grubby --info=DEFAULT | grep ^args | sed 's/^args="/  /' | sed 's/"$//'

    # Check if reboot needed
    local current_cmdline
    current_cmdline=$(cat /proc/cmdline)
    if echo "$current_cmdline" | grep -q "hugepages=${hugepages}" && \
       echo "$current_cmdline" | grep -q "mem=${MEM_LIMIT}"; then
        echo "  Kernel params already active — no reboot needed."
    else
        echo
        echo -e "  ${YELLOW}REBOOT REQUIRED${NC} for kernel parameters to take effect."
        echo "  Run: sudo reboot"
    fi
}

# ============================================================================
# Hugepage Allocation (Certus)
# ============================================================================

allocate_hugepages_node() {
    local target=$1
    local node_path="/sys/devices/system/node/node${GPU_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages"

    header "Hugepages (node $GPU_NUMA)"

    if [[ ! -f "$node_path" ]]; then
        echo -e "  ${YELLOW}1G hugepage support not available at runtime${NC}"
        echo "  Will be allocated at next boot from node $GPU_NUMA (memmap reserves node 0)"
        return
    fi

    local current
    current=$(cat "$node_path")
    if [[ $current -ge $target ]]; then
        echo "  Node $GPU_NUMA already has $current × 1G hugepages (need $target)"
        return
    fi

    echo "  Allocating $target × 1G hugepages on node $GPU_NUMA..."
    echo "$target" > "$node_path"

    local actual
    actual=$(cat "$node_path")
    if [[ $actual -lt $target ]]; then
        echo -e "  ${YELLOW}Only got $actual / $target — 1G pages require contiguous memory${NC}"
        echo "  Reboot required for full allocation (boot param handles it)."
    else
        echo -e "  ${GREEN}Allocated $actual × 1G hugepages on node $GPU_NUMA${NC}"
    fi
}

# ============================================================================
# Device Binding — vfio-pci (Certus)
# ============================================================================

bind_to_vfio() {
    header "Binding NVMe to vfio-pci (persistent)"

    if ! command -v driverctl &>/dev/null; then
        echo -e "  ${RED}driverctl not found. Install with: dnf install driverctl${NC}" >&2
        exit 1
    fi

    if ! modprobe vfio-pci; then
        echo -e "  ${RED}Failed to load vfio-pci module. Is IOMMU enabled?${NC}" >&2
        exit 1
    fi

    teardown_raid_if_active

    for bdf in "${NVME_BDFS[@]}"; do
        local drv
        drv=$(get_driver "$bdf")

        if [[ "$drv" == "vfio-pci" ]]; then
            echo "  $bdf: already bound to vfio-pci (persistent)"
            continue
        fi

        echo "  $bdf: setting persistent override → vfio-pci"
        driverctl set-override "$bdf" vfio-pci
    done

    # Fix permissions on IOMMU groups
    for bdf in "${NVME_BDFS[@]}"; do
        local dev_path="/sys/bus/pci/devices/$bdf"
        if [[ -L "$dev_path/iommu_group" ]]; then
            local group
            group=$(basename "$(readlink "$dev_path/iommu_group")")
            chmod a+rw "/dev/vfio/$group" 2>/dev/null && \
                echo "  $bdf: set permissions on /dev/vfio/$group"
        fi
    done

    echo
    echo "  All ${#NVME_BDFS[@]} NVMe devices bound to vfio-pci (survives reboot)."
}

# ============================================================================
# Device Binding — nvme kernel driver (SharedStorage)
# ============================================================================

bind_to_nvme() {
    header "Binding NVMe to kernel driver (persistent)"

    if ! command -v driverctl &>/dev/null; then
        echo -e "  ${RED}driverctl not found. Install with: dnf install driverctl${NC}" >&2
        exit 1
    fi

    for bdf in "${NVME_BDFS[@]}"; do
        local drv
        drv=$(get_driver "$bdf")

        if [[ "$drv" == "nvme" ]]; then
            echo "  $bdf: already bound to nvme"
        else
            echo "  $bdf: clearing vfio-pci override → nvme"
        fi
        driverctl unset-override "$bdf" 2>/dev/null || true
    done

    # Wait for block devices to appear
    echo "  Waiting for block devices..."
    sleep 2

    for bdf in "${NVME_BDFS[@]}"; do
        local blk
        blk=$(get_blkdev "$bdf")
        echo "  $bdf → /dev/$blk"
    done
}

# ============================================================================
# RAID0 Setup (SharedStorage)
# ============================================================================

teardown_raid_if_active() {
    if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
        echo "  Unmounting $MOUNT_POINT"
        umount "$MOUNT_POINT"
    fi

    if [[ -e "$MD_DEVICE" ]]; then
        echo "  Stopping $MD_DEVICE"
        mdadm --stop "$MD_DEVICE" 2>/dev/null || true
    fi
}

setup_raid() {
    header "Setting up RAID0 + XFS"

    # Collect block device paths
    local blkdevs=()
    for bdf in "${NVME_BDFS[@]}"; do
        local blk
        blk=$(get_blkdev "$bdf")
        if [[ "$blk" == "-" ]]; then
            echo -e "  ${RED}$bdf has no block device — is it bound to nvme?${NC}" >&2
            exit 1
        fi
        blkdevs+=("/dev/$blk")
    done

    # Check if RAID already exists and is assembled
    if [[ -e "$MD_DEVICE" ]] && mdadm --detail "$MD_DEVICE" &>/dev/null; then
        echo "  $MD_DEVICE already assembled"
    else
        # Try to assemble existing array first
        if mdadm --assemble "$MD_DEVICE" "${blkdevs[@]}" 2>/dev/null; then
            echo "  Assembled existing $MD_DEVICE"
        else
            # Create new RAID0
            echo -e "  ${YELLOW}Creating new RAID0 — this will DESTROY data on ${blkdevs[*]}${NC}"
            mdadm --create "$MD_DEVICE" \
                --level=0 \
                --raid-devices=${#blkdevs[@]} \
                --chunk=512K \
                "${blkdevs[@]}"
            echo "  Created $MD_DEVICE (RAID0, 512K chunks, ${#blkdevs[@]} devices)"

            # Format with XFS
            echo "  Formatting with XFS..."
            mkfs.xfs -f -L "$XFS_LABEL" "$MD_DEVICE"
        fi
    fi

    # Mount
    mkdir -p "$MOUNT_POINT"
    if ! mountpoint -q "$MOUNT_POINT"; then
        mount "$MD_DEVICE" "$MOUNT_POINT"
        echo "  Mounted $MD_DEVICE at $MOUNT_POINT"
    else
        echo "  Already mounted at $MOUNT_POINT"
    fi

    # Clean stale KV data
    if [[ -d "$MOUNT_POINT/shared-kv" ]]; then
        local kv_size
        kv_size=$(du -sh "$MOUNT_POINT/shared-kv" 2>/dev/null | cut -f1)
        echo "  Existing KV data: $kv_size — clearing"
        rm -rf "$MOUNT_POINT/shared-kv"
    fi
    mkdir -p "$MOUNT_POINT/shared-kv"

    echo
    echo "  RAID0 ready at $MOUNT_POINT"
    df -h "$MOUNT_POINT" | tail -1 | awk '{printf "  Capacity: %s, Used: %s\n", $2, $3}'
}

# ============================================================================
# Main
# ============================================================================

usage() {
    echo "Usage: sudo $0 {certus|sharedstorage|status}"
    echo
    echo "Modes:"
    echo "  certus         56G hugepages, NVMe → vfio-pci (SPDK)"
    echo "  sharedstorage  0 hugepages, NVMe → RAID0 + XFS"
    echo "  status         Show current configuration"
    echo
    echo "System topology:"
    echo "  GPU NUMA node:  $GPU_NUMA"
    echo "  NVMe (node $GPU_NUMA): ${NVME_BDFS[*]}"
    echo "  CPUs (node $GPU_NUMA): $NUMA_CPUS"
    echo "  Memory budget:  ${TOTAL_USABLE_NODE1} GiB from node $GPU_NUMA"
    exit 1
}

main() {
    local mode="${1:-}"

    case "$mode" in
        status)
            show_status
            exit 0
            ;;
        certus|sharedstorage)
            check_root
            ;;
        *)
            usage
            ;;
    esac

    header "Configuring for: $mode"
    echo "  Memory: ${TOTAL_USABLE_NODE1} GiB on NUMA node $GPU_NUMA"
    if [[ "$mode" == "certus" ]]; then
        echo "  Hugepages: ${CERTUS_HUGEPAGES} × 1G (SPDK DRAM tier)"
        echo "  Regular mem: $((TOTAL_USABLE_NODE1 - CERTUS_HUGEPAGES)) GiB"
        echo "  NVMe: vfio-pci (SPDK userspace)"
    else
        echo "  Hugepages: 0"
        echo "  Regular mem: ${TOTAL_USABLE_NODE1} GiB (page cache)"
        echo "  NVMe: RAID0 + XFS at $MOUNT_POINT"
    fi
    echo "  CPUs: $NUMA_CPUS"
    echo

    # 1. Set kernel parameters
    set_kernel_params "$mode"

    # 2. Hugepages
    if [[ "$mode" == "certus" ]]; then
        allocate_hugepages_node "$CERTUS_HUGEPAGES"
    fi

    # 3. Configure devices
    if [[ "$mode" == "certus" ]]; then
        bind_to_vfio
    else
        bind_to_nvme
        setup_raid
    fi

    # 4. Summary
    header "Done"
    echo
    echo "  Run benchmarks with:"
    echo "    numactl --cpunodebind=$GPU_NUMA --membind=$GPU_NUMA <command>"
    echo
    if [[ "$mode" == "certus" ]]; then
        echo "  Certus server should use:"
        echo "    --pci-allowlist ${NVME_BDFS[0]},${NVME_BDFS[1]},${NVME_BDFS[2]},${NVME_BDFS[3]}"
        echo "    --memory-tier-size $((CERTUS_HUGEPAGES))G"
    else
        echo "  SharedStorage KV path:"
        echo "    $MOUNT_POINT/shared-kv"
    fi
    echo
}

main "$@"
