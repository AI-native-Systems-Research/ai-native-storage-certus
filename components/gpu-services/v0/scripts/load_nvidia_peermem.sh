#!/bin/bash
# Load nvidia-peermem kernel module for GPU P2P DMA (GPUDirect RDMA/Storage).
#
# This enables SPDK (and other RDMA/DMA engines) to map GPU BAR memory
# through the IOMMU, allowing NVMe DMA directly into GPU device memory.
#
# Requirements:
#   - NVIDIA driver loaded (nvidia.ko)
#   - nvidia-peermem.ko built for the running kernel (DKMS or manual)
#   - Root/sudo access
#
# Usage:
#   sudo ./load_nvidia_peermem.sh          # load for this session
#   sudo ./load_nvidia_peermem.sh --persist # also load on boot

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

if [[ $EUID -ne 0 ]]; then
    error "This script must be run as root (use sudo)."
    exit 1
fi

# Check NVIDIA driver is loaded
if ! lsmod | grep -q '^nvidia '; then
    error "NVIDIA kernel driver not loaded. Load it first:"
    echo "  modprobe nvidia"
    exit 1
fi

# Check module exists for this kernel
if ! modinfo nvidia_peermem &>/dev/null; then
    error "nvidia-peermem module not found for kernel $(uname -r)."
    echo ""
    echo "Install options:"
    echo "  1. DKMS (if NVIDIA driver was installed via .run or CUDA toolkit):"
    echo "     dkms install nvidia-peermem/$(modinfo nvidia 2>/dev/null | grep '^version:' | awk '{print $2}') -k $(uname -r)"
    echo ""
    echo "  2. NVIDIA CUDA repository (RHEL/CentOS):"
    echo "     dnf install nvidia-peer-memory"
    echo ""
    echo "  3. Build from source:"
    echo "     git clone https://github.com/Mellanox/nv_peer_memory.git"
    echo "     cd nv_peer_memory && make && make install"
    exit 1
fi

# Load the module
if lsmod | grep -q '^nvidia_peermem '; then
    info "nvidia-peermem is already loaded."
else
    info "Loading nvidia-peermem..."
    modprobe nvidia-peermem
    if lsmod | grep -q '^nvidia_peermem '; then
        info "nvidia-peermem loaded successfully."
    else
        error "Failed to load nvidia-peermem."
        exit 1
    fi
fi

# Verify it's working
info "Module info:"
modinfo nvidia_peermem | grep -E '^(filename|version|description):'

# Persist across reboots if requested
if [[ "${1:-}" == "--persist" ]]; then
    CONF="/etc/modules-load.d/nvidia-peermem.conf"
    if [[ ! -f "$CONF" ]]; then
        echo "nvidia-peermem" > "$CONF"
        info "Persistence enabled: $CONF"
    else
        info "Already persistent: $CONF exists."
    fi
fi

echo ""
info "GPU P2P DMA is ready. You can now run the integration test:"
echo "  cargo test -p gpu-services --features 'gpu,spdk' --test gpu_nvme_p2p -- --nocapture"
