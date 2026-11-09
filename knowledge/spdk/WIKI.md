# SPDK (Storage Performance Development Kit)

## Overview

SPDK is an open-source framework from Intel for building high-performance,
scalable, user-mode storage applications. It achieves extreme I/O performance by
moving storage drivers out of the kernel and into user space, eliminating context
switches, system calls, and interrupt overhead on the data path.

Repository: <https://github.com/spdk/spdk>

## Core Design Principles

- **Polled-mode operation** — instead of relying on interrupts, SPDK dedicates
  CPU cores that continuously poll for completion. This trades CPU cycles for
  deterministic, low-latency I/O.
- **User-space drivers** — NVMe and other device drivers run entirely in user
  space via VFIO/UIO, bypassing the kernel block layer.
- **Lock-free, share-nothing architecture** — each thread owns its resources
  (queues, buffers, bdevs). No locks are taken on the data path.
- **Run-to-completion threading** — a single thread runs an event loop
  (`spdk_thread`) that processes pollers and messages without preemption.

## Key Subsystems

| Subsystem | Purpose |
|---|---|
| **NVMe driver** (`lib/nvme`) | User-space NVMe driver supporting local PCIe, NVMe-oF (RDMA, TCP, FC), and multi-path. |
| **Block device layer** (`lib/bdev`) | Generic block device abstraction with pluggable back-ends (NVMe, AIO, malloc, Ceph RBD, etc.). Virtual bdevs (passthru, split, RAID, lvol, GPT) compose on top. |
| **Blobstore** (`lib/blob`) | Log-structured, extent-based object store built on bdevs. Manages blobs (variable-size, thin-provisioned) with copy-on-write snapshots and clones. |
| **BlobFS** (`lib/blobfs`) | Simple filesystem layer on top of Blobstore; used by RocksDB integration. |
| **Logical volumes** (`lib/lvol`) | Thin-provisioned volumes with snapshots and clones, built on Blobstore. |
| **NVMe-oF target** (`lib/nvmf`) | High-performance NVMe-over-Fabrics target supporting RDMA, TCP, and FC transports. |
| **iSCSI target** (`lib/iscsi`) | User-space iSCSI target. |
| **Vhost** (`lib/vhost`) | Vhost-user back-end for virtio-blk and virtio-scsi, enabling VM storage. |
| **Acceleration framework** (`lib/accel`) | Pluggable acceleration for copy, CRC, compress, encrypt operations (software or hardware via DSA/IAA). |
| **FTL** (`lib/ftl`) | Flash Translation Layer for open-channel and zoned SSDs. |
| **FSDEV** (`lib/fsdev`) | Filesystem device abstraction for FUSE-based exports. |
| **Thread / reactor** (`lib/thread`, `module/event`) | The SPDK threading model: lightweight `spdk_thread` instances scheduled on reactor cores with pollers. |
| **JSON-RPC** (`lib/jsonrpc`, `lib/rpc`) | Control-plane interface; all runtime configuration (create bdev, attach controller, etc.) is done via JSON-RPC. |

## Threading Model

SPDK's threading model is central to its performance:

1. **Reactors** — one per assigned CPU core. Each reactor runs a tight
   poll loop.
2. **spdk_thread** — a lightweight, cooperative thread scheduled on a reactor.
   Owns pollers, I/O channels, and message queues.
3. **Pollers** — callbacks registered on a thread that are invoked on each
   iteration (or at a timed interval). Used for completion processing,
   timeout checks, and periodic work.
4. **I/O channels** (`spdk_io_channel`) — per-thread, per-device data
   structures that give lock-free access to a device's resources.
5. **Messages** — inter-thread communication is via lock-free message passing
   (`spdk_thread_send_msg`), not shared-memory locks.

## Configuration

SPDK is configured at build time via `./configure` and at runtime via JSON-RPC.
Key build options:

| Flag | Effect |
|---|---|
| `--prefix=PATH` | Install prefix |
| `--without-crypto` | Disable crypto support |
| `--without-tests` | Skip unit tests |
| `--with-fio` | Build the FIO plugin |
| `--with-rdma` | Enable RDMA transport |
| `--with-fuse` | Enable FUSE support |

Runtime configuration is typically done through the `spdk_tgt` application
using JSON-RPC calls or a JSON config file.

## Build Dependencies

On RHEL/Fedora systems the following packages are required:

```
numactl-devel ninja-build CUnit-devel libuuid-devel libaio-devel
ncurses-devel patchelf meson python3-pyelftools
```

---

# DPDK (Data Plane Development Kit)

## Overview

DPDK is a set of user-space libraries and drivers for fast packet processing,
originally developed by Intel. SPDK uses DPDK as its environment layer
(`env_dpdk`) for:

- **Memory management** — hugepage-backed, NUMA-aware allocation via
  `rte_malloc` and the mempool library.
- **PCI device access** — user-space PCI driver binding (VFIO/UIO).
- **Core affinity and threading** — EAL (Environment Abstraction Layer)
  manages lcore-to-CPU pinning.
- **Lock-free data structures** — rings, hash tables, and mempools.

Repository: <https://github.com/spdk/dpdk> (SPDK's fork, pinned as a git submodule)

## Key DPDK Libraries Used by SPDK

| Library | Purpose |
|---|---|
| **EAL** (`librte_eal`) | Environment Abstraction Layer — initialization, hugepage setup, lcore management, PCI enumeration. |
| **Ring** (`librte_ring`) | Lock-free FIFO ring buffer (SPSC and MPMC variants). Foundation of the mempool. |
| **Mempool** (`librte_mempool`) | Fixed-size object pool backed by hugepages. Used for NVMe command buffers, I/O descriptors. |
| **PCI bus** (`librte_bus_pci`) | User-space PCI device scanning and driver binding. |
| **Vhost** (`librte_vhost`) | Vhost-user protocol implementation for VM virtio backends. |
| **Mbuf** (`librte_mbuf`) | Message buffer type (packet descriptor), used for network-facing transports. |
| **Power** (`librte_power`) | CPU frequency scaling for power management. |
| **Cryptodev** (`librte_cryptodev`) | Crypto device abstraction (hardware and software providers). |

## How SPDK Uses DPDK

SPDK links DPDK statically and wraps it behind `lib/env_dpdk`:

- `spdk_env_init()` calls `rte_eal_init()` with the user's core mask,
  hugepage config, and PCI allow/block lists.
- `spdk_malloc()` / `spdk_zmalloc()` are thin wrappers around `rte_malloc()`
  with NUMA node selection.
- `spdk_pci_enumerate()` uses DPDK's PCI bus to scan and bind NVMe devices.
- The NVMe driver allocates command/completion queue pairs from DPDK mempools
  backed by hugepages (DMA-safe memory).

## DPDK Build Integration

DPDK is built automatically as part of the SPDK build process. SPDK's
`configure` script invokes Meson to configure DPDK with a minimal set of
libraries and drivers (only those needed by SPDK), then Ninja compiles it.
The built DPDK is placed in `dpdk/build/` within the SPDK source tree.

## Hugepages

DPDK (and therefore SPDK) requires Linux hugepages for DMA-safe, pinned memory.
Typical setup:

```bash
echo 1024 > /sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages
mkdir -p /dev/hugepages
mount -t hugetlbfs nodev /dev/hugepages
```

Or for 1 GB hugepages (better TLB efficiency for large allocations):

```bash
# Kernel boot parameter: default_hugepagesz=1G hugepagesz=1G hugepages=4
```

## VFIO / UIO

For DPDK/SPDK to access PCI devices from user space, the device must be
unbound from its kernel driver and bound to `vfio-pci` (preferred) or
`uio_pci_generic`. SPDK provides a setup script:

```bash
scripts/setup.sh   # unbinds NVMe devices from kernel, allocates hugepages
scripts/setup.sh reset  # restores kernel drivers
```
