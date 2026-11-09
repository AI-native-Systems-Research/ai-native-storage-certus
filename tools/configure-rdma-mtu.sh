#!/bin/bash
# Configure RDMA NIC MTU for all active RDMA-capable interfaces.
#
# Sets the network interface MTU (via ip link) to enable jumbo frames
# for optimal RDMA throughput.
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

FOUND=0

# Parse "rdma link show" output directly — format:
#   link <device>/<port> state ACTIVE physical_state LINK_UP netdev <netdev>
while read -r _ devport _ state _ _ rest; do
    dev=$(echo "$devport" | cut -d'/' -f1)
    port=$(echo "$devport" | cut -d'/' -f2)

    # Extract netdev from remaining fields
    netdev=""
    if [[ "$rest" =~ netdev[[:space:]]+([^[:space:]]+) ]]; then
        netdev="${BASH_REMATCH[1]}"
    fi

    if [[ -z "$netdev" ]]; then
        echo "  $dev/$port: state=$state (no netdev, skipping)"
        continue
    fi

    current_mtu=$(cat "/sys/class/net/$netdev/mtu" 2>/dev/null || echo "unknown")
    echo "  $dev/$port -> $netdev (state=$state, current MTU=$current_mtu)"

    if [[ "$state" != "ACTIVE" ]]; then
        echo "    SKIP: port not active"
        continue
    fi

    if ip link set dev "$netdev" mtu "$MTU" 2>/dev/null; then
        new_mtu=$(cat "/sys/class/net/$netdev/mtu")
        echo "    SET: $netdev MTU $current_mtu -> $new_mtu"
        FOUND=$((FOUND + 1))
    else
        echo "    FAIL: could not set MTU on $netdev (check driver/switch support)"
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
