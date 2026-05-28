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
exclude=kernel*5.14.0-687* kernel*5.14.0-7* rdma-core* libibverbs* ibacm* infiniband-diags* perftest* librdmacm* libmlx5* libibumad* opensm*

```

To check OFED is loaded and working do `modinfo ib_core` and check that it is pointing to extra not to drivers/infiniband. Make sure to install 'kernel-headers' package before trying to install OFED.**

Instal kernel source and development:
```
sudo dnf groupinstall "Development Tools"
sudo dnf install kernel-devel-$(uname -r) kernel-headers-$(uname -r)
```

Check with:
```
ls -l /lib/modules/$(uname -r)/build
```

Do not install/upgrade kernel headers during DOCA ofed install. Some times need..
```
depmod -a
dracut -f --regenerate-all
```

