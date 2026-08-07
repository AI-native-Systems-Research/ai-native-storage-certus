#!/bin/bash
#
# configure-bench.sh — Configure system for Certus or SharedStorage benchmarks.
#
# Pins the workload's resources (NVMe, memory, CPUs, hugepages) to one NUMA node
# (RESOURCE_NUMA). On THIS host that is node 0 (drives 61-64); the GPU is on node
# 1, so GPU access is cross-node by design. Sets kernel boot parameters via
# grubby and configures NVMe devices at runtime (vfio-pci for Certus, RAID0+XFS
# for SharedStorage).
#
# Defaults are tuned for this host: RESOURCE_NUMA=0, drives 61-64, 16×1G
# hugepages, mem=32G via the kernel method (no cgroup). Override any of them with
# env vars (RESOURCE_NUMA, GPU_NUMA, NVME_BDFS, CERTUS_HUGEPAGES, MEM_LIMIT,
# NUMA_CPUS, MEM_METHOD) if the topology changes.
#
# Memory can be limited two ways (MEM_METHOD env var):
#   kernel  (default) — cap RAM via mem= boot param (memmap= too if the resource
#                       node is the HIGH node); mem=32G is already in this kernel.
#   cgroup            — cap RAM at runtime via a systemd slice; no reboot.
#
# Usage:
#   sudo ./configure-bench.sh certus                     # Certus (node 0, 16×1G, mem=32G)
#   sudo ./configure-bench.sh sharedstorage              # SharedStorage (RAID0+XFS)
#   sudo MEM_METHOD=cgroup ./configure-bench.sh certus   # Certus, cgroup mem limit (no reboot)
#   sudo NVME_BDFS="0000:c1:00.0 ..." ./configure-bench.sh certus  # override drive set
#   sudo ./configure-bench.sh status                     # Show current configuration
#
set -euo pipefail

# Directory this script lives in (tools/), used to locate the built .so etc.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ============================================================================
# Configuration — edit these if hardware changes
# ============================================================================

# NUMA node holding the workload's resources (NVMe drives, hugepages, CPUs, RAM).
# On THIS host that is node 0 — the certus vfio drives (61-64) live there and
# `mem=32G` keeps all usable RAM in node 0's low address range. Overridable via
# the RESOURCE_NUMA env var if the topology changes.
RESOURCE_NUMA="${RESOURCE_NUMA:-0}"

# NUMA node the GPU (a1:00.0) sits on — used ONLY for the GPU-location sanity
# check, NOT for pinning. On this host the GPU is on node 1 while the drives are
# on node 0, so this deliberately differs from RESOURCE_NUMA.
GPU_NUMA="${GPU_NUMA:-1}"

# NVMe devices making up the workload storage (node $RESOURCE_NUMA). Default is
# the node-0 set (61-64); override with the NVME_BDFS env var for the node-1 set.
NVME_BDFS=(${NVME_BDFS:-"0000:61:00.0" "0000:62:00.0" "0000:63:00.0" "0000:64:00.0"})

# CPUs on the resource NUMA node ($RESOURCE_NUMA). Node 0 = 0-15,32-47.
NUMA_CPUS="${NUMA_CPUS:-0-15,32-47}"

# RAID / filesystem. Env-overridable so the SharedStorage RAID can target a
# device/mount/label distinct from any pre-existing array (e.g. a separate
# model-fs RAID that already occupies /dev/md0 // /mnt/fs-backend-bench). The
# certus-mode teardown (bind_to_vfio -> teardown_raid_if_active) acts on these
# same vars, so overriding them keeps certus from tearing down an unrelated array.
MD_DEVICE="${MD_DEVICE:-/dev/md0}"
MOUNT_POINT="${MOUNT_POINT:-/mnt/fs-backend-bench}"
XFS_LABEL="${XFS_LABEL:-fs-bench}"

# Memory cap. `mem=32G` truncates total physical RAM to 32 GiB; because node 0
# occupies the low addresses, all 32G lands on node 0 and node 1 gets 0 MB — so
# no memmap reservation is needed when resources are already on node 0. (If the
# resource node were the HIGH one, NODE0_RESERVE would carve out node 0 via
# memmap; leave it empty for the node-0 layout.)
NODE0_RESERVE=''
MEM_LIMIT="${MEM_LIMIT:-32G}"
TOTAL_USABLE_GIB="${TOTAL_USABLE_GIB:-32}"  # GiB usable on node $RESOURCE_NUMA after mem=

# Hugepages (1 GiB pages)
CERTUS_HUGEPAGES="${CERTUS_HUGEPAGES:-16}"  # 16 GiB SPDK DRAM tier on node $RESOURCE_NUMA
# SharedStorage needs no boot-reserved hugepages (all RAM -> page cache), so the
# default is 0. Overridable: when SharedStorage runs in the same invocation as
# Certus-SPDK, the orchestrator sets this to CERTUS_HUGEPAGES so this phase does
# not clobber the boot reservation Certus-SPDK requires (its runtime pages are
# still released via free_all_hugepages, so page cache is unaffected this run).
SS_HUGEPAGES="${SS_HUGEPAGES:-0}"

# DPDK single-allocation ceiling. spdk_zmalloc -> rte_malloc cannot return a
# block spanning more than one memseg list (RTE_MAX_MEM_MB_PER_LIST), so a
# single Certus DRAM tier is hard-capped just under this regardless of how many
# hugepages exist. Raised from the stock 32 GiB to 64 GiB via a patched
# deps/spdk/dpdk/config/rte_config.h (rebuilt with deps/build_spdk.sh) to allow
# a 48 GiB tier. Keep CERTUS_HUGEPAGES below this. NOTE: also bounded by
# RTE_MAX_MEM_MB_PER_TYPE=64 GiB, so the practical single-tier max is ~60 GiB.
DPDK_MEMSEG_LIST_GIB=64

# Minimum regular RAM vLLM needs to init + generate (empirical: OOMs below ~16G
# with an 8B model). The certus cgroup cap must clear this or vLLM is OOM-killed.
VLLM_MIN_RAM_GIB=16

# DPDK reserves ~3 x 1G hugepages for its own EAL heap + per-drive DMA buffers
# (measured: 48-page pool - 44G tier = 4 free before drives, 1 after). So a
# single DRAM-tier spdk_zmalloc maxes at ~(CERTUS_HUGEPAGES - this) GiB. The run
# script's dram_cache_bytes must stay under that, not the full pool.
DPDK_HUGEPAGE_OVERHEAD_GIB=3

# Built native module whose allocation path must include SPDK hugepage support.
CERTUS_NATIVE_SO="certus-connector/certus_native/certus_native.cpython-312-x86_64-linux-gnu.so"

# Memory limiting method:
#   kernel  — boot-param cap via mem=/memmap= (default on this host; needs reboot
#             to change, but mem=32G is already in the running kernel)
#   cgroup  — runtime cap via a systemd slice (no reboot)
# This host uses the kernel method: `mem=32G` bounds RAM and, because node 0 owns
# the low addresses, keeps it all on the resource node — no cgroup slice needed.
MEM_METHOD="${MEM_METHOD:-kernel}"

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
        echo -e "${RED}error: this script must be run as root (use sudo).${NC}" >&2
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
    if [[ -f /sys/devices/system/node/node${RESOURCE_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages ]]; then
        hp_node=$(cat /sys/devices/system/node/node${RESOURCE_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages)
    fi

    local total_mem node0_mem node1_mem
    total_mem=$(free -g | awk '/Mem:/{print $2}')
    node0_mem=$(numactl -H 2>/dev/null | grep "node 0 size" | awk '{print $4}')
    node1_mem=$(numactl -H 2>/dev/null | grep "node 1 size" | awk '{print $4}')

    # --- Display checks ---
    # Each check is tagged with which connector(s) it currently SATISFIES.
    # Empty [] = satisfies neither (problem). Colored tag = meets that connector's requirement.

    header "GPU / resource NUMA nodes"
    local gpu_numa
    gpu_numa=$(cat /sys/bus/pci/devices/0000:a1:00.0/numa_node 2>/dev/null || echo "?")
    if [[ "$gpu_numa" == "$GPU_NUMA" ]]; then
        echo -e "  ${tag_certus}${tag_ss} GPU on NUMA node $gpu_numa (expected $GPU_NUMA)"
    else
        echo -e "  ${tag_empty} GPU on NUMA node $gpu_numa, expected $GPU_NUMA"
        fail_both "GPU on NUMA node $gpu_numa (expected $GPU_NUMA)"
    fi
    if [[ "$RESOURCE_NUMA" != "$GPU_NUMA" ]]; then
        echo "  Resources (NVMe/hugepages/CPUs/RAM) pinned to node $RESOURCE_NUMA; GPU on node $GPU_NUMA (cross-node by design on this host)"
    else
        echo "  Resources pinned to node $RESOURCE_NUMA (same as GPU)"
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
    # Node-0 reservation only matters when the resource node is the HIGH node and
    # node 0 must be emptied (NODE0_RESERVE set). On the node-0 resource layout,
    # node 0 is SUPPOSED to hold the RAM, so its memory is expected, not a fault.
    if [[ -z "$NODE0_RESERVE" ]]; then
        echo "  node 0 holds ${node0_mem:-?} MB — expected (node 0 is the resource node)"
    elif [[ -n "$node0_mem" && $node0_mem -le 10000 ]]; then
        echo -e "  ${tag_certus}${tag_ss} node 0 reserved (${node0_mem} MB)"
    elif [[ -n "$node0_mem" && $node0_mem -gt 10000 && "$MEM_METHOD" == "cgroup" ]]; then
        # cgroup mode doesn't reserve node 0 — allocations are pinned to the
        # resource node via numactl --membind, so node 0 having memory is fine.
        echo "  node 0 has ${node0_mem} MB — not reserved (cgroup mode pins to node $RESOURCE_NUMA via numactl)"
    elif [[ -n "$node0_mem" && $node0_mem -gt 10000 ]]; then
        echo -e "  ${tag_empty} node 0 has ${node0_mem} MB — memmap reservation not active"
        fail_both "node 0 not reserved (${node0_mem} MB, memmap inactive)"
    fi

    # cgroup memory limit (runtime). The cap is mode-specific: certus expects the
    # regular-RAM budget left after hugepages, sharedstorage the full node budget.
    local certus_gib=$((TOTAL_USABLE_GIB - CERTUS_HUGEPAGES))
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
            if [[ $slice_gib -eq $TOTAL_USABLE_GIB ]]; then mem_tag="${mem_tag}${tag_ss}"; fi
            if [[ -z "$mem_tag" ]]; then mem_tag="${tag_empty}"; fi
            echo -e "  ${mem_tag} cgroup slice ${BENCH_SLICE}: MemoryMax=${slice_gib} GiB (runtime limit)"
            # In cgroup mode, a cap that doesn't match a connector's budget disqualifies it.
            if [[ "$MEM_METHOD" == "cgroup" ]]; then
                [[ $slice_gib -ne $certus_gib ]] && fail_certus "cgroup cap ${slice_gib}G ≠ certus budget ${certus_gib}G"
                [[ $slice_gib -ne $TOTAL_USABLE_GIB ]] && fail_ss "cgroup cap ${slice_gib}G ≠ sharedstorage budget ${TOTAL_USABLE_GIB}G"
            fi
        fi
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo -e "  ${tag_empty} cgroup slice ${BENCH_SLICE} not active — run 'sudo $0 <mode>' to create it"
        fail_both "cgroup slice ${BENCH_SLICE} not active"
    else
        echo "  cgroup slice ${BENCH_SLICE}: not active (MEM_METHOD=kernel)"
    fi

    echo "  Hugepages: $hp_total × 1G (free: $hp_free, node $RESOURCE_NUMA: $hp_node)"
    # certus needs CERTUS_HUGEPAGES on the GPU node; sharedstorage needs none.
    if [[ $hp_total -eq $CERTUS_HUGEPAGES && $hp_node -ge $CERTUS_HUGEPAGES ]]; then
        echo -e "  ${tag_certus} $hp_total × 1G on node $RESOURCE_NUMA"
        fail_ss "hugepages present ($hp_total × 1G; sharedstorage needs 0)"
    elif [[ $hp_total -eq $SS_HUGEPAGES ]]; then
        echo -e "  ${tag_ss} no hugepages — all RAM available for page cache"
        fail_certus "no hugepages (certus needs $CERTUS_HUGEPAGES × 1G)"
    else
        echo -e "  ${tag_empty} $hp_total hugepages — certus needs $CERTUS_HUGEPAGES on node $RESOURCE_NUMA, sharedstorage needs $SS_HUGEPAGES"
        fail_certus "hugepages misconfigured ($hp_total total, $hp_node on node $RESOURCE_NUMA; need $CERTUS_HUGEPAGES on node $RESOURCE_NUMA)"
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

    # memmap= — only needed when the resource node is the HIGH node and node 0
    # must be carved out. When resources are on node 0 (NODE0_RESERVE empty),
    # mem= alone keeps all RAM on node 0, so no memmap is required or expected.
    # The kernel normalizes the offset to hex in /proc/cmdline, so match either form.
    if [[ -z "$NODE0_RESERVE" ]]; then
        echo "  memmap= not needed (resources on node $RESOURCE_NUMA; mem=${MEM_LIMIT} keeps RAM there)"
    elif echo "$cmdline" | grep -qE 'memmap=254G\$(2G|0x80000000)'; then
        echo -e "  ${tag_certus}${tag_ss} memmap=${NODE0_RESERVE} — node 0 reserved"
    elif [[ "$MEM_METHOD" == "cgroup" ]]; then
        echo "  memmap= not set — not needed (cgroup mode pins to node $RESOURCE_NUMA via numactl)"
    elif echo "$cmdline" | grep -q "memmap="; then
        echo -e "  ${tag_empty} memmap= present but OFFSET MISSING — reservation not active"
        fail_both "memmap= offset missing (kernel mode)"
    else
        echo -e "  ${tag_empty} memmap= MISSING — memory not isolated to NUMA node $RESOURCE_NUMA"
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

    header "NVMe Devices (NUMA node $RESOURCE_NUMA)"
    printf "  %-14s %-12s %-10s %-6s\n" "BDF" "Driver" "Block Dev" "Status"
    printf "  %-14s %-12s %-10s %-6s\n" "--------------" "------------" "----------" "------"
    for bdf in "${NVME_BDFS[@]}"; do
        local drv blk status
        drv=$(get_driver "$bdf")
        blk=$(get_blkdev "$bdf")
        local dev_numa
        dev_numa=$(cat "/sys/bus/pci/devices/$bdf/numa_node" 2>/dev/null || echo "?")
        if [[ "$dev_numa" != "$RESOURCE_NUMA" ]]; then
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
        local certus_cap_gib=$((TOTAL_USABLE_GIB - CERTUS_HUGEPAGES))
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
        echo "      fix: sudo rm -f /var/tmp/spdk_pci_lock_0000:c*"
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
    echo "  numactl --cpunodebind=$RESOURCE_NUMA --membind=$RESOURCE_NUMA <command>"
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
        echo "$((TOTAL_USABLE_GIB - CERTUS_HUGEPAGES))G"
    else
        echo "${TOTAL_USABLE_GIB}G"
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
    echo "    systemd-run --slice=${BENCH_SLICE} --scope numactl --cpunodebind=$RESOURCE_NUMA --membind=$RESOURCE_NUMA <command>"
    echo
    echo -e "  ${YELLOW}Note:${NC} MemoryMax caps regular RAM; run under numactl --membind=$RESOURCE_NUMA"
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
    # and numactl --membind pins allocations to node $RESOURCE_NUMA, so node 0 doesn't
    # need reserving.
    local new_args="default_hugepagesz=1G hugepagesz=1G hugepages=${hugepages}"
    if [[ "$MEM_METHOD" == "kernel" ]]; then
        new_args="$new_args mem=${MEM_LIMIT}"
        # memmap only when carving node 0 out of a HIGH resource node; empty on
        # the node-0 layout where mem= alone keeps all RAM on the resource node.
        [[ -n "$NODE0_RESERVE" ]] && new_args="$new_args memmap=${NODE0_RESERVE}"
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
    local node_path="/sys/devices/system/node/node${RESOURCE_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages"

    header "Hugepages (node $RESOURCE_NUMA)"

    if [[ ! -f "$node_path" ]]; then
        echo -e "  ${YELLOW}1G hugepage support not available at runtime${NC}"
        echo "  Will be allocated at next boot from node $RESOURCE_NUMA (memmap reserves node 0)"
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
        echo "  Node $RESOURCE_NUMA already has $current × 1G hugepages"
        return
    elif [[ $current -gt $target ]]; then
        # Over-allocated (e.g. target lowered) — shrink to exactly target so the
        # regular-RAM budget grows accordingly. Free pages release live.
        echo "  Node $RESOURCE_NUMA has $current × 1G, reducing to $target..."
    else
        echo "  Allocating $target × 1G hugepages on node $RESOURCE_NUMA..."
    fi
    echo "$target" > "$node_path"

    local actual
    actual=$(cat "$node_path")
    if [[ $actual -lt $target ]]; then
        echo -e "  ${YELLOW}Only got $actual / $target — 1G pages require contiguous memory${NC}"
        echo "  Reboot required for full allocation (boot param handles it)."
    else
        echo -e "  ${GREEN}Allocated $actual × 1G hugepages on node $RESOURCE_NUMA${NC}"
    fi

    # The certus-server (SPDK/DPDK) runs as the invoking user, NOT root — its uid
    # must match the rootless container's vLLM process for CUDA IPC. DPDK creates a
    # per-segment file under the hugetlbfs mount, so that mount has to be writable
    # by that user or EAL dies with "get_seg_fd(): ... Permission denied". The vfio
    # nodes are already opened to the user in bind_to_vfio; do the same for the
    # hugepage dir here (root:root by default).
    local hp_owner="${SUDO_USER:-}"
    if [[ -n "$hp_owner" && "$hp_owner" != "root" ]]; then
        local hp_mnt
        hp_mnt=$(awk '$3=="hugetlbfs" && $4 ~ /pagesize=1024M/ {print $2; exit}' /proc/mounts)
        [[ -z "$hp_mnt" ]] && hp_mnt=$(awk '$3=="hugetlbfs" {print $2; exit}' /proc/mounts)
        if [[ -n "$hp_mnt" ]]; then
            chown "$hp_owner" "$hp_mnt" \
                && echo "  Owner of $hp_mnt: $hp_owner (SPDK runs as this user)"
        fi
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
            # Wipe stale partition-table / fs signatures first: otherwise mdadm
            # detects them and STOPS at an interactive "partition table exists ...
            # Continue creating array [y/N]?" prompt. profile_all.sh redirects this
            # command's output to a log, so that prompt is invisible and the whole
            # run looks hung. wipefs removes the trigger; the `<<<"y"` here-string is
            # a harmless fallback that auto-confirms any residual prompt.
            # NB: do NOT pipe `yes |` here — under `set -o pipefail`, `yes` dies with
            # SIGPIPE (141) when mdadm closes the pipe, and that non-zero propagates
            # through the pipeline, tripping `set -e` right after the array starts
            # (before mkfs/mount). A here-string has no such pipe.
            # Reclaim the member drives from any STRAY array first. A prior failed
            # run can leave an array assembled on these drives under a DIFFERENT name
            # (the upstream picker chooses the lowest free /dev/mdN, so md1 left over
            # -> this run targets md2). teardown_raid_if_active only stops $MD_DEVICE,
            # so it misses the stray, and mdadm --create then fails "Device or
            # resource busy". Stop whatever md device currently holds each member.
            for _d in "${blkdevs[@]}"; do
                for _h in "/sys/block/$(basename "$_d")/holders/"md*; do
                    [[ -e "$_h" ]] || continue
                    _md="/dev/$(basename "$_h")"
                    echo "  Reclaiming ${_d} from stray array ${_md}"
                    umount "$_md" 2>/dev/null || true
                    mdadm --stop "$_md" 2>/dev/null || true
                done
            done
            # Now clear signatures on the (freed) members: wipefs for partition/fs
            # signatures, --zero-superblock for any residual md metadata.
            for _d in "${blkdevs[@]}"; do
                wipefs -a "$_d" 2>/dev/null || true
                mdadm --zero-superblock "$_d" 2>/dev/null || true
            done
            mdadm --create "$MD_DEVICE" \
                --level=0 \
                --raid-devices=${#blkdevs[@]} \
                --chunk=512K \
                "${blkdevs[@]}" <<<"y"
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

    # The KV backend writes here, and podman's :z relabel runs, as the invoking
    # (rootless) user. A root-owned mount blocks both the writes and the
    # lsetxattr relabel (EPERM). Hand ownership to that user.
    local owner="${SUDO_USER:-$(id -un)}"
    if [[ "$owner" != "root" ]]; then
        chown -R "$owner" "$MOUNT_POINT"
        echo "  Owner: $owner"
    fi

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
    echo "  certus         ${CERTUS_HUGEPAGES}×1G hugepages, NVMe → vfio-pci (SPDK)"
    echo "  sharedstorage  0 hugepages, NVMe → RAID0 + XFS"
    echo "  status         Show current configuration"
    echo
    echo "System topology:"
    echo "  Resource NUMA node: $RESOURCE_NUMA"
    echo "  GPU NUMA node:      $GPU_NUMA"
    echo "  NVMe (node $RESOURCE_NUMA):     ${NVME_BDFS[*]}"
    echo "  CPUs (node $RESOURCE_NUMA):     $NUMA_CPUS"
    echo "  Memory budget:      ${TOTAL_USABLE_GIB} GiB from node $RESOURCE_NUMA"
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
    echo "  Memory: ${TOTAL_USABLE_GIB} GiB on NUMA node $RESOURCE_NUMA"
    if [[ "$mode" == "certus" ]]; then
        echo "  Hugepages: ${CERTUS_HUGEPAGES} × 1G (SPDK DRAM tier)"
        echo "  Regular mem: $((TOTAL_USABLE_GIB - CERTUS_HUGEPAGES)) GiB"
        echo "  NVMe: vfio-pci (SPDK userspace)"
    else
        echo "  Hugepages: 0"
        echo "  Regular mem: ${TOTAL_USABLE_GIB} GiB (page cache)"
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
        echo "      numactl --cpunodebind=$RESOURCE_NUMA --membind=$RESOURCE_NUMA <command>"
        echo "    (the slice enforces MemoryMax=$(cgroup_mem_max_for "$mode"))"
    else
        echo "    numactl --cpunodebind=$RESOURCE_NUMA --membind=$RESOURCE_NUMA <command>"
    fi
    echo
    if [[ "$mode" == "certus" ]]; then
        echo "  Certus server should use:"
        echo "    --pci-allowlist ${NVME_BDFS[0]},${NVME_BDFS[1]},${NVME_BDFS[2]},${NVME_BDFS[3]}"
        echo "    --memory-tier-size $((CERTUS_HUGEPAGES - DPDK_HUGEPAGE_OVERHEAD_GIB))G"
    else
        echo "  SharedStorage KV path:"
        echo "    $MOUNT_POINT/shared-kv"
    fi
    echo
}

main "$@"
