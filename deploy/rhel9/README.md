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

## Kernel requirements

doca-ofed does not build with some kernels (e.g., 5.14.0-687.10.1).
Known working kernels with DOCA 3.3.0 are 5.14.0-611.30.1

Lock in kernel:
```
sudo grubby --set-default=/boot/vmlinuz-5.14.0-611.30.1.el9_7.x86_64
```

Prevent kernel upgrades, open /etc/dnf/dnf.conf and append the following instruction line:
```
exclude=kernel*5.14.0-687* kernel*5.14.0-7*
```
