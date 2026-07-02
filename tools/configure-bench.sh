#!/bin/bash
#
# configure-bench.sh — Configure system for Certus or SharedStorage benchmarks.
#
# Ensures all resources (NVMe, memory, CPUs) are co-located on the GPU's NUMA
# node. Sets kernel boot parameters via grubby and configures NVMe devices at
# runtime (vfio-pci for Certus, RAID0+XFS for SharedStorage).
#
# Memory can be limited two ways (MEM_METHOD env var):
#   cgroup  (default) — cap RAM at runtime via a systemd slice; no reboot.
#   kernel            — cap RAM via mem=/memmap= boot params; requires reboot.
#
# Usage:
#   sudo ./configure-bench.sh certus                     # Certus, cgroup mem limit (default)
#   sudo ./configure-bench.sh sharedstorage              # SharedStorage, cgroup mem limit
#   sudo MEM_METHOD=kernel ./configure-bench.sh certus   # Certus, kernel mem limit (reboot)
#   sudo CGROUP_MEM_MAX=32G ./configure-bench.sh certus  # cap the bench slice at 32 GiB
#   sudo ./configure-bench.sh status                     # Show current configuration
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
# memmap=254G$2G  → reserve node 0 from 2G to 256G (keep 2G for boot)
# mem=320G        → truncate at 320G (keeps 64G from node 1: 256G–320G)
NODE0_RESERVE='254G$2G'
MEM_LIMIT="320G"
TOTAL_USABLE_NODE1="64"  # GiB available on node 1 after memmap+mem

# Hugepages (1 GiB pages)
CERTUS_HUGEPAGES=56      # 56 GiB for SPDK DRAM tier, leaves 8G regular
SS_HUGEPAGES=0           # all regular memory for page cache

# Memory limiting method:
#   cgroup  — runtime cap via a systemd slice (default; no reboot)
#   kernel  — boot-param cap via mem=/memmap= (requires reboot)
MEM_METHOD="${MEM_METHOD:-cgroup}"

# cgroup slice used to cap benchmark memory at runtime. Run benchmarks under it
# with: systemd-run --slice=certus-bench.slice --scope numactl ... <command>
BENCH_SLICE="certus-bench.slice"
# RAM ceiling for the bench slice. If unset, defaults per-mode (see
# cgroup_mem_max_for): certus gets the regular-memory budget left after
# hugepages, sharedstorage gets the full node-1 budget. hugetlb pages are NOT
# charged to the memory cgroup on this kernel, so this only bounds regular RAM.
CGROUP_MEM_MAX="${CGROUP_MEM_MAX:-}"

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

# fail_certus / fail_ss: record that a check disqualifies a connector.
# Used via nameref by show_status (bash 4.3+). Reason shown in the summary.
fail_certus() { certus_ok=false; certus_bad+=("$1"); }
fail_ss()     { ss_ok=false;     ss_bad+=("$1"); }
# fail_both: a check that neither connector can tolerate (e.g. wrong GPU node).
fail_both()   { fail_certus "$1"; fail_ss "$1"; }

show_status() {
    # Per-connector readiness: a connector is "ready" only if EVERY check it
    # needs is satisfied. Checks that discriminate (hugepages, NVMe driver, ...)
    # will fail one connector while passing the other — that's expected. The
    # system is usable by a connector only when that connector has zero fails.
    local certus_ok=true ss_ok=true
    local -a certus_bad=() ss_bad=()

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
        fail_both "GPU on NUMA node $gpu_numa (expected $GPU_NUMA)"
    fi
    echo

    header "Memory"
    echo "  Total: ${total_mem} GiB (node 0: ${node0_mem} MB, node 1: ${node1_mem} MB)"

    if [[ $total_mem -le 100 ]]; then
        echo -e "  ${tag_certus}${tag_ss} RAM limited to ~${total_mem} GiB"
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        # cgroup mode caps RAM per-slice at runtime, not system-wide, so total
        # system RAM is expected to be full. The slice check below is authoritative.
        echo "  ${total_mem} GiB total — system RAM not capped (cgroup limits the bench slice, see below)"
    else
        echo -e "  ${tag_empty} ${total_mem} GiB — not limited (need mem=${MEM_LIMIT})"
        fail_both "system RAM not limited (need mem=${MEM_LIMIT})"
    fi
    if [[ -n "$node0_mem" && $node0_mem -le 10000 ]]; then
        echo -e "  ${tag_certus}${tag_ss} node 0 reserved (${node0_mem} MB)"
    elif [[ -n "$node0_mem" && $node0_mem -gt 10000 && "$MEM_METHOD" == "cgroup" ]]; then
        # cgroup mode doesn't reserve node 0 — allocations are pinned to node 1
        # via numactl --membind=1, so node 0 having memory is fine.
        echo "  node 0 has ${node0_mem} MB — not reserved (cgroup mode pins to node $GPU_NUMA via numactl)"
    elif [[ -n "$node0_mem" && $node0_mem -gt 10000 ]]; then
        echo -e "  ${tag_empty} node 0 has ${node0_mem} MB — memmap reservation not active"
        fail_both "node 0 not reserved (${node0_mem} MB, memmap inactive)"
    fi

    # cgroup memory limit (runtime). The cap is mode-specific: certus expects the
    # regular-RAM budget left after hugepages, sharedstorage the full node budget.
    local certus_gib=$((TOTAL_USABLE_NODE1 - CERTUS_HUGEPAGES))
    if systemctl is-active --quiet "$BENCH_SLICE" 2>/dev/null; then
        local slice_max
        slice_max=$(systemctl show -p MemoryMax --value "$BENCH_SLICE" 2>/dev/null)
        if [[ "$slice_max" == "infinity" || -z "$slice_max" ]]; then
            echo -e "  ${tag_empty} slice ${BENCH_SLICE} active but MemoryMax unset"
            [[ "$MEM_METHOD" == "cgroup" ]] && fail_both "cgroup slice active but MemoryMax unset"
        else
            local slice_gib=$((slice_max / 1024 / 1024 / 1024))
            local mem_tag=""
            if [[ $slice_gib -eq $certus_gib ]]; then mem_tag="${tag_certus}"; fi
            if [[ $slice_gib -eq $TOTAL_USABLE_NODE1 ]]; then mem_tag="${mem_tag}${tag_ss}"; fi
            if [[ -z "$mem_tag" ]]; then mem_tag="${tag_empty}"; fi
            echo -e "  ${mem_tag} cgroup slice ${BENCH_SLICE}: MemoryMax=${slice_gib} GiB (runtime limit)"
            # In cgroup mode, a cap that doesn't match a connector's budget disqualifies it.
            if [[ "$MEM_METHOD" == "cgroup" ]]; then
                [[ $slice_gib -ne $certus_gib ]] && fail_certus "cgroup cap ${slice_gib}G ≠ certus budget ${certus_gib}G"
                [[ $slice_gib -ne $TOTAL_USABLE_NODE1 ]] && fail_ss "cgroup cap ${slice_gib}G ≠ sharedstorage budget ${TOTAL_USABLE_NODE1}G"
            fi
        fi
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo -e "  ${tag_empty} cgroup slice ${BENCH_SLICE} not active — run 'sudo $0 <mode>' to create it"
        fail_both "cgroup slice ${BENCH_SLICE} not active"
    else
        echo "  cgroup slice ${BENCH_SLICE}: not active (MEM_METHOD=kernel)"
    fi

    echo "  Hugepages: $hp_total × 1G (free: $hp_free, node $GPU_NUMA: $hp_node)"
    # certus needs CERTUS_HUGEPAGES on the GPU node; sharedstorage needs none.
    if [[ $hp_total -eq $CERTUS_HUGEPAGES && $hp_node -ge $CERTUS_HUGEPAGES ]]; then
        echo -e "  ${tag_certus} $hp_total × 1G on node $GPU_NUMA"
        fail_ss "hugepages present ($hp_total × 1G; sharedstorage needs 0)"
    elif [[ $hp_total -eq $SS_HUGEPAGES ]]; then
        echo -e "  ${tag_ss} no hugepages — all RAM available for page cache"
        fail_certus "no hugepages (certus needs $CERTUS_HUGEPAGES × 1G)"
    else
        echo -e "  ${tag_empty} $hp_total hugepages — certus needs $CERTUS_HUGEPAGES on node $GPU_NUMA, sharedstorage needs $SS_HUGEPAGES"
        fail_certus "hugepages misconfigured ($hp_total total, $hp_node on node $GPU_NUMA; need $CERTUS_HUGEPAGES on node $GPU_NUMA)"
        [[ $hp_total -ne $SS_HUGEPAGES ]] && fail_ss "hugepages present ($hp_total × 1G; sharedstorage needs 0)"
    fi
    echo

    header "Kernel (running)"

    # mem= (only required in kernel mode; cgroup mode caps RAM via the slice)
    if echo "$cmdline" | grep -q "mem=${MEM_LIMIT}"; then
        echo -e "  ${tag_certus}${tag_ss} mem=${MEM_LIMIT}"
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo "  mem= not set — not needed (cgroup slice caps RAM at runtime)"
    else
        echo -e "  ${tag_empty} mem=${MEM_LIMIT} MISSING — page cache not limited"
        fail_both "mem=${MEM_LIMIT} missing (kernel mode)"
    fi

    # memmap= (must include the $offset to actually reserve memory).
    # The kernel normalizes the offset to hex in /proc/cmdline, so match either form.
    # Only required in kernel mode; cgroup mode pins to node $GPU_NUMA via numactl.
    if echo "$cmdline" | grep -qE 'memmap=254G\$(2G|0x80000000)'; then
        echo -e "  ${tag_certus}${tag_ss} memmap=254G\$2G — node 0 reserved"
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo "  memmap= not set — not needed (cgroup mode pins to node $GPU_NUMA via numactl)"
    elif echo "$cmdline" | grep -q "memmap="; then
        echo -e "  ${tag_empty} memmap= present but OFFSET MISSING — reservation not active"
        fail_both "memmap= offset missing (kernel mode)"
    else
        echo -e "  ${tag_empty} memmap= MISSING — memory not isolated to NUMA node $GPU_NUMA"
        fail_both "memmap= missing (kernel mode)"
    fi

    # iommu=pt — required for vfio-pci (certus only). sharedstorage uses the
    # kernel nvme driver, so it doesn't care.
    if echo "$cmdline" | grep -q "iommu=pt"; then
        echo -e "  ${tag_certus} iommu=pt"
    else
        echo -e "  ${tag_empty} iommu=pt MISSING — required for vfio-pci"
        fail_certus "iommu=pt missing (required for vfio-pci)"
    fi
    echo

    header "Kernel (next boot)"
    local next_boot_args
    next_boot_args=$(grubby --info=DEFAULT 2>/dev/null | grep ^args= | sed 's/^args="//' | sed 's/"$//')
    if [[ -z "$next_boot_args" && $EUID -ne 0 ]]; then
        echo -e "  ${YELLOW}Run with sudo to see next-boot kernel args${NC}"
    elif [[ -n "$next_boot_args" ]]; then
        local next_differs=false
        if [[ "$next_boot_args" != "$cmdline" ]]; then next_differs=true; fi

        if $next_differs; then
            echo -e "  ${YELLOW}Differs from running kernel:${NC}"
        else
            echo "  Same as running kernel"
        fi
        echo "  $next_boot_args"
    else
        echo "  (could not read grubby config)"
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
            fail_both "NVMe $bdf on wrong NUMA node ($dev_numa)"
        else
            status="ok"
        fi
        printf "  %-14s %-12s %-10s %-6s\n" "$bdf" "$drv" "$blk" "$status"
    done

    # vfio-pci → certus (SPDK userspace); nvme → sharedstorage (kernel driver).
    if [[ $nvme_count -gt 0 && $vfio_count -gt 0 ]]; then
        echo -e "  ${tag_empty} MIXED drivers: $vfio_count vfio-pci + $nvme_count nvme"
        fail_both "NVMe drivers mixed ($vfio_count vfio-pci + $nvme_count nvme)"
    elif [[ $vfio_count -eq ${#NVME_BDFS[@]} ]]; then
        echo -e "  ${tag_certus} all drives bound to vfio-pci"
        fail_ss "NVMe bound to vfio-pci (sharedstorage needs nvme)"
    elif [[ $nvme_count -eq ${#NVME_BDFS[@]} ]]; then
        echo -e "  ${tag_ss} all drives bound to nvme"
        fail_certus "NVMe bound to nvme (certus needs vfio-pci)"
    else
        echo -e "  ${tag_empty} NVMe drivers incomplete ($vfio_count vfio-pci, $nvme_count nvme of ${#NVME_BDFS[@]})"
        fail_both "NVMe drivers incomplete ($vfio_count vfio-pci, $nvme_count nvme of ${#NVME_BDFS[@]})"
    fi
    echo

    # RAID only matters for sharedstorage (needs the mounted XFS filesystem).
    if [[ $nvme_count -gt 0 ]]; then
        header "RAID"
        if [[ -e "$MD_DEVICE" ]] && mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
            echo -e "  ${tag_ss} $MD_DEVICE mounted at $MOUNT_POINT"
            df -h "$MOUNT_POINT" | tail -1 | awk '{printf "  Usage: %s / %s (%s)\n", $3, $2, $5}'
        elif [[ -e "$MD_DEVICE" ]]; then
            echo -e "  ${tag_empty} $MD_DEVICE exists but NOT mounted"
            fail_ss "RAID $MD_DEVICE not mounted"
        else
            echo -e "  ${tag_empty} no RAID configured"
            fail_ss "no RAID configured at $MOUNT_POINT"
        fi
        echo
    fi


    header "Run"
    echo "  numactl --cpunodebind=$GPU_NUMA --membind=$GPU_NUMA <command>"
    echo "  CPUs: $NUMA_CPUS"
    echo

    # --- Summary ---
    # A connector is READY only if every check it needs is satisfied. Most checks
    # discriminate (hugepages, NVMe driver, cgroup cap), so exactly one connector
    # is normally ready; a half-configured system is ready for NEITHER.
    header "Summary"
    if $certus_ok; then
        echo -e "  ${GREEN}READY for certus${NC}"
    else
        echo -e "  ${RED}NOT ready for certus:${NC}"
        for r in "${certus_bad[@]}"; do echo "    - $r"; done
    fi
    if $ss_ok; then
        echo -e "  ${GREEN}READY for sharedstorage${NC}"
    else
        echo -e "  ${RED}NOT ready for sharedstorage:${NC}"
        for r in "${ss_bad[@]}"; do echo "    - $r"; done
    fi
    echo
    if ! $certus_ok && ! $ss_ok; then
        echo -e "  Configure one with: sudo $0 {certus|sharedstorage}"
    fi
}

# ============================================================================
# Memory Limiting — cgroup (runtime, no reboot)
# ============================================================================

# Per-mode default RAM ceiling for the bench slice (regular memory only —
# hugetlb is not charged to the memory cgroup on this kernel).
#   certus         → node-1 budget minus the hugepage reservation (e.g. 64-56=8)
#   sharedstorage  → full node-1 budget (all of it is page cache)
cgroup_mem_max_for() {
    local mode=$1
    if [[ -n "$CGROUP_MEM_MAX" ]]; then
        echo "$CGROUP_MEM_MAX"
    elif [[ "$mode" == "certus" ]]; then
        echo "$((TOTAL_USABLE_NODE1 - CERTUS_HUGEPAGES))G"
    else
        echo "${TOTAL_USABLE_NODE1}G"
    fi
}

setup_cgroup_mem() {
    local mode=$1
    local mem_max
    mem_max=$(cgroup_mem_max_for "$mode")

    header "Memory limit via cgroup ($mem_max)"

    if ! command -v systemctl >/dev/null 2>&1; then
        echo -e "  ${RED}systemd not available — cannot use cgroup memory limit.${NC}" >&2
        echo "  Fall back to kernel params: sudo MEM_METHOD=kernel $0 <mode>" >&2
        exit 1
    fi

    # Create a persistent slice with a hard memory ceiling. MemoryMax is the hard
    # cap (OOM-kills on breach); MemorySwapMax=0 prevents the cap being dodged via
    # swap so the RAM ceiling is real.
    #
    # MemoryMax bounds *regular* RAM only — 1G hugepages are not charged to the
    # memory cgroup on this kernel. We deliberately do NOT try to cap hugepages
    # per-slice: systemd creates the slice's cgroup on demand (only when a process
    # is placed in it via systemd-run), so there's no cgroup dir to write
    # hugetlb.1GB.max into at configure time, and systemd exposes no directive for
    # it. It's also unnecessary — the hugepages=N boot param reserves exactly N
    # pages globally, so certus physically cannot use more than that. Total
    # footprint is therefore bounded to MemoryMax + N GiB (e.g. 8G + 56G = 64G).
    local unit="/etc/systemd/system/${BENCH_SLICE}"
    echo "  Writing $unit"
    {
        echo "# Benchmark memory-limit slice (generated by configure-bench.sh)"
        echo "[Unit]"
        echo "Description=Certus benchmark memory-limited slice"
        echo
        echo "[Slice]"
        echo "MemoryAccounting=yes"
        echo "MemoryMax=${mem_max}"
        echo "MemorySwapMax=0"
    } > "$unit"

    systemctl daemon-reload
    # Start the slice so its cgroup exists and the limit is live immediately.
    systemctl start "$BENCH_SLICE"

    if [[ "$mode" == "certus" ]]; then
        echo "  Hugepages bounded globally by hugepages=${CERTUS_HUGEPAGES} boot param"
        echo "  (total footprint ≈ MemoryMax ${mem_max} + ${CERTUS_HUGEPAGES}G hugepages)"
    fi

    echo -e "  ${GREEN}Slice ${BENCH_SLICE} active — MemoryMax=${mem_max}, swap disabled${NC}"
    echo "  Run benchmarks inside it with:"
    echo "    systemd-run --slice=${BENCH_SLICE} --scope numactl --cpunodebind=$GPU_NUMA --membind=$GPU_NUMA <command>"
    echo
    echo -e "  ${YELLOW}Note:${NC} MemoryMax caps regular RAM; run under numactl --membind=$GPU_NUMA"
    echo "  to keep allocations on the GPU node (no node-0 reservation needed)."
    echo "  Only the hugepages= boot param is required (1G pages reserved at boot)."
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

    header "Kernel Boot Parameters ($mode, MEM_METHOD=$MEM_METHOD)"

    # Remove old conflicting params
    local remove_args="hugepages hugepagesz default_hugepagesz mem memmap"
    echo "  Removing old params: $remove_args"
    grubby --update-kernel=ALL --remove-args="$remove_args" 2>/dev/null || true

    # Hugepages are always set via boot params (1G pages need contiguous memory
    # reserved at boot). The mem= RAM cap and memmap= node-0 reservation are only
    # needed in kernel mode; in cgroup mode the systemd slice caps RAM at runtime
    # and numactl --membind pins allocations to node $GPU_NUMA, so node 0 doesn't
    # need reserving.
    local new_args="default_hugepagesz=1G hugepagesz=1G hugepages=${hugepages}"
    if [[ "$MEM_METHOD" == "kernel" ]]; then
        new_args="$new_args mem=${MEM_LIMIT} memmap=${NODE0_RESERVE}"
    fi
    echo "  Setting: $new_args"
    grubby --update-kernel=ALL --args="$new_args"

    # Verify
    echo
    echo "  Next boot kernel: $(grubby --default-kernel)"
    echo "  Effective kernel args for next boot:"
    grubby --info=DEFAULT | grep ^args | sed 's/^args="/  /' | sed 's/"$//' | sed 's/\\\$/$/g'

    # Check if reboot needed
    local current_cmdline mem_ok=true
    current_cmdline=$(cat /proc/cmdline)
    if [[ "$MEM_METHOD" == "kernel" ]] && ! echo "$current_cmdline" | grep -q "mem=${MEM_LIMIT}"; then
        mem_ok=false
    fi
    if echo "$current_cmdline" | grep -q "hugepages=${hugepages}" && $mem_ok; then
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

    # Free any 1G hugepages on other NUMA nodes. The boot param `hugepages=N`
    # (no node qualifier) spreads N pages evenly across all nodes, so without
    # this we'd end up with N/2 on node0 PLUS `target` on node1. Zeroing the
    # other nodes guarantees exactly `target` pages, all on the GPU node.
    for other in /sys/devices/system/node/node*/hugepages/hugepages-1048576kB/nr_hugepages; do
        [[ "$other" == "$node_path" ]] && continue
        local other_n other_node
        other_n=$(cat "$other")
        other_node=$(basename "$(dirname "$(dirname "$(dirname "$other")")")")
        if [[ $other_n -gt 0 ]]; then
            echo "  Freeing $other_n × 1G hugepages on $other_node"
            echo 0 > "$other"
        fi
    done

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

# Free ALL 1G hugepages (all nodes) at runtime — for sharedstorage, which wants
# every GiB available as page cache. Boot reserves hugepages=CERTUS_HUGEPAGES, so
# without this SS would silently lose that RAM from its page-cache budget. Free
# pages can be released live (no reboot); pages held by a process cannot.
free_all_hugepages() {
    header "Hugepages (freeing for page cache)"

    local freed_any=false
    for f in /sys/devices/system/node/node*/hugepages/hugepages-1048576kB/nr_hugepages; do
        [[ -f "$f" ]] || continue
        local n node
        n=$(cat "$f")
        # path: .../node<N>/hugepages/hugepages-1048576kB/nr_hugepages
        node=$(basename "$(dirname "$(dirname "$(dirname "$f")")")")
        if [[ $n -gt 0 ]]; then
            echo "  Freeing $n × 1G on $node"
            echo 0 > "$f"
            freed_any=true
        fi
    done

    local remaining
    remaining=$(grep HugePages_Total /proc/meminfo | awk '{print $2}')
    if [[ $remaining -eq 0 ]]; then
        $freed_any && echo -e "  ${GREEN}All hugepages freed — full RAM available for page cache${NC}" \
                   || echo "  No hugepages reserved — nothing to free"
    else
        echo -e "  ${YELLOW}$remaining × 1G still reserved (likely in use by a process)${NC}"
        echo "  Stop any SPDK/certus process, or reboot with hugepages=0, to reclaim."
    fi
}

# ============================================================================
# Device Binding — vfio-pci (Certus)
# ============================================================================

bind_to_vfio() {
    header "Binding NVMe to vfio-pci (persistent)"

    if ! modprobe vfio-pci; then
        echo -e "  ${RED}Failed to load vfio-pci module. Is IOMMU enabled?${NC}" >&2
        exit 1
    fi

    teardown_raid_if_active

    # Runtime bind
    for bdf in "${NVME_BDFS[@]}"; do
        local drv
        drv=$(get_driver "$bdf")

        if [[ "$drv" == "vfio-pci" ]]; then
            echo "  $bdf: already bound to vfio-pci"
            continue
        fi

        local dev_path="/sys/bus/pci/devices/$bdf"

        if [[ "$drv" != "none" ]]; then
            echo "  $bdf: unbinding from $drv"
            echo "$bdf" > "$dev_path/driver/unbind"
        fi

        echo "  $bdf: binding to vfio-pci"
        echo "vfio-pci" > "$dev_path/driver_override"
        echo "$bdf" > /sys/bus/pci/drivers_probe
        echo "" > "$dev_path/driver_override"
    done

    # Persistent udev rule
    local udev_rule="/etc/udev/rules.d/99-certus-vfio.rules"
    echo "  Writing $udev_rule"
    {
        echo "# Bind Certus NVMe drives to vfio-pci (generated by configure-bench.sh)"
        for bdf in "${NVME_BDFS[@]}"; do
            local slot="${bdf#*:}"  # strip domain
            echo "ACTION==\"add\", SUBSYSTEM==\"pci\", KERNEL==\"$bdf\", ATTR{driver_override}=\"vfio-pci\", RUN+=\"/bin/sh -c 'echo $bdf > /sys/bus/pci/drivers_probe'\""
        done
    } > "$udev_rule"
    udevadm control --reload-rules

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
    echo "  All ${#NVME_BDFS[@]} NVMe devices bound to vfio-pci (persists via $udev_rule)."
}

# ============================================================================
# Device Binding — nvme kernel driver (SharedStorage)
# ============================================================================

bind_to_nvme() {
    header "Binding NVMe to kernel driver (persistent)"

    # Remove persistent vfio rule
    local udev_rule="/etc/udev/rules.d/99-certus-vfio.rules"
    if [[ -f "$udev_rule" ]]; then
        echo "  Removing $udev_rule"
        rm -f "$udev_rule"
        udevadm control --reload-rules
    fi

    for bdf in "${NVME_BDFS[@]}"; do
        local drv
        drv=$(get_driver "$bdf")

        if [[ "$drv" == "nvme" ]]; then
            echo "  $bdf: already bound to nvme"
            continue
        fi

        local dev_path="/sys/bus/pci/devices/$bdf"

        if [[ "$drv" != "none" ]]; then
            echo "  $bdf: unbinding from $drv"
            echo "$bdf" > "$dev_path/driver/unbind"
        fi

        echo "  $bdf: binding to nvme"
        echo "" > "$dev_path/driver_override"
        echo "$bdf" > /sys/bus/pci/drivers_probe
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
    if [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo "  Memory limit: cgroup slice ${BENCH_SLICE} (MemoryMax=$(cgroup_mem_max_for "$mode"), no reboot)"
    else
        echo "  Memory limit: kernel mem=${MEM_LIMIT} (reboot required)"
    fi
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

    # 1b. Memory limit via cgroup (runtime, no reboot) — the default.
    if [[ "$MEM_METHOD" == "cgroup" ]]; then
        setup_cgroup_mem "$mode"
    fi

    # 2. Hugepages
    if [[ "$mode" == "certus" ]]; then
        allocate_hugepages_node "$CERTUS_HUGEPAGES"
    else
        # sharedstorage wants all RAM as page cache — release the boot reservation.
        free_all_hugepages
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
    if [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo "    systemd-run --slice=${BENCH_SLICE} --scope \\"
        echo "      numactl --cpunodebind=$GPU_NUMA --membind=$GPU_NUMA <command>"
        echo "    (the slice enforces MemoryMax=$(cgroup_mem_max_for "$mode"))"
    else
        echo "    numactl --cpunodebind=$GPU_NUMA --membind=$GPU_NUMA <command>"
    fi
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
