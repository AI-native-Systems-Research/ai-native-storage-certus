#!/bin/bash
# P2P Capability Preflight — classifies this machine's GPU-direct/P2P support.
#
# Checks: nvidia-peermem, gdrdrv, PCIe topology, hugepages, VFIO, GPU BAR1.
# Outputs JSON to stdout.
#
# Usage:
#   ./check_p2p_capability.sh [--nvme-pci 0000:62:00.0] [--gpu-id 0]

set -euo pipefail

NVME_PCI="${1:-}"
GPU_ID="${2:-0}"

# --- Helper functions ---

check_module() {
    lsmod 2>/dev/null | grep -q "^$1 " && echo "true" || echo "false"
}

get_gpu_bar1_size() {
    nvidia-smi --query-gpu=bar1.total --format=csv,noheader,nounits -i "$GPU_ID" 2>/dev/null || echo "0"
}

get_gpu_pci_address() {
    nvidia-smi --query-gpu=pci.bus_id --format=csv,noheader -i "$GPU_ID" 2>/dev/null | tr '[:upper:]' '[:lower:]' || echo "unknown"
}

check_hugepages() {
    local nr=$(cat /sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages 2>/dev/null || echo "0")
    local free=$(cat /sys/kernel/mm/hugepages/hugepages-2048kB/free_hugepages 2>/dev/null || echo "0")
    echo "{\"nr\": $nr, \"free\": $free, \"sufficient\": $([ "$nr" -ge 512 ] && echo true || echo false)}"
}

check_vfio() {
    if [ -n "$NVME_PCI" ]; then
        local driver=$(readlink "/sys/bus/pci/devices/$NVME_PCI/driver" 2>/dev/null | xargs basename 2>/dev/null || echo "none")
        if [ "$driver" = "vfio-pci" ]; then
            echo "true"
        else
            echo "false"
        fi
    else
        # Check if any NVMe device is bound to vfio-pci
        ls /sys/bus/pci/drivers/vfio-pci/*/class 2>/dev/null | xargs grep -l "0x010802" >/dev/null 2>&1 && echo "true" || echo "false"
    fi
}

check_same_root_complex() {
    local gpu_pci=$(get_gpu_pci_address)
    if [ -z "$NVME_PCI" ] || [ "$gpu_pci" = "unknown" ]; then
        echo "unknown"
        return
    fi
    # Compare PCIe domain (first 4 hex digits)
    local gpu_domain=$(echo "$gpu_pci" | cut -d: -f1)
    local nvme_domain=$(echo "$NVME_PCI" | cut -d: -f1)
    if [ "$gpu_domain" = "$nvme_domain" ]; then
        echo "true"
    else
        echo "false"
    fi
}

check_gdrcopy() {
    if $(check_module "gdrdrv") = "true" && ldconfig -p 2>/dev/null | grep -q "libgdrapi"; then
        echo "true"
    elif [ -f "/dev/gdrdrv" ]; then
        echo "true"
    else
        $(check_module "gdrdrv")
    fi
}

# --- Collect data ---

NVIDIA_PEERMEM=$(check_module "nvidia_peermem")
GDRDRV=$(check_module "gdrdrv")
GDRCOPY_AVAILABLE=$(check_gdrcopy)
GPU_BAR1_MB=$(get_gpu_bar1_size)
GPU_PCI=$(get_gpu_pci_address)
HUGEPAGES=$(check_hugepages)
VFIO_BOUND=$(check_vfio)
SAME_ROOT=$(check_same_root_complex)

# Determine P2P capability
P2P_CAPABLE="false"
if [ "$NVIDIA_PEERMEM" = "true" ] && [ "$GDRDRV" = "true" ]; then
    P2P_CAPABLE="true"
fi

# Determine recommended modes
MODES="[\"bounce_pinned\""
if [ "$P2P_CAPABLE" = "true" ]; then
    MODES="$MODES, \"p2p_gdrcopy\""
fi
MODES="$MODES]"

# --- Output JSON ---

cat <<EOF
{
  "p2p_capable": $P2P_CAPABLE,
  "gdrcopy_available": $GDRCOPY_AVAILABLE,
  "nvidia_peermem_loaded": $NVIDIA_PEERMEM,
  "gdrdrv_loaded": $GDRDRV,
  "same_root_complex": $SAME_ROOT,
  "gpu_bar1_mb": $GPU_BAR1_MB,
  "gpu_pci": "$GPU_PCI",
  "nvme_pci": "${NVME_PCI:-auto}",
  "vfio_bound": $VFIO_BOUND,
  "hugepages": $HUGEPAGES,
  "recommended_modes": $MODES,
  "topology": "$(nvidia-smi topo -m 2>/dev/null | head -5 | tr '\n' '|' || echo 'unavailable')"
}
EOF
