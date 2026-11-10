#!/bin/bash
sudo chmod -R a+rwx /dev/vfio
sudo chmod -R a+rwx /dev/hugepages
sudo sysctl -w net.core.rmem_max=67108864

# Set memlock unlimited for SPDK/DPDK DMA memory pinning
if ! grep -q "memlock unlimited" /etc/security/limits.conf; then
    echo "* soft memlock unlimited" | sudo tee -a /etc/security/limits.conf
    echo "* hard memlock unlimited" | sudo tee -a /etc/security/limits.conf
fi
