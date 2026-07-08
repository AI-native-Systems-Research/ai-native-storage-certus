# Feature Specification: Block Device Kernel (io_uring)

**Feature Branch**: `001-block-device-kernel`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `block-device-kernel` component provides a Linux kernel block device driver that implements the `IBlockDevice` and `IBlockDeviceAdmin` interfaces using `io_uring` for all IO operations. It enables Certus to access raw NVMe (or other Linux block) devices through the standard kernel path rather than the SPDK userspace driver, making it suitable for environments where SPDK is unavailable or undesirable.

The component follows the Certus actor model: a dedicated thread owns the `io_uring` instance and file descriptor, processes client commands via lock-free SPSC channels, and delivers completions back to callers. All IO uses `O_DIRECT | O_DSYNC` to bypass the page cache and guarantee write durability without explicit fsync. Feature-gated telemetry (`--features telemetry`) provides runtime IO statistics (latency, throughput, operation counts).

## User Scenarios & Testing

### User Story 1 - Synchronous Block IO (Priority: P1)

As a storage subsystem consumer, I want to perform synchronous reads and writes to a raw block device at specified LBAs, so that I can store and retrieve data with guaranteed durability.

**Acceptance Scenarios**:

- **Given** the component is initialized with a valid block device path, **When** a `WriteSync` command is sent followed by a `ReadSync` at the same LBA, **Then** the read returns exactly the data that was written.
- **Given** a `WriteSync` command targets an LBA beyond the device range, **When** the command is processed, **Then** a `ReadDone`/`WriteDone` completion with `LbaOutOfRange` error is returned.
- **Given** a `WriteSync` command specifies `ns_id != 1`, **When** the command is processed, **Then** an `InvalidNamespace` error is returned.

### User Story 2 - Asynchronous Block IO (Priority: P1)

As a high-throughput data pipeline, I want to submit asynchronous reads and writes with timeout support, so that IO can be overlapped with computation and stalled operations do not block indefinitely.

**Acceptance Scenarios**:

- **Given** the component is initialized, **When** a `WriteAsync` command is sent with a 5-second timeout, **Then** a `WriteDone` completion arrives with the operation handle.
- **Given** an async operation exceeds its timeout, **When** the deadline passes, **Then** the actor sends a `Timeout` completion and cancels the io_uring operation via `AsyncCancel`.

### User Story 3 - Multi-Client Isolation (Priority: P1)

As a multi-tenant storage system, I want multiple clients to connect independently with their own SPSC channel pairs, so that IO from one client does not interfere with another.

**Acceptance Scenarios**:

- **Given** the component is initialized, **When** two clients connect and submit writes to different LBAs, **Then** each client receives completions only for its own operations.
- **Given** client 1 writes data to LBA X, **When** client 2 reads LBA X, **Then** client 2 observes the data written by client 1 (shared underlying device).

### User Story 4 - Device Initialization & Auto-Detection (Priority: P1)

As an operator, I want the component to automatically detect the device size when `num_blocks = 0` is specified, so that I do not need to manually query device geometry.

**Acceptance Scenarios**:

- **Given** `num_blocks = 0` is passed to `create()`, **When** `initialize()` is called on a valid block device, **Then** the device size is detected via `BLKGETSIZE64` ioctl and `num_sectors()` returns a positive value.
- **Given** a path that is not a block device (e.g., `/dev/null`), **When** `initialize()` is called, **Then** a `NotInitialized` error is returned indicating it is not a block device.

### User Story 5 - Write Zeros (Priority: P2)

As a storage system performing secure erasure or extent initialization, I want to zero a range of blocks atomically, so that subsequent reads of those blocks return all zeros.

**Acceptance Scenarios**:

- **Given** non-zero data exists at LBAs 0..3, **When** a `WriteZeros { lba: 0, num_blocks: 4 }` command is processed, **Then** a subsequent read of those blocks returns all-zero data.

### User Story 6 - Operation Abort (Priority: P2)

As a client that needs to cancel in-flight IO, I want to abort a previously submitted async operation by its handle, so that resources are freed and the operation does not complete.

**Acceptance Scenarios**:

- **Given** an async operation is in flight, **When** an `AbortOp` command is sent with its handle, **Then** the actor issues `AsyncCancel` to io_uring, removes the operation from the inflight map, and sends an `AbortAck` completion.

### User Story 7 - Namespace Probe (Priority: P2)

As a discovery subsystem, I want to probe available namespaces, so that I can learn device geometry without prior configuration.

**Acceptance Scenarios**:

- **Given** the component is initialized, **When** a `NsProbe` command is sent, **Then** a `NsProbeResult` completion is returned containing exactly one namespace (ns_id=1) with the configured sector count and sector size.

### User Story 8 - Telemetry (Priority: P3)

As a performance engineer, I want to collect IO statistics (operation count, min/max/mean latency, throughput), so that I can monitor and tune system performance.

**Acceptance Scenarios**:

- **Given** the component is compiled with `--features telemetry` and initialized, **When** `telemetry()` is called after IO operations, **Then** a `TelemetrySnapshot` is returned with non-zero `total_ops` and valid latency/throughput values.
- **Given** the component is compiled without the `telemetry` feature, **When** `telemetry()` is called, **Then** a `FeatureNotEnabled` error is returned.

## Requirements

### Functional Requirements

- **FR-001**: The component MUST implement the `IBlockDevice` trait, providing `connect_client()`, `sector_size()`, `num_sectors()`, `max_queue_depth()`, `num_io_queues()`, `max_transfer_size()`, `block_size()`, `numa_node()`, `nvme_version()`, and `telemetry()`.
- **FR-002**: The component MUST implement the `IBlockDeviceAdmin` trait, providing `initialize()`, `shutdown()`, `signal_stop()`, `set_pci_address()`, `set_actor_cpu()`, and `detach_controller()`. Admin methods not applicable to kernel devices (`set_pci_address`, `set_actor_cpu`, `signal_stop`, `detach_controller`) MUST be no-ops.
- **FR-003**: `connect_client()` MUST return a `ClientChannels` struct containing a command sender and completion receiver (SPSC channels with capacity 64). It MUST fail with `NotInitialized` if `initialize()` has not been called.
- **FR-004**: The actor MUST support the following `Command` variants: `ReadSync`, `WriteSync`, `ReadAsync`, `WriteAsync`, `WriteZeros`, `BatchSubmit`, `AbortOp`, `NsProbe`.
- **FR-005**: Commands `NsCreate`, `NsDelete`, `NsFormat`, and `ControllerReset` MUST return a `NotSupported` error.
- **FR-006**: All reads and writes MUST validate the LBA range before submission. If `lba + num_blocks > device_num_blocks` or if `ns_id != 1`, the operation MUST fail with `LbaOutOfRange` or `InvalidNamespace` respectively.
- **FR-007**: `WriteSync` and synchronous path of `ReadSync` MUST call `ring.submit_and_wait(1)` to block until the CQE arrives.
- **FR-008**: `WriteAsync` and `ReadAsync` MUST submit to io_uring non-blocking (`ring.submit()`) and track the operation in an inflight map keyed by the op handle.
- **FR-009**: The actor's `on_idle()` callback MUST poll all client ingress channels, harvest io_uring completions, and check for timed-out operations.
- **FR-010**: Timed-out operations MUST send a `Timeout` completion to the client and issue `AsyncCancel` on the io_uring ring.
- **FR-011**: `BatchSubmit` MUST recursively process each contained operation as an independent command.
- **FR-012**: `NsProbe` MUST return a single `NamespaceInfo` with `ns_id=1`, the configured block count, and configured sector size.
- **FR-013**: `WriteZeros` MUST allocate a 512-byte-aligned zero buffer via `posix_memalign`, submit a write via io_uring, wait for completion, and free the buffer.
- **FR-014**: `shutdown()` MUST send a `Shutdown` control message to the actor and call `deactivate()` to join the actor thread.
- **FR-015**: `DeviceConfig::new()` MUST reject block sizes < 512 or non-power-of-two. If `num_blocks = 0`, it MUST auto-detect via `BLKGETSIZE64` ioctl.
- **FR-016**: `open_block_device()` MUST open with `O_DIRECT | O_DSYNC`, verify `O_DIRECT` is active via `fcntl(F_GETFL)`, and invalidate page cache via `posix_fadvise(POSIX_FADV_DONTNEED)`.
- **FR-017**: Client IDs MUST be monotonically increasing (assigned via `AtomicU64::fetch_add`).
- **FR-018**: `max_queue_depth()` MUST return `128` (the io_uring ring depth).
- **FR-019**: `num_io_queues()` MUST return `1` (single actor thread).
- **FR-020**: `max_transfer_size()` MUST return `block_size * 256`.
- **FR-021**: `numa_node()` MUST return `-1` (not pinned).
- **FR-022**: `nvme_version()` MUST return `"N/A (kernel block device)"`.

### Non-Functional Requirements

- **NFR-001**: All IO MUST use `O_DIRECT` to bypass the kernel page cache, ensuring predictable latency without cache pollution.
- **NFR-002**: All writes MUST use `O_DSYNC` to guarantee data durability on completion without requiring a separate fsync.
- **NFR-003**: The SPSC channel capacity per client MUST be 64 slots to bound memory usage while allowing pipelining.
- **NFR-004**: The io_uring submission queue depth MUST be 128 entries.
- **NFR-005**: DMA buffers MUST be 512-byte aligned for `O_DIRECT` compatibility.
- **NFR-006**: The component MUST run on Linux only (depends on io_uring, `O_DIRECT`, `BLKGETSIZE64` ioctl, and `posix_memalign`).
- **NFR-007**: Telemetry collection MUST be zero-cost when the `telemetry` feature is disabled (compile-time gating via `#[cfg(feature = "telemetry")]`).
- **NFR-008**: The actor thread MUST be dedicated (one OS thread per component instance) to avoid head-of-line blocking across components.
- **NFR-009**: The component MUST have Criterion benchmarks measuring sync IO latency (read/write 4KB) and throughput (1/8/32/128 blocks).
- **NFR-010**: All unsafe code MUST include `// SAFETY:` justification comments explaining why the invariants hold.

## Key Entities

| Entity | Description |
|--------|-------------|
| `BlockDeviceKernelComponent` | Top-level component struct; holds device config, actor handle, and telemetry. Created via `define_component!` macro. |
| `DeviceConfig` | Validated configuration: device path, block size (>= 512, power of 2), num_blocks, total_bytes. |
| `KernelHandler` | Actor handler implementing `ActorHandler<ControlMessage>`; owns the fd, io_uring ring, client map, and inflight op map. |
| `ClientSession` | Per-client state: id, ingress receiver, callback sender. |
| `ControlMessage` | Enum: `ConnectClient`, `DisconnectClient`, `Shutdown`. |
| `InflightOp` | Tracking struct for async operations: handle, client_id, deadline, is_read flag, telemetry fields. |
| `TelemetryStats` | Lock-free atomic counters for ops, latency (min/max/total), and bytes transferred. |
| `ClientChannels` | Returned to caller: `command_tx` (Sender<Command>) and `completion_rx` (Receiver<Completion>). |

## Dependencies

| Dependency | Purpose |
|-----------|---------|
| `component-core` | Actor model, `ActorHandler` trait, SPSC channels, `IUnknown` |
| `component-macros` | `define_component!` proc macro |
| `component-framework` | Facade re-export |
| `interfaces` | `IBlockDevice`, `IBlockDeviceAdmin`, `Command`, `Completion`, `DmaBuffer`, `NvmeBlockError`, `TelemetrySnapshot` |
| `io-uring` (crate, v0.7) | io_uring syscall wrapper (submission/completion queues, opcodes) |
| `libc` (crate, v0.2) | `posix_memalign`, `posix_fadvise`, `ioctl`, `stat`, `fcntl`, `O_DIRECT`, `O_DSYNC` |
| `crossbeam-channel` (v0.5) | Used transitionally (actor MPSC control channel) |
| Linux kernel >= 5.1 | io_uring support |
| Raw block device | Backing store (e.g., `/dev/nvme0n1`, `/dev/sda`) |

## Success Criteria

1. **Correctness**: Write-then-read roundtrip at any valid LBA returns identical data (data integrity across 64+ blocks with unique patterns verified in integration tests).
2. **Durability**: Writes are durable on completion (O_DSYNC guarantees; no data loss on power failure for acknowledged writes).
3. **Error handling**: Invalid namespace, out-of-range LBA, non-block-device path, and uninitialized state all produce typed errors without panicking.
4. **Multi-client**: Two or more clients operate independently with their own channel pairs; no cross-contamination of completions.
5. **Performance**: Criterion benchmarks for 4KB read/write latency and multi-block throughput run without regression on target hardware.
6. **Clean lifecycle**: `initialize()` -> IO operations -> `shutdown()` completes without resource leaks; actor thread joins cleanly.
7. **Feature isolation**: Telemetry feature does not affect non-telemetry builds (zero-cost abstraction).

## Implementation Notes

- The component uses `define_component!` macro which auto-implements `IUnknown` for interface discovery and provides typed receptacle support. The `logger` receptacle is optional (gracefully degraded if unbound).
- `O_DIRECT` requires all IO buffers to be 512-byte aligned and IO sizes to be multiples of the sector size. The `DmaBuffer` type enforces this at the interface level.
- The `wait_for_cqe()` helper loops over the completion queue looking for the specific user_data key. While waiting for a synchronous operation, it also processes unrelated CQEs (from concurrent async operations) to avoid starvation.
- The `on_idle()` return value controls actor liveliness: returns `true` (keep running) when clients are connected or operations are in-flight; returns `false` only when `shutdown_requested` is set.
- Write-zeros uses a heap-allocated aligned buffer rather than `fallocate(FALLOC_FL_ZERO_RANGE)` because `O_DIRECT` file descriptors may not support fallocate on all block device types.
- The `BLKGETSIZE64` ioctl constant (`0x80081272`) is the x86_64 value; portability to other architectures would require conditional compilation.
- Batch operations (`BatchSubmit`) are processed sequentially within the actor (recursive `process_command` calls). They do not enable true multi-SQE batching at the io_uring level.
