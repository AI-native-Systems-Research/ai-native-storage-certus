# Certus System Configuration

## Hardware requirements
1. At least one GPU > Volta architecture, e.g., A30, A100
2. NVMe SSD that are not bound to the kernel nvme driver
3. ConnectX RDMA NIC or Bluefield DPU

## Installation and configuration sequence

1.  Clean operating system (remove old NVIDIA, OFED, etc.)
2.  Install DOCA (OFED only is sufficient for non-DPU RNIC)
3.  Install NVIDIA drivers
4.  Install CUDA drivers
5.  Configure SPDK