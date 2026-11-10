# block-device-filesys

**Crate**: `block-device-filesys`
**Path**: `components/block-device-filesys/`
**Version**: 0.1.0
**Features**: `telemetry` (IO statistics)

## Description

File-backed block device that implements the same `IBlockDevice` interface as the SPDK NVMe driver, allowing the Certus system to operate without hardware NVMe drives. Uses a regular Linux file as its backing store with `O_DIRECT | O_SYNC` to bypass page cache. Falls back to buffered IO on filesystems that don't support O_DIRECT (e.g., tmpfs).

Uses `io_uring` for async IO. If io_uring initialization fails (old kernel), degrades gracefully to synchronous `pread`/`pwrite`. Every write is followed by `fdatasync` (linked io_uring SQE or explicit syscall) to simulate NVMe write-completion durability semantics.

## Component Definition

```
BlockDeviceFilesysComponent {
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

- **Actor model**: single OS thread runs io_uring event loop (depth 128). Clients communicate via SPSC channels (capacity 64 per client).
- **Non-blocking completion delivery**: per-client `VecDeque` backlog prevents head-of-line blocking across clients.
- **Command support**: ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp (io_uring AsyncCancel), NsProbe. NVMe-specific commands (NsCreate/Delete/Format, ControllerReset) return `NotSupported`.
- **Timeout handling**: per-op deadlines checked each poll cycle; timed-out ops get `Completion::Timeout` + `AsyncCancel`.
- **Single namespace**: only `ns_id=1` supported.
