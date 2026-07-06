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

# Directory this script lives in (tools/), used to locate the built .so etc.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ============================================================================
# Configuration — edit these if hardware changes
# ============================================================================

# Physical GPU location (A30). NOT necessarily where the benchmark runs — see
# BENCH_NUMA below. Used only for the cross-node advisory in status.
GPU_NUMA=1

# NUMA node the benchmark pins all its resources (CPUs, NVMe, memory, hugepages)
# to. Default 0. We deliberately bench on node 0 even though the GPU is on node
# 1: node 0 is the BOTTOM of physical memory, so capping RAM to isolate it needs
# only a plain `mem=NNG` boot param — no `memmap=SIZE$OFFSET`, hence no literal
# `$`. On this RHEL 9 BLS box GRUB's blscfg does `$var` expansion on the stored
# options= line, which silently ate the `$2G` offset of the node-1 memmap= form
# (→ node 1 = 0 MB) and made a backslash-escaped form unbootable. Benching on
# node 0 sidesteps that trap entirely. The GPU stays on node 1 and is reached
# cross-UPI (its VRAM is on-card; only host DMA crosses the interconnect).
BENCH_NUMA="${BENCH_NUMA:-0}"

# Per-node resource tables, selected by BENCH_NUMA.
#   node 0: CPUs 0-15,32-47   NVMe 61–64:00.0
#   node 1: CPUs 16-31,48-63  NVMe c1–c4:00.0  (+ A30 GPU a1:00.0)
if [[ "$BENCH_NUMA" == "0" ]]; then
    NVME_BDFS=("0000:61:00.0" "0000:62:00.0" "0000:63:00.0" "0000:64:00.0")
    NUMA_CPUS="0-15,32-47"
else
    NVME_BDFS=("0000:c1:00.0" "0000:c2:00.0" "0000:c3:00.0" "0000:c4:00.0")
    NUMA_CPUS="16-31,48-63"
fi

# RAID / filesystem
MD_DEVICE="/dev/md0"
MOUNT_POINT="/mnt/fs-backend-bench"
XFS_LABEL="fs-bench"

# Minimum regular RAM vLLM needs to init + generate (empirical: OOMs below ~16G
# with an 8B model). The certus cgroup cap must clear this or vLLM is OOM-killed.
VLLM_MIN_RAM_GIB=16

# Memory cap (regular RAM). Node 0 sits at physical 0, so a bare `mem=${BENCH_MEM}`
# keeps the bottom BENCH_MEM (all node 0) and drops everything above it (node 1)
# — no memmap, no `$`, no boot risk. Default 64 GiB to match the prior node-1
# 64 GiB runs for apples-to-apples comparison; override via env for sweeps.
#
# NOTE: the old node-1 approach (`mem=320G memmap=254G$2G`) is intentionally gone.
# The memmap= reserved-region form requires a literal `$`, which GRUB's blscfg
# strips from the BLS options= line at boot — see the BENCH_NUMA comment above.
# Do NOT reintroduce a `$`-bearing kernel arg here.
BENCH_MEM="${BENCH_MEM:-64G}"
BENCH_MEM=24
TOTAL_USABLE_NODE1="${BENCH_MEM%G}"  # GiB budget on the bench node (numeric)

# Hugepages (1 GiB pages)
CERTUS_HUGEPAGES="$((BENCH_MEM - VLLM_MIN_RAM_GIB))"      # 48 GiB SPDK DRAM tier, leaves 16G regular for vLLM (needs DPDK RTE_MAX_MEM_MB_PER_LIST raised to 64G)
SS_HUGEPAGES=0           # all regular memory for page cache

# DPDK single-allocation ceiling. spdk_zmalloc -> rte_malloc cannot return a
# block spanning more than one memseg list (RTE_MAX_MEM_MB_PER_LIST), so a
# single Certus DRAM tier is hard-capped just under this regardless of how many
# hugepages exist. Raised from the stock 32 GiB to 64 GiB via a patched
# deps/spdk/dpdk/config/rte_config.h (rebuilt with deps/build_spdk.sh) to allow
# a 48 GiB tier. Keep CERTUS_HUGEPAGES below this. NOTE: also bounded by
# RTE_MAX_MEM_MB_PER_TYPE=64 GiB, so the practical single-tier max is ~60 GiB.
DPDK_MEMSEG_LIST_GIB=64

# DPDK reserves ~3 x 1G hugepages for its own EAL heap + per-drive DMA buffers
# (measured: 48-page pool - 44G tier = 4 free before drives, 1 after). So a
# single DRAM-tier spdk_zmalloc maxes at ~(CERTUS_HUGEPAGES - this) GiB. The run
# script's dram_cache_bytes must stay under that, not the full pool.
DPDK_HUGEPAGE_OVERHEAD_GIB=3

# Built native module whose allocation path must include SPDK hugepage support.
CERTUS_NATIVE_SO="certus-connector/certus_native/certus_native.cpython-312-x86_64-linux-gnu.so"

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
    if [[ -f /sys/devices/system/node/node${BENCH_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages ]]; then
        hp_node=$(cat /sys/devices/system/node/node${BENCH_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages)
    fi

    local total_mem node0_mem node1_mem
    total_mem=$(free -g | awk '/Mem:/{print $2}')
    node0_mem=$(numactl -H 2>/dev/null | grep "node 0 size" | awk '{print $4}')
    node1_mem=$(numactl -H 2>/dev/null | grep "node 1 size" | awk '{print $4}')

    # --- Display checks ---
    # Each check is tagged with which connector(s) it currently SATISFIES.
    # Empty [] = satisfies neither (problem). Colored tag = meets that connector's requirement.

    header "GPU / bench NUMA node"
    local gpu_numa
    gpu_numa=$(cat /sys/bus/pci/devices/0000:a1:00.0/numa_node 2>/dev/null || echo "?")
    if [[ "$gpu_numa" == "$BENCH_NUMA" ]]; then
        echo -e "  ${tag_certus}${tag_ss} GPU on NUMA node $gpu_numa — benching on the same node"
    else
        # Benching cross-node is a deliberate choice (avoids the memmap=…$… boot
        # trap on this box), not an error. The GPU's VRAM is on-card; only host
        # DMA crosses UPI. Informational, not a fail.
        echo -e "  ${tag_certus}${tag_ss} GPU on node $gpu_numa, benching on node $BENCH_NUMA — GPU DMA crosses UPI (intentional)"
    fi
    echo

    header "Memory"
    echo "  Total: ${total_mem} GiB (node 0: ${node0_mem} MB, node 1: ${node1_mem} MB)"

    # In kernel mode a plain `mem=${BENCH_MEM}` caps total RAM to just the bench
    # node (node 0 = bottom of physical memory). Accept anything at or below the
    # cap plus a little slack — the kernel loses a bit to crashkernel/reserved.
    local bench_mem_gib=${BENCH_MEM%G}
    if [[ $total_mem -le $((bench_mem_gib + 10)) ]]; then
        echo -e "  ${tag_certus}${tag_ss} RAM limited to ~${total_mem} GiB (mem=${BENCH_MEM})"
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        # cgroup mode caps RAM per-slice at runtime, not system-wide, so total
        # system RAM is expected to be full. The slice check below is authoritative.
        echo "  ${total_mem} GiB total — system RAM not capped (cgroup limits the bench slice, see below)"
    else
        echo -e "  ${tag_empty} ${total_mem} GiB — not limited (need mem=${BENCH_MEM})"
        fail_both "system RAM not limited (need mem=${BENCH_MEM})"
    fi

    # The node NOT being benched should have been dropped by the mem= cap. For the
    # default (BENCH_NUMA=0) the dropped node is node 1; benching on node 1 would
    # instead drop node 0.
    local other_mem other_node
    if [[ "$BENCH_NUMA" == "0" ]]; then other_node=1; other_mem="$node1_mem"; else other_node=0; other_mem="$node0_mem"; fi
    if [[ -n "$other_mem" && $other_mem -le 10000 ]]; then
        echo -e "  ${tag_certus}${tag_ss} node $other_node dropped (${other_mem} MB) — RAM isolated to node $BENCH_NUMA"
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo "  node $other_node has ${other_mem} MB — not dropped (cgroup mode pins to node $BENCH_NUMA via numactl)"
    elif [[ -n "$other_mem" ]]; then
        echo -e "  ${tag_empty} node $other_node has ${other_mem} MB — mem= cap not active"
        fail_both "node $other_node not dropped (${other_mem} MB, mem= cap inactive)"
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

    echo "  Hugepages: $hp_total × 1G (free: $hp_free, node $BENCH_NUMA: $hp_node)"
    # certus needs CERTUS_HUGEPAGES on the bench node; sharedstorage needs none.
    if [[ $hp_total -eq $CERTUS_HUGEPAGES && $hp_node -ge $CERTUS_HUGEPAGES ]]; then
        echo -e "  ${tag_certus} $hp_total × 1G on node $BENCH_NUMA"
        fail_ss "hugepages present ($hp_total × 1G; sharedstorage needs 0)"
    elif [[ $hp_total -eq $SS_HUGEPAGES ]]; then
        echo -e "  ${tag_ss} no hugepages — all RAM available for page cache"
        fail_certus "no hugepages (certus needs $CERTUS_HUGEPAGES × 1G)"
    else
        echo -e "  ${tag_empty} $hp_total hugepages — certus needs $CERTUS_HUGEPAGES on node $BENCH_NUMA, sharedstorage needs $SS_HUGEPAGES"
        fail_certus "hugepages misconfigured ($hp_total total, $hp_node on node $BENCH_NUMA; need $CERTUS_HUGEPAGES on node $BENCH_NUMA)"
        [[ $hp_total -ne $SS_HUGEPAGES ]] && fail_ss "hugepages present ($hp_total × 1G; sharedstorage needs 0)"
    fi
    echo

    header "Kernel (running)"

    # Full raw cmdline first. The per-param checks below normalize specific args,
    # but this is what actually booted — and it's where a stripped `$` or a wrong
    # mem= cap shows up at a glance.
    echo "  $cmdline"
    echo

    # mem= (only required in kernel mode; cgroup mode caps RAM via the slice).
    # Node 0 is isolated with a plain mem= cap — no memmap, no `$` to be eaten.
    if echo "$cmdline" | grep -q "mem=${BENCH_MEM}"; then
        echo -e "  ${tag_certus}${tag_ss} mem=${BENCH_MEM} — RAM capped to node $BENCH_NUMA"
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo "  mem= not set — not needed (cgroup slice caps RAM at runtime)"
    else
        echo -e "  ${tag_empty} mem=${BENCH_MEM} MISSING — RAM not capped to node $BENCH_NUMA"
        fail_both "mem=${BENCH_MEM} missing (kernel mode)"
    fi

    # A leftover memmap= from an older config means the `$`-trap could be in play.
    if echo "$cmdline" | grep -q "memmap="; then
        echo -e "  ${YELLOW}note:${NC} stale memmap= in cmdline — no longer used (node 0 needs only mem=)"
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

    header "NVMe Devices (NUMA node $BENCH_NUMA)"
    printf "  %-14s %-12s %-10s %-6s\n" "BDF" "Driver" "Block Dev" "Status"
    printf "  %-14s %-12s %-10s %-6s\n" "--------------" "------------" "----------" "------"
    for bdf in "${NVME_BDFS[@]}"; do
        local drv blk status
        drv=$(get_driver "$bdf")
        blk=$(get_blkdev "$bdf")
        local dev_numa
        dev_numa=$(cat "/sys/bus/pci/devices/$bdf/numa_node" 2>/dev/null || echo "?")
        if [[ "$dev_numa" != "$BENCH_NUMA" ]]; then
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
            # Verify the mount is actually XFS on md0 — not a stale/wrong fs left
            # behind (e.g. a corrupt array that certus's raw SPDK writes clobbered
            # but that still assembled + mounted). fstype must be xfs to be ready.
            local fstype src
            fstype=$(findmnt -no FSTYPE "$MOUNT_POINT" 2>/dev/null)
            src=$(findmnt -no SOURCE "$MOUNT_POINT" 2>/dev/null)
            if [[ "$fstype" == "xfs" && "$src" == "$MD_DEVICE" ]]; then
                echo -e "  ${tag_ss} $MD_DEVICE (xfs) mounted at $MOUNT_POINT"
                df -h "$MOUNT_POINT" | tail -1 | awk '{printf "  Usage: %s / %s (%s)\n", $3, $2, $5}'
            else
                echo -e "  ${tag_empty} $MOUNT_POINT mounted but not xfs-on-$MD_DEVICE (fstype=${fstype:-none}, src=${src:-none})"
                fail_ss "mount at $MOUNT_POINT is not xfs on $MD_DEVICE (fstype=${fstype:-none})"
            fi
        elif [[ -e "$MD_DEVICE" ]]; then
            echo -e "  ${tag_empty} $MD_DEVICE exists but NOT mounted"
            fail_ss "RAID $MD_DEVICE not mounted"
        else
            echo -e "  ${tag_empty} no RAID configured"
            fail_ss "no RAID configured at $MOUNT_POINT"
        fi
        echo
    fi

    # --- Certus build & runtime hazards (certus only) ---
    # These encode failure modes found the hard way: a build without the SPDK
    # hugepage path silently allocs the DRAM tier from cgroup-charged RAM; a tier
    # over the DPDK memseg cap can't allocate; a cgroup cap below vLLM's floor
    # OOMs it; stale SPDK locks / orphaned GPU procs block startup.
    header "Certus build & sizing"

    # 1. Native module built with the SPDK hugepage allocation path.
    # NB: use grep -c (not grep -q) — with `set -o pipefail`, grep -q exits on
    # first match and SIGPIPEs `strings`, making the pipeline return nonzero and
    # falsely failing the check.
    local so_path="$SCRIPT_DIR/../$CERTUS_NATIVE_SO"
    if [[ -f "$so_path" ]]; then
        local hp_path_count
        hp_path_count=$(strings "$so_path" 2>/dev/null | grep -c 'allocated from SPDK hugepages' || true)
        if [[ "$hp_path_count" -gt 0 ]]; then
            echo -e "  ${tag_certus} certus_native has SPDK hugepage alloc path"
        else
            echo -e "  ${tag_empty} certus_native MISSING SPDK hugepage path — DRAM tier will use cgroup-charged RAM"
            echo "      fix: add features=[\"spdk\"] to memory-tier dep, rebuild (maturin develop --release)"
            fail_certus "certus_native built without SPDK hugepage path (memory-tier missing spdk feature)"
        fi
    else
        echo -e "  ${YELLOW}certus_native .so not found at $CERTUS_NATIVE_SO — cannot verify build${NC}"
    fi

    # 2. A single DRAM-tier spdk_zmalloc is bounded by BOTH the DPDK memseg cap
    #    AND the pool minus DPDK's own ~3-page overhead. Report the max usable.
    local max_tier_gib=$((CERTUS_HUGEPAGES - DPDK_HUGEPAGE_OVERHEAD_GIB))
    [[ $max_tier_gib -gt $DPDK_MEMSEG_LIST_GIB ]] && max_tier_gib=$DPDK_MEMSEG_LIST_GIB
    if [[ $CERTUS_HUGEPAGES -ge $DPDK_MEMSEG_LIST_GIB ]]; then
        echo -e "  ${tag_empty} hugepage pool ${CERTUS_HUGEPAGES}G >= DPDK memseg cap ${DPDK_MEMSEG_LIST_GIB}G — raise RTE_MAX_MEM_MB_PER_LIST"
        fail_certus "hugepage pool ${CERTUS_HUGEPAGES}G at/over DPDK single-alloc ceiling ${DPDK_MEMSEG_LIST_GIB}G"
    else
        echo -e "  ${tag_certus} max single DRAM tier ≈ ${max_tier_gib}G (pool ${CERTUS_HUGEPAGES}G − ${DPDK_HUGEPAGE_OVERHEAD_GIB}G DPDK overhead; memseg cap ${DPDK_MEMSEG_LIST_GIB}G)"
        echo "      set DRAM_CACHE_BYTES <= ${max_tier_gib}G (a full-pool ${CERTUS_HUGEPAGES}G request fails — no room for DPDK heap)"
    fi

    # 3. cgroup cap clears vLLM's regular-RAM floor (cgroup mode only).
    if [[ "$MEM_METHOD" == "cgroup" ]]; then
        local certus_cap_gib=$((TOTAL_USABLE_NODE1 - CERTUS_HUGEPAGES))
        if [[ $certus_cap_gib -ge $VLLM_MIN_RAM_GIB ]]; then
            echo -e "  ${tag_certus} cgroup cap ${certus_cap_gib}G >= vLLM floor ${VLLM_MIN_RAM_GIB}G"
        else
            echo -e "  ${tag_empty} cgroup cap ${certus_cap_gib}G < vLLM floor ${VLLM_MIN_RAM_GIB}G — vLLM will be OOM-killed"
            fail_certus "cgroup cap ${certus_cap_gib}G below vLLM RAM floor ${VLLM_MIN_RAM_GIB}G"
        fi
    fi

    # 4. No stale SPDK per-device lock files (block device claim if unreadable).
    local stale_locks=()
    for bdf in "${NVME_BDFS[@]}"; do
        local lock="/var/tmp/spdk_pci_lock_${bdf}"
        [[ -e "$lock" ]] && stale_locks+=("$lock")
    done
    if [[ ${#stale_locks[@]} -eq 0 ]]; then
        echo -e "  ${tag_certus} no stale SPDK lock files"
    else
        echo -e "  ${tag_empty} ${#stale_locks[@]} stale SPDK lock file(s) — may block device claim (Permission denied)"
        echo "      fix: sudo rm -f ${stale_locks[*]}"
        fail_certus "${#stale_locks[@]} stale SPDK lock file(s) in /var/tmp"
    fi

    # 5. No orphaned GPU-holding process (leftover EngineCore starves a new run).
    if command -v nvidia-smi >/dev/null 2>&1; then
        # Count non-empty lines. Don't use `grep -c . || echo 0`: grep exits 1 on
        # zero matches, so the fallback appends a second "0" and breaks the -eq.
        local gpu_procs
        # `|| true` so a zero-match grep (exit 1) doesn't trip `set -e`.
        gpu_procs=$(nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader 2>/dev/null | grep -c '[0-9]' || true)
        gpu_procs=${gpu_procs//[!0-9]/}
        if [[ "${gpu_procs:-0}" -eq 0 ]]; then
            echo -e "  ${tag_certus}${tag_ss} GPU free (no compute processes)"
        else
            echo -e "  ${tag_empty} $gpu_procs process(es) holding GPU memory — a new run may fail to init"
            nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader 2>/dev/null | sed 's/^/      /'
            echo "      fix: sudo pkill -9 -f 'VLLM::EngineCore' (or kill the listed pids)"
            fail_both "$gpu_procs process(es) holding GPU memory"
        fi
    fi
    echo


    header "Run"
    echo "  numactl --cpunodebind=$BENCH_NUMA --membind=$BENCH_NUMA <command>"
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
    echo "    systemd-run --slice=${BENCH_SLICE} --scope numactl --cpunodebind=$BENCH_NUMA --membind=$BENCH_NUMA <command>"
    echo
    echo -e "  ${YELLOW}Note:${NC} MemoryMax caps regular RAM; run under numactl --membind=$BENCH_NUMA"
    echo "  to keep allocations on the bench node (no boot-time reservation needed)."
    echo "  Only the hugepages= boot param is required (1G pages reserved at boot)."
}

# Remove the bench cgroup slice. Used when switching to kernel mode, where the
# mem=/memmap= boot params cap RAM system-wide and a leftover MemoryMax slice
# would double-limit (and mislead the status check).
teardown_cgroup_mem() {
    command -v systemctl >/dev/null 2>&1 || return 0
    local unit="/etc/systemd/system/${BENCH_SLICE}"
    if systemctl is-active --quiet "$BENCH_SLICE" 2>/dev/null || [[ -f "$unit" ]]; then
        header "Removing cgroup slice (kernel mode caps RAM via boot params)"
        systemctl stop "$BENCH_SLICE" 2>/dev/null || true
        if [[ -f "$unit" ]]; then
            echo "  Removing $unit"
            rm -f "$unit"
            systemctl daemon-reload
        fi
        echo -e "  ${GREEN}Slice ${BENCH_SLICE} removed${NC}"
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

    header "Kernel Boot Parameters ($mode, MEM_METHOD=$MEM_METHOD)"

    # Remove old conflicting params
    local remove_args="hugepages hugepagesz default_hugepagesz mem memmap"
    echo "  Removing old params: $remove_args"
    grubby --update-kernel=ALL --remove-args="$remove_args" 2>/dev/null || true

    # Hugepages are always set via boot params (1G pages need contiguous memory
    # reserved at boot). The mem= RAM cap is only needed in kernel mode; in cgroup
    # mode the systemd slice caps RAM at runtime and numactl --membind pins
    # allocations to node $BENCH_NUMA, so the cap isn't needed at boot.
    #
    # Benching on node 0 (the default) means a plain `mem=${BENCH_MEM}` cap: node 0
    # is at physical 0, so this keeps the bottom BENCH_MEM (all node 0) and drops
    # node 1. NO memmap, NO `$` — the reserved-region form that GRUB's blscfg
    # mangles is deliberately avoided (see the BENCH_NUMA config comment).
    local new_args="default_hugepagesz=1G hugepagesz=1G hugepages=${hugepages}"
    if [[ "$MEM_METHOD" == "kernel" ]]; then
        new_args="$new_args mem=${BENCH_MEM}"
    fi
    echo "  Setting: $new_args"
    grubby --update-kernel=ALL --args="$new_args"

    # Verify
    echo
    echo "  Next boot kernel: $(grubby --default-kernel)"
    echo "  Effective kernel args for next boot:"
    grubby --info=DEFAULT | grep ^args | sed 's/^args="/  /' | sed 's/"$//' | sed 's/\\\$/$/g'

    # Guard against the `$`-in-BLS trap ever regressing: if any stored kernel arg
    # carries a `memmap=…$…`, GRUB's blscfg will strip the `$offset` at boot and
    # the reservation silently won't apply (the exact bug that put node 1 at 0 MB).
    local stored_args
    stored_args=$(grubby --info=DEFAULT 2>/dev/null | grep ^args=)
    if echo "$stored_args" | grep -qE 'memmap=[^ "]*\$'; then
        echo
        echo -e "  ${RED}WARNING:${NC} a memmap=…\$… arg is present. GRUB's blscfg expands"
        echo -e "  ${RED}${NC}         \$ on the BLS options= line and will STRIP the offset at"
        echo -e "  ${RED}${NC}         boot → reservation won't apply. Use BENCH_NUMA=0 (mem= only)."
    fi

    # Check if reboot needed
    local current_cmdline mem_ok=true
    current_cmdline=$(cat /proc/cmdline)
    if [[ "$MEM_METHOD" == "kernel" ]] && ! echo "$current_cmdline" | grep -q "mem=${BENCH_MEM}"; then
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
    local node_path="/sys/devices/system/node/node${BENCH_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages"

    header "Hugepages (node $BENCH_NUMA)"

    if [[ ! -f "$node_path" ]]; then
        echo -e "  ${YELLOW}1G hugepage support not available at runtime${NC}"
        echo "  Will be allocated at next boot from node $BENCH_NUMA (hugepages= boot param)"
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
    if [[ $current -eq $target ]]; then
        echo "  Node $BENCH_NUMA already has $current × 1G hugepages"
        return
    elif [[ $current -gt $target ]]; then
        # Over-allocated (e.g. target lowered) — shrink to exactly target so the
        # regular-RAM budget grows accordingly. Free pages release live.
        echo "  Node $BENCH_NUMA has $current × 1G, reducing to $target..."
    else
        echo "  Allocating $target × 1G hugepages on node $BENCH_NUMA..."
    fi
    echo "$target" > "$node_path"

    local actual
    actual=$(cat "$node_path")
    if [[ $actual -lt $target ]]; then
        echo -e "  ${YELLOW}Only got $actual / $target — 1G pages require contiguous memory${NC}"
        echo "  Reboot required for full allocation (boot param handles it)."
    else
        echo -e "  ${GREEN}Allocated $actual × 1G hugepages on node $BENCH_NUMA${NC}"
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

# Remove stale SPDK per-device lock files. SPDK writes /var/tmp/spdk_pci_lock_<bdf>
# (root-owned, 0600) while it holds a device; if a run dies without releasing them
# the files linger and the next certus startup gets Permission denied claiming the
# device. Safe to remove when no SPDK process is running.
free_spdk_locks() {
    local locks=()
    for bdf in "${NVME_BDFS[@]}"; do
        local lock="/var/tmp/spdk_pci_lock_${bdf}"
        [[ -e "$lock" ]] && locks+=("$lock")
    done
    if [[ ${#locks[@]} -eq 0 ]]; then
        echo "  No stale SPDK lock files"
    else
        echo "  Removing ${#locks[@]} stale SPDK lock file(s): ${locks[*]}"
        rm -f "${locks[@]}"
    fi
}

bind_to_vfio() {
    header "Binding NVMe to vfio-pci (persistent)"

    if ! modprobe vfio-pci; then
        echo -e "  ${RED}Failed to load vfio-pci module. Is IOMMU enabled?${NC}" >&2
        exit 1
    fi

    free_spdk_locks

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

    # ALWAYS recreate the array + filesystem. These drives may have last been
    # used by certus, where SPDK binds them to vfio-pci and writes RAW block I/O
    # — clobbering the mdadm superblocks and XFS metadata. A leftover superblock
    # can still trick `mdadm --assemble` into bringing up a CORRUPT array that
    # mounts but silently loses/misreads data. So we never reuse: tear down, wipe
    # every member's signatures, create fresh, and reformat.
    teardown_raid_if_active

    echo -e "  ${YELLOW}Recreating RAID0 — this DESTROYS all data on ${blkdevs[*]}${NC}"
    for dev in "${blkdevs[@]}"; do
        mdadm --zero-superblock "$dev" 2>/dev/null || true  # drop stale md metadata
        wipefs -a "$dev" >/dev/null 2>&1 || true            # drop fs/partition signatures
    done

    mdadm --create "$MD_DEVICE" \
        --level=0 \
        --raid-devices=${#blkdevs[@]} \
        --chunk=512K \
        --run \
        --force \
        "${blkdevs[@]}"
    echo "  Created $MD_DEVICE (RAID0, 512K chunks, ${#blkdevs[@]} devices)"

    # Format with XFS
    echo "  Formatting with XFS (label=$XFS_LABEL)..."
    mkfs.xfs -f -L "$XFS_LABEL" "$MD_DEVICE"

    # Mount
    mkdir -p "$MOUNT_POINT"
    if ! mountpoint -q "$MOUNT_POINT"; then
        mount "$MD_DEVICE" "$MOUNT_POINT"
        echo "  Mounted $MD_DEVICE at $MOUNT_POINT"
    else
        echo "  Already mounted at $MOUNT_POINT"
    fi

    # Fresh mkfs means the volume is empty; create the KV directory. The bench
    # workload runs as the invoking (non-root) user, so hand ownership to them —
    # otherwise the root-owned dir is unwritable and the run fails at startup.
    mkdir -p "$MOUNT_POINT/shared-kv"
    local owner="${SUDO_USER:-root}"
    chown -R "$owner": "$MOUNT_POINT/shared-kv" 2>/dev/null || true
    echo "  KV dir $MOUNT_POINT/shared-kv owned by $owner"

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
    echo "  Bench NUMA node: $BENCH_NUMA   (GPU is physically on node $GPU_NUMA)"
    echo "  NVMe (node $BENCH_NUMA): ${NVME_BDFS[*]}"
    echo "  CPUs (node $BENCH_NUMA): $NUMA_CPUS"
    echo "  Memory budget:   ${TOTAL_USABLE_NODE1} GiB (mem=${BENCH_MEM}) on node $BENCH_NUMA"
    echo
    echo "Env overrides: BENCH_NUMA (default 0), BENCH_MEM (default 64G), MEM_METHOD (cgroup|kernel)"
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
        echo "  Memory limit: kernel mem=${BENCH_MEM} (reboot required)"
    fi
    echo "  Memory: ${TOTAL_USABLE_NODE1} GiB on NUMA node $BENCH_NUMA"
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
    #     In kernel mode, tear down any leftover slice so it doesn't double-limit.
    if [[ "$MEM_METHOD" == "cgroup" ]]; then
        setup_cgroup_mem "$mode"
    else
        teardown_cgroup_mem
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
        echo "      numactl --cpunodebind=$BENCH_NUMA --membind=$BENCH_NUMA <command>"
        echo "    (the slice enforces MemoryMax=$(cgroup_mem_max_for "$mode"))"
    else
        echo "    numactl --cpunodebind=$BENCH_NUMA --membind=$BENCH_NUMA <command>"
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
