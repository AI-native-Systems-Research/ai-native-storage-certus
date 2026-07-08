# Feature Specification: File-Backed Block Device (block-device-filesys)

**Feature Branch**: `001-block-device-filesys`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `block-device-filesys` component provides a filesystem-backed block device that implements the `IBlockDevice` and `IBlockDeviceAdmin` interfaces from the Certus component framework. It uses a regular Linux file as its backing store, enabling development, testing, and benchmarking of the Certus storage stack without requiring physical NVMe hardware or SPDK.

The component follows the actor model: a dedicated thread runs an io_uring event loop for async IO operations and falls back to synchronous pread/pwrite with fdatasync for sync operations or when io_uring is unavailable. Multiple clients connect concurrently via lock-free SPSC channels, each receiving independent command/completion endpoints. The backing file is opened with O_DIRECT and O_SYNC to bypass the kernel page cache and simulate NVMe completion semantics, with automatic fallback to buffered IO on filesystems that do not support O_DIRECT (e.g., tmpfs).

## User Scenarios & Testing

### User Story 1 - Development Without Hardware (Priority: P1)

As a Certus developer, I want a block device component that uses a local file instead of real NVMe hardware, so that I can develop and test the storage stack on any Linux machine without special hardware or SPDK setup.

**Acceptance Scenarios**:

- **Given** a valid file path, block size (4096), and block count (256), **when** I call `BlockDeviceFilesysComponent::create()` followed by `initialize()`, **then** the backing file is created with exactly `block_size * num_blocks` bytes pre-allocated via fallocate, and the actor thread starts.
- **Given** an initialized component, **when** I call `connect_client()`, **then** I receive a `ClientChannels` struct with a command sender and completion receiver for issuing IO.
- **Given** connected client channels, **when** I send a `WriteSync` command followed by a `ReadSync` to the same LBA, **then** the read returns the exact bytes that were written (data integrity roundtrip).

### User Story 2 - Async IO with io_uring (Priority: P1)

As a performance engineer, I want async read/write operations backed by io_uring, so that I can benchmark realistic IO patterns with kernel-level async submission without needing NVMe hardware.

**Acceptance Scenarios**:

- **Given** an initialized component with io_uring available, **when** I send a `WriteAsync` command with a tag of 42, **then** I receive a `WriteDone` completion with the same tag echoed back and an assigned `OpHandle`.
- **Given** an initialized component on a system without io_uring (e.g., old kernel or container), **when** I send async commands, **then** the actor falls back to synchronous pread/pwrite and still delivers correct completions.
- **Given** an in-flight async operation, **when** the operation's timeout expires, **then** I receive a `Timeout` completion and the underlying io_uring SQE is cancelled via `AsyncCancel`.

### User Story 3 - Multi-Client Concurrent Access (Priority: P1)

As a system integrator, I want multiple clients to share a single block device concurrently, so that the dispatcher and other upper-layer components can each hold independent connections.

**Acceptance Scenarios**:

- **Given** an initialized component, **when** two clients connect and each writes to different LBAs, **then** each client can read the other's data correctly (shared backing store, independent channels).
- **Given** multiple connected clients, **when** one client sends commands, **then** completions are delivered only to that client's callback channel (no cross-contamination).

### User Story 4 - Error Handling and Validation (Priority: P2)

As a developer, I want clear error reporting for invalid operations, so that I can diagnose misconfiguration quickly.

**Acceptance Scenarios**:

- **Given** a component that has not been initialized, **when** I call `connect_client()`, **then** I receive `NvmeBlockError::NotInitialized`.
- **Given** a device with 16 blocks, **when** I issue a read at LBA 20, **then** I receive `NvmeBlockError::LbaOutOfRange`.
- **Given** namespace ID 2, **when** I issue any read or write, **then** I receive `NvmeBlockError::InvalidNamespace` (only ns_id=1 is supported).
- **Given** a command like `NsCreate`, `NsDelete`, `NsFormat`, or `ControllerReset`, **when** sent to this file-backed device, **then** I receive `NvmeBlockError::NotSupported`.

### User Story 5 - Write Zeros Support (Priority: P2)

As a storage consumer, I want to zero a range of blocks efficiently, so that I can deallocate or sanitize data regions.

**Acceptance Scenarios**:

- **Given** blocks previously written with non-zero data, **when** I send `Command::WriteZeros { lba: 0, num_blocks: 4 }`, **then** reading those blocks back yields all zeros, and the write is followed by fdatasync for durability.

### User Story 6 - Telemetry Collection (Priority: P3)

As an operator, I want IO statistics (latency, throughput, op count), so that I can profile the system's storage path.

**Acceptance Scenarios**:

- **Given** the component compiled with `--features telemetry`, **when** I call `telemetry()` after performing IO, **then** I receive a `TelemetrySnapshot` with non-zero `total_ops` and valid latency/throughput fields.
- **Given** the component compiled without the `telemetry` feature, **when** I call `telemetry()`, **then** I receive `NvmeBlockError::FeatureNotEnabled`.

## Requirements

### Functional Requirements

- **FR-001**: The component MUST implement `IBlockDevice` and `IBlockDeviceAdmin` interfaces as defined in the `interfaces` crate.
- **FR-002**: The component MUST use the actor model with a dedicated thread for all IO processing, communicating with clients via lock-free SPSC channels (capacity: 64 entries per channel).
- **FR-003**: The component MUST support synchronous read (`ReadSync`) and write (`WriteSync`) commands using pread/pwrite syscalls with fdatasync after each write.
- **FR-004**: The component MUST support asynchronous read (`ReadAsync`) and write (`WriteAsync`) commands via io_uring when available, with linked fsync SQEs for write durability.
- **FR-005**: The component MUST fall back to synchronous pread/pwrite when io_uring is unavailable (e.g., kernel too old, container restrictions), logging a warning.
- **FR-006**: The component MUST support `WriteZeros` commands by writing a zero-filled, 512-byte-aligned buffer followed by fdatasync.
- **FR-007**: The component MUST support `BatchSubmit` by processing each sub-command sequentially.
- **FR-008**: The component MUST support `AbortOp` by issuing an io_uring `AsyncCancel` and immediately acknowledging the abort.
- **FR-009**: The component MUST support `NsProbe` by returning a single namespace (ns_id=1) with the configured sector count and sector size.
- **FR-010**: The component MUST reject `NsCreate`, `NsDelete`, `NsFormat`, and `ControllerReset` commands with `NvmeBlockError::NotSupported`.
- **FR-011**: The component MUST validate that the block size is a power of 2 and at least 512 bytes, and that num_blocks > 0, during configuration validation.
- **FR-012**: The component MUST validate LBA ranges before every IO operation, returning `LbaOutOfRange` if the range exceeds device capacity.
- **FR-013**: The component MUST validate that ns_id == 1 for all namespace-scoped operations, returning `InvalidNamespace` otherwise.
- **FR-014**: The component MUST create the backing file via fallocate if it does not exist, and verify size matches the expected total bytes if it already exists.
- **FR-015**: The component MUST open the backing file with O_DIRECT | O_SYNC, falling back to buffered IO (with a stderr warning) if the filesystem returns EINVAL.
- **FR-016**: The component MUST support multiple concurrent clients, each with independent SPSC channel pairs, multiplexed by the single actor thread.
- **FR-017**: The component MUST check for async operation timeouts on each actor idle cycle and deliver `Completion::Timeout` for expired operations.
- **FR-018**: The component MUST propagate caller-assigned tags in async operation completions.
- **FR-019**: The component MUST assign monotonically increasing `OpHandle` values to each submitted operation.
- **FR-020**: The `IBlockDeviceAdmin` lifecycle methods (`set_pci_address`, `set_actor_cpu`, `signal_stop`, `detach_controller`) MUST be no-ops since this is a file-backed device (no real hardware to manage).

### Non-Functional Requirements

- **NFR-001**: The actor thread MUST use non-blocking polling (try_recv on client channels) to avoid stalling other clients while one is idle.
- **NFR-002**: The io_uring submission queue depth MUST be 128 entries (DEFAULT_RING_DEPTH).
- **NFR-003**: The maximum transfer size MUST be `block_size * 256`.
- **NFR-004**: The component MUST report `numa_node() == -1` (not hardware-pinned).
- **NFR-005**: The component MUST have Criterion benchmarks for latency (sync read/write 4KB) and throughput (write zeros at 1/8/32/128 blocks, read at same sizes).
- **NFR-006**: Feature-gated telemetry (`telemetry` feature flag) MUST NOT incur overhead when disabled (compile-time exclusion via `#[cfg(feature = "telemetry")]`).
- **NFR-007**: All DMA buffers used for O_DIRECT IO MUST be 512-byte aligned (posix_memalign).
- **NFR-008**: All unsafe code MUST have `// SAFETY:` justification comments.

## Key Entities

| Entity | Description |
|--------|-------------|
| `BlockDeviceFilesysComponent` | The top-level component struct, created via `define_component!`. Holds config, actor handle, and client ID counter. |
| `FilesysHandler` | The `ActorHandler<ControlMessage>` implementation that runs on the dedicated thread. Owns the file descriptor, io_uring instance, client sessions, and inflight operation tracking. |
| `ClientSession` | Per-client state held by the actor: client ID, ingress receiver, and callback sender. |
| `ControlMessage` | Enum sent to the actor: `ConnectClient`, `DisconnectClient`, `Shutdown`. |
| `DeviceConfig` | Validated configuration (file path, block size, num blocks, total bytes). |
| `InflightOp` | Tracking state for in-flight async io_uring operations (handle, client ID, deadline, tag). |
| `TelemetryStats` | Lock-free atomic counters for ops, latency, and bytes (behind `telemetry` feature gate). |

## Dependencies

| Dependency | Purpose |
|-----------|---------|
| `component-core` | Actor model, SPSC channels, `IUnknown` trait |
| `component-macros` | `define_component!` and `define_interface!` proc macros |
| `component-framework` | Facade re-export of the framework |
| `interfaces` (with `spdk` feature) | `IBlockDevice`, `IBlockDeviceAdmin`, `Command`, `Completion`, `DmaBuffer`, error types |
| `io-uring` (0.7) | Linux io_uring async IO submission and completion |
| `libc` (0.2) | pread, pwrite, fdatasync, fallocate, posix_memalign, free |
| `crossbeam-channel` (0.5) | (Available but primary channels are from `component-core::channel::spsc`) |

## Success Criteria

1. All unit tests pass (`cargo test -p block-device-filesys`).
2. All integration tests pass, verifying data integrity across sync/async read/write roundtrips, write zeros, multi-client access, error paths, and namespace probe.
3. The component builds and passes tests on systems both with and without io_uring support (graceful fallback).
4. Criterion benchmarks produce stable latency and throughput results.
5. The component integrates cleanly into the Certus dispatch chain as a drop-in replacement for the SPDK-backed `block-device-spdk-nvme` for testing purposes.
6. `cargo clippy -- -D warnings` and `cargo fmt --check` pass without error.

## Implementation Notes

- The io_uring write path uses `IO_LINK` to chain the write SQE with a subsequent `Fsync(DATASYNC)` SQE, ensuring durability semantics match NVMe write completion guarantees.
- The fsync completion CQE is identified by setting bit 63 of the user_data and is silently consumed (not forwarded to clients).
- The `on_idle()` handler returns `false` (stop the actor) only when shutdown is requested. When no clients are connected and no ops are inflight, it returns `false` to allow the thread to park via the actor framework's condvar. The actor remains responsive to control messages for new client connections.
- The `PciAddress` type from the `spdk_types` module is accepted by `IBlockDeviceAdmin::set_pci_address` but ignored since there is no physical controller.
- DmaBuffer lifetime management is the caller's responsibility; the actor borrows the Arc for the duration of the IO.
