# block-device-kernel

**Crate**: `block-device-kernel`
**Path**: `components/block-device-kernel/`
**Version**: 0.1.0
**Features**: `telemetry` (IO statistics)

## Description

Linux kernel-path block device driver using `io_uring` on raw block devices (e.g., `/dev/nvme0n1`). Implements the same `IBlockDevice` interface as the SPDK-based driver for environments where userspace NVMe drivers are not available. Opens devices with `O_DIRECT | O_DSYNC` and invalidates page cache via `posix_fadvise(DONTNEED)`.

## Component Definition

```
BlockDeviceKernelComponent {
    version: "0.1.0",
    provides: [IBlockDevice, IBlockDeviceAdmin],
    receptacles: {
        logger: ILogger,
    },
}
```

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging during initialization and client connections |

## Key Semantics

- **Actor model**: single OS thread with io_uring event loop (ring depth 128). Clients communicate via SPSC channels.
- **io_uring only**: all reads/writes submitted as io_uring SQEs. No pread/pwrite fallback.
- **Non-blocking completion delivery**: per-client `VecDeque` backlog for head-of-line blocking prevention.
- **Command support**: ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp (io_uring AsyncCancel), NsProbe. Namespace management commands return `NotSupported`.
- **Device validation**: verifies path is a block device via `stat(2)`, checks block_size >= 512 and power-of-two, auto-detects size via `BLKGETSIZE64` ioctl.
- **Single namespace**: only `ns_id=1` supported.
- **IBlockDeviceAdmin**: `set_pci_address`, `set_actor_cpu` are no-ops (not applicable to kernel devices).
