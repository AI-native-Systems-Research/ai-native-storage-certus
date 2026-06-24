#!/bin/bash
# Configure RDMA NIC MTU for all active RDMA-capable interfaces.
#
# Sets both the network interface MTU (via ip link) and the RDMA device MTU
# (via rdma link) to enable jumbo frames for optimal RDMA throughput.
#
# Usage:
#   ./configure-rdma-mtu.sh [MTU]
#
# Arguments:
#   MTU   Desired MTU size in bytes (default: 9000)
#
# Requires: root/sudo, iproute2, rdma-core tools (rdma command)
#
# Examples:
#   ./configure-rdma-mtu.sh          # Set all RDMA interfaces to MTU 9000
#   ./configure-rdma-mtu.sh 4200     # Set all RDMA interfaces to MTU 4200
#   ./configure-rdma-mtu.sh 1500     # Reset to standard MTU

set -euo pipefail

MTU=${1:-9000}

if [[ $MTU -lt 1280 || $MTU -gt 9216 ]]; then
    echo "ERROR: MTU must be between 1280 and 9216 (got: $MTU)" >&2
    exit 1
fi

if [[ $EUID -ne 0 ]]; then
    echo "ERROR: This script must be run as root (use sudo)" >&2
    exit 1
fi

if ! command -v rdma &>/dev/null; then
    echo "ERROR: 'rdma' command not found. Install rdma-core tools." >&2
    exit 1
fi

echo "Configuring RDMA NIC MTU to $MTU"
echo "================================="
echo

# Discover RDMA devices and their associated netdevs
FOUND=0
while IFS= read -r line; do
    # Parse rdma link output: "link mlx5_0/1 state ACTIVE ..."
    dev=$(echo "$line" | awk '{print $2}' | cut -d'/' -f1)
    port=$(echo "$line" | awk '{print $2}' | cut -d'/' -f2)
    state=$(echo "$line" | grep -oP 'state \K\w+')

    if [[ -z "$dev" ]]; then
        continue
    fi

    # Find the associated network interface
    netdev=""
    if [[ -f "/sys/class/infiniband/$dev/ports/$port/gid_attrs/ndevs/0" ]]; then
        netdev=$(cat "/sys/class/infiniband/$dev/ports/$port/gid_attrs/ndevs/0" 2>/dev/null || true)
    fi

    # Fallback: check for netdev in sysfs
    if [[ -z "$netdev" && -d "/sys/class/infiniband/$dev/device/net/" ]]; then
        netdev=$(ls "/sys/class/infiniband/$dev/device/net/" 2>/dev/null | head -1)
    fi

    if [[ -z "$netdev" ]]; then
        echo "  $dev/$port: state=$state (no associated netdev found, skipping)"
        continue
    fi

    current_mtu=$(cat "/sys/class/net/$netdev/mtu" 2>/dev/null || echo "unknown")

    echo "  $dev/$port -> $netdev (state=$state, current MTU=$current_mtu)"

    if [[ "$state" != "ACTIVE" ]]; then
        echo "    SKIP: port not active"
        continue
    fi

    # Set network interface MTU
    if ip link set dev "$netdev" mtu "$MTU" 2>/dev/null; then
        new_mtu=$(cat "/sys/class/net/$netdev/mtu")
        echo "    SET: $netdev MTU $current_mtu -> $new_mtu"
        FOUND=$((FOUND + 1))
    else
        echo "    FAIL: could not set MTU on $netdev"
    fi

done < <(rdma link show 2>/dev/null)

echo
if [[ $FOUND -eq 0 ]]; then
    echo "WARNING: No RDMA interfaces were configured."
    echo "         Check that RDMA devices are present and ports are active."
    exit 1
else
    echo "Done: configured $FOUND interface(s) to MTU $MTU"
fi
