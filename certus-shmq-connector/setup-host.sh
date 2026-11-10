#!/bin/bash
# setup-host.sh — one-time host provisioning for the certus-shmq-connector
# benchmark (RHEL/Fedora, root).
#
# Host prerequisites (GPU passthrough, NVMe→vfio, hugepages) for running the
# certus-server storage backend; these are transport-independent. The closing
# "Next:" hint launches certus-server with --shm-path.
#
# Covers BOTH host prerequisites that a container cannot provide:
#   A. GPU  — nvidia-container-toolkit + CDI spec (so podman can pass the GPU
#             and inject the host libcuda.so into the workload container).
#   B. Server storage — bind the certus-server NVMe drives to vfio-pci and
#             allocate 1 GiB hugepages for the SPDK DRAM tier.
#
# The reusable repo script tools/configure-bench.sh does the broader NUMA/cgroup
# bench setup but is hardcoded to the c1-c4 drives; this script targets the
# server's own drive set (default 0000:61-64:00.0, NUMA 0) so it doesn't touch
# the c1-c4 drives (which hold filesystems / the podman image store here).
#
# Usage:
#   sudo ./setup-host.sh                       # default: hugepages = physical RAM - 16G
#   sudo MEM_TOTAL_GIB=24 ./setup-host.sh      # under mem=24G cap -> 8 hugepages
#   sudo VLLM_RESERVE_GIB=16 MEM_TOTAL_GIB=64 ./setup-host.sh   # explicit split
#   sudo HUGEPAGES_1G=8 ./setup-host.sh        # override the count directly
#   sudo SKIP_DRIVES=1 ./setup-host.sh         # GPU only
#   sudo NVME_BDFS="0000:61:00.0 0000:62:00.0" ./setup-host.sh  # custom drives
#
# Idempotent: skips installs/binds already in place; re-generates the CDI spec.
set -euo pipefail

# ── Config (override via env) ──
# Server NVMe drives — the NUMA-0 set (0000:61-64), NOT the c1-c4 filesystem
# drives. NUMA 0 is deliberate: all GPU<->SSD transfers stage through the DRAM
# tier (GPU->DRAM->SSD, never direct), and the tier's hugepages must sit on the
# node whose RAM we cap with the mem= kernel param — node 0 in this bench setup.
# So drive/GPU NUMA locality is irrelevant; tier-on-node-0 is what matters.
NVME_BDFS="${NVME_BDFS:-0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0}"
# NUMA node for the hugepage pool that feeds the SPDK DRAM tier (the mem=-capped
# node). Keep at 0 unless the drives and cap node change together.
NVME_NUMA="${NVME_NUMA:-0}"

# Memory split (mirrors tools/configure-bench.sh):
#   hugepages(1G) = MEM_TOTAL_GIB - VLLM_RESERVE_GIB
# i.e. reserve regular RAM for vLLM to init/run, give ALL the rest to the SPDK
# DRAM cache as 1 GiB hugepages. The usable tier is then ~(hugepages - 3) GiB
# (DPDK EAL/DMA overhead).
#
# MEM_TOTAL_GIB default = the PHYSICAL RAM installed in the box (dmidecode),
# i.e. what's available with no mem= cap. When you DO run under a mem= cap (as
# the bench does), pass MEM_TOTAL_GIB=<cap> explicitly so the tier is sized to
# the capped, kernel-visible RAM rather than the full DIMM total.
VLLM_RESERVE_GIB="${VLLM_RESERVE_GIB:-16}"
# Physical total from dmidecode (sum populated DIMMs); fall back to node meminfo.
_phys_gib=$(dmidecode -t memory 2>/dev/null | awk '
    /Size:/ && $2 ~ /^[0-9]+$/ { v=$2; if ($3=="MB") v=v/1024; else if ($3=="TB") v=v*1024; sum+=v }
    END { printf "%d", sum }')
if [[ -z "${_phys_gib}" || "${_phys_gib}" -eq 0 ]]; then
    _node_kb=$(awk '/MemTotal/{print $4}' "/sys/devices/system/node/node${NVME_NUMA}/meminfo" 2>/dev/null)
    _phys_gib=$(( ${_node_kb:-0} / 1024 / 1024 ))
fi
MEM_TOTAL_GIB="${MEM_TOTAL_GIB:-${_phys_gib}}"
# Allow explicit HUGEPAGES_1G override; otherwise derive from the memory split.
if [[ -z "${HUGEPAGES_1G:-}" ]]; then
    HUGEPAGES_1G=$(( MEM_TOTAL_GIB - VLLM_RESERVE_GIB ))
    [[ ${HUGEPAGES_1G} -lt 0 ]] && HUGEPAGES_1G=0
fi
SKIP_DRIVES="${SKIP_DRIVES:-0}"
SKIP_GPU="${SKIP_GPU:-0}"

if [[ ${HUGEPAGES_1G} -le 0 ]]; then
    echo "error: computed HUGEPAGES_1G=${HUGEPAGES_1G} (MEM_TOTAL_GIB=${MEM_TOTAL_GIB}," \
         "VLLM_RESERVE_GIB=${VLLM_RESERVE_GIB}). Set MEM_TOTAL_GIB to the RAM on" \
         "node ${NVME_NUMA}, or HUGEPAGES_1G directly." >&2
    exit 1
fi

if [[ $EUID -ne 0 ]]; then
    echo "error: must run as root (sudo ./setup-host.sh)" >&2
    exit 1
fi

# ════════════════════════════ A. GPU ════════════════════════════
if [[ "${SKIP_GPU}" != "1" ]]; then
    echo "== A1. NVIDIA driver present? =="
    if ! command -v nvidia-smi >/dev/null 2>&1 || ! nvidia-smi -L >/dev/null 2>&1; then
        echo "error: nvidia-smi not working — install/enable the NVIDIA GPU driver first." >&2
        exit 1
    fi
    nvidia-smi -L

    echo "== A2. Install nvidia-container-toolkit (dnf) =="
    if command -v nvidia-ctk >/dev/null 2>&1; then
        echo "  nvidia-ctk already installed ($(nvidia-ctk --version 2>/dev/null | head -1))"
    else
        curl -s -L https://nvidia.github.io/libnvidia-container/stable/rpm/nvidia-container-toolkit.repo \
            -o /etc/yum.repos.d/nvidia-container-toolkit.repo
        dnf install -y nvidia-container-toolkit
    fi

    echo "== A3. Generate CDI spec for podman (rootless uses CDI) =="
    mkdir -p /etc/cdi
    nvidia-ctk cdi generate --output=/etc/cdi/nvidia.yaml
    echo "  wrote /etc/cdi/nvidia.yaml"
    nvidia-ctk cdi list 2>/dev/null | grep -E "nvidia.com/gpu" \
        || echo "  warning: no nvidia.com/gpu devices listed — check the CDI spec." >&2
fi

# ═══════════════════════ B. Server storage ═══════════════════════
if [[ "${SKIP_DRIVES}" != "1" ]]; then
    echo "== B1. Bind NVMe drives to vfio-pci =="
    if ! modprobe vfio-pci; then
        echo "error: cannot load vfio-pci — is IOMMU enabled (iommu=pt on the kernel cmdline)?" >&2
        exit 1
    fi
    for bdf in ${NVME_BDFS}; do
        dev_path="/sys/bus/pci/devices/${bdf}"
        if [[ ! -d "${dev_path}" ]]; then
            echo "  ${bdf}: NOT PRESENT — skipping" >&2
            continue
        fi
        drv=$(basename "$(readlink -f "${dev_path}/driver" 2>/dev/null)" 2>/dev/null || true)
        if [[ "${drv}" == "vfio-pci" ]]; then
            echo "  ${bdf}: already bound to vfio-pci"
            continue
        fi
        [[ -n "${drv}" && "${drv}" != "." ]] && { echo "  ${bdf}: unbinding from ${drv}"; echo "${bdf}" > "${dev_path}/driver/unbind"; }
        echo "  ${bdf}: binding to vfio-pci"
        echo "vfio-pci" > "${dev_path}/driver_override"
        echo "${bdf}" > /sys/bus/pci/drivers_probe
        echo "" > "${dev_path}/driver_override"
    done

    echo "== B2. Allocate ${HUGEPAGES_1G} x 1G hugepages on NUMA node ${NVME_NUMA} =="
    echo "  (MEM_TOTAL_GIB=${MEM_TOTAL_GIB} - VLLM_RESERVE_GIB=${VLLM_RESERVE_GIB} = ${HUGEPAGES_1G} hugepages; usable tier ~$((HUGEPAGES_1G - 3))G)"
    hp_path="/sys/devices/system/node/node${NVME_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages"
    if [[ ! -f "${hp_path}" ]]; then
        echo "error: ${hp_path} not found — 1 GiB hugepages unsupported on this node?" >&2
        exit 1
    fi
    echo "${HUGEPAGES_1G}" > "${hp_path}"
    got=$(cat "${hp_path}")
    echo "  node ${NVME_NUMA}: requested ${HUGEPAGES_1G}, got ${got} x 1G hugepages"
    if [[ "${got}" -lt "${HUGEPAGES_1G}" ]]; then
        echo "  warning: fewer hugepages than requested — not enough free contiguous RAM." >&2
        echo "           The server --memory-tier-size must fit in ${got} GiB." >&2
    fi
fi

cat <<EOF

Done.
  GPU:   CDI spec at /etc/cdi/nvidia.yaml (podman --device nvidia.com/gpu=<id>)
  Drives: ${NVME_BDFS} → vfio-pci
  Hugepages: $(cat "/sys/devices/system/node/node${NVME_NUMA}/hugepages/hugepages-1048576kB/nr_hugepages" 2>/dev/null || echo '?') x 1G on node ${NVME_NUMA}

Next:
  deps/build_spdk.sh && cargo build --release -p certus-server
  target/release/certus-server $(for b in ${NVME_BDFS}; do printf -- '--device-pci %s ' "$b"; done)\\
      --memory-tier-size <=N>G --shm-path /dev/shm/certus-shmq --format
  ./certus-shmq-connector/run-bench.sh     # container shares /dev/shm via --ipc=host
EOF
