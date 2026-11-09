# IBlockDevice Contract: block-device-filesys

**Date**: 2026-06-04

This document specifies how block-device-filesys fulfills the IBlockDevice interface contract defined in `components/interfaces/src/iblock_device.rs`.

## Interface Methods

### connect_client() → Result<ClientChannels, NvmeBlockError>

- Creates per-client SPSC channels (capacity: 64)
- Registers client with actor via ControlMessage::ConnectClient
- Returns ClientChannels { command_tx, completion_rx }
- Error: NotInitialized if actor not started

### sector_size(ns_id: u32) → Result<u32, NvmeBlockError>

- Returns configured block_size
- ns_id must be 1 (single namespace); returns InvalidNamespace for other values

### num_sectors(ns_id: u32) → Result<u64, NvmeBlockError>

- Returns configured num_blocks
- ns_id must be 1; returns InvalidNamespace for other values

### max_queue_depth() → u32

- Returns io_uring SQ size (configurable, default 128)

### num_io_queues() → u32

- Returns 1 (single actor thread with single io_uring instance)

### max_transfer_size() → u32

- Returns block_size × 256 (arbitrary reasonable limit, 1MB for 4KB blocks)

### block_size() → u32

- Returns configured block_size

### numa_node() → i32

- Returns -1 (no NUMA affinity for file-backed device)

### nvme_version() → String

- Returns "N/A (file-backed)" (no NVMe hardware)

### telemetry() → Result<TelemetrySnapshot, NvmeBlockError>

- With `telemetry` feature: returns current statistics
- Without: returns FeatureNotEnabled error

## Command Processing Contract

### ReadSync { ns_id, lba, buf }

- Validates: ns_id == 1, lba + (buf.len / block_size) <= num_blocks
- Executes: pread(fd, buf.as_mut_slice(), lba × block_size)
- Completion: ReadDone { handle, result: Ok(()) }
- Error: LbaOutOfRange, InvalidNamespace

### WriteSync { ns_id, lba, buf }

- Validates: ns_id == 1, lba + (buf.len / block_size) <= num_blocks
- Executes: pwrite(fd, buf.as_slice(), lba × block_size) + fdatasync(fd)
- Completion: WriteDone { handle, result: Ok(()) }
- Error: LbaOutOfRange, InvalidNamespace

### ReadAsync { ns_id, lba, buf, timeout_ms }

- Validates same as ReadSync
- Submits: io_uring read SQE at offset lba × block_size
- Tracks: InflightOp with deadline = now + timeout_ms
- Completion (on CQE): ReadDone { handle, result }
- Timeout: Completion::Timeout { handle }

### WriteAsync { ns_id, lba, buf, timeout_ms }

- Validates same as WriteSync
- Submits: io_uring write SQE linked to fsync SQE (IOSQE_IO_LINK)
- Tracks: InflightOp with deadline
- Completion (on CQE): WriteDone { handle, result }
- Timeout: Completion::Timeout { handle }

### WriteZeros { ns_id, lba, num_blocks }

- Validates: ns_id == 1, lba + num_blocks <= total num_blocks
- Executes: pwrite zeros + fdatasync (inline, synchronous)
- Completion: WriteZerosDone { handle, result }

### BatchSubmit { ops }

- Processes each operation sequentially
- Individual operation failures do not abort the batch
- Each op gets its own Completion

### AbortOp { handle }

- Submits io_uring AsyncCancel for the target handle
- Completion: AbortAck { handle } (whether cancel succeeded or op already completed)

### NsProbe

- Returns NsProbeResult with single NamespaceInfo { ns_id: 1, num_sectors, sector_size }

### NsCreate / NsDelete / NsFormat / ControllerReset

- Returns Error { error: NotSupported("...") }
