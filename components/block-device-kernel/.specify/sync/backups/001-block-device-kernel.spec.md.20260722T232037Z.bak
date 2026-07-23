# Feature Specification: Block Device Kernel Component

**Feature Branch**: `001-block-device-kernel`

**Created**: 2026-07-08

**Status**: Backfilled

**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `block-device-kernel` component implements the `IBlockDevice` interface using a raw Linux block device (e.g., `/dev/nvme0n1`) as the backing store. All IO is routed exclusively through `io_uring` — there is no `pread`/`pwrite` fallback path. The file descriptor is opened with `O_DIRECT | O_DSYNC` to bypass the kernel page cache and guarantee write durability without explicit `fsync` calls.

The component follows the Certus actor model: a dedicated thread runs an `io_uring` event loop, processing commands from multiple clients via lock-free SPSC channels. It serves as a kernel-native alternative to the SPDK-based `block-device-spdk-nvme` component, providing the same `IBlockDevice` interface semantics without requiring SPDK userspace drivers.

## User Scenarios & Testing

### User Story 1 - Direct Block IO on Raw Device (Priority: P1)

A developer or system operator needs to perform block-level IO against a raw Linux NVMe device using the standard `IBlockDevice` interface. They configure the component with a device path, block size, and optional block count (auto-detect if 0), initialize it, connect a client, and perform synchronous and asynchronous read/write operations.

**Why this priority**: This is the fundamental capability of the component — without raw device IO, it has no purpose.

**Independent Test**: Requires a real block device (override via `TEST_BLOCK_DEVICE` env var). Tests write data, read it back, and verify integrity.

**Acceptance Scenarios**:

1. **Given** a valid block device path and block size, **When** the component is initialized, **Then** the device is opened with `O_DIRECT | O_DSYNC`, an `io_uring` ring (depth 128) is created, the actor thread starts, and stale page-cache pages are dropped via `posix_fadvise(POSIX_FADV_DONTNEED)`.
2. **Given** an initialized component with a connected client, **When** the client sends a `WriteSync` command followed by a `ReadSync` at the same LBA, **Then** the data read back matches the data written exactly, and writes are durable by virtue of `O_DSYNC`.
3. **Given** an initialized component, **When** the client sends `ReadAsync` and `WriteAsync` commands with timeout, **Then** operations are submitted to `io_uring` asynchronously and completions are delivered on the completion channel with correct `OpHandle` values.
4. **Given** an initialized component, **When** the client sends a `WriteZeros` command for N blocks at a given LBA, **Then** reading that LBA range returns all-zero data. The zero buffer is allocated via `posix_memalign` with 512-byte alignment for `O_DIRECT` compatibility.
5. **Given** `num_blocks = 0` at creation, **When** the component is initialized, **Then** the device size is auto-detected via `BLKGETSIZE64` ioctl and the block count is derived from `device_bytes / block_size`.

---

### User Story 2 - Drop-In Replacement for SPDK Block Device (Priority: P2)

A developer working on higher-level Certus components (dispatcher, extent-manager) needs to use a kernel-native block device without SPDK dependencies. They bind `block-device-kernel` in place of `block-device-spdk-nvme` via the component-framework's interface discovery mechanism.

**Why this priority**: API compatibility enables seamless substitution for environments where SPDK is unavailable or undesirable.

**Independent Test**: Exercise all `IBlockDevice` query methods and verify NVMe-specific operations return `NotSupported`.

**Acceptance Scenarios**:

1. **Given** the `block-device-kernel` component, **When** queried via `IBlockDevice` methods, **Then** `sector_size(1)` returns the configured block size, `num_sectors(1)` returns the configured block count, `block_size()` returns the configured block size, `max_queue_depth()` returns 128 (ring depth), `num_io_queues()` returns 1, `max_transfer_size()` returns `block_size * 256`.
2. **Given** the component, **When** queried via `IUnknown`, **Then** it provides `IBlockDevice` and `IBlockDeviceAdmin` interfaces.
3. **Given** the component, **When** `NsCreate`, `NsDelete`, `NsFormat`, or `ControllerReset` commands are sent, **Then** a `NotSupported` error completion is returned.
4. **Given** the component, **When** `NsProbe` is sent, **Then** a single `NamespaceInfo` is returned with `ns_id=1`, the configured `num_sectors`, and the configured `sector_size`.

---

### User Story 3 - Multi-Client Concurrent Access (Priority: P2)

Multiple clients need to perform IO against the same block device simultaneously. Each client receives independent SPSC channel pairs and the actor serializes all operations through the single `io_uring` event loop.

**Why this priority**: Multi-client support is essential for production use where multiple subsystems share a device.

**Independent Test**: Connect two clients, have each write to different LBAs, then cross-read to verify data isolation and integrity.

**Acceptance Scenarios**:

1. **Given** an initialized component, **When** multiple clients call `connect_client()`, **Then** each receives independent `ClientChannels` with unique client IDs.
2. **Given** two connected clients, **When** client A writes to LBA 0 and client B writes to LBA 1, **Then** client A can read LBA 1 and see client B's data (shared device, independent channels).

---

### User Story 4 - Performance Benchmarking (Priority: P3)

A developer needs to benchmark the kernel block device to compare latency and throughput against the SPDK-based component. Criterion benchmarks measure sync IO latency and sequential throughput at varying block counts.

**Why this priority**: Performance measurement is critical for a storage component but the component must function correctly first.

**Independent Test**: `cargo bench` produces Criterion results for latency and throughput benchmark groups.

**Acceptance Scenarios**:

1. **Given** the component crate, **When** `cargo bench --bench latency` is run, **Then** Criterion benchmarks execute for command construction latency and synchronous 4KB read/write latency.
2. **Given** the component crate, **When** `cargo bench --bench throughput` is run, **Then** benchmarks measure write and read throughput at 1, 8, 32, and 128 block transfer sizes (4KB blocks).

---

### Edge Cases

- **Non-block device path**: Initialization must return `NotInitialized` error (verified via `stat(2)` checking `S_IFBLK`).
- **Non-existent device path**: Initialization must return `NotInitialized` error.
- **LBA out of range**: Read/write at LBA beyond configured device size returns `LbaOutOfRange` error.
- **Invalid namespace**: Operations with `ns_id != 1` return `InvalidNamespace` error.
- **Block size < 512**: `DeviceConfig::new()` rejects with error.
- **Block size not power of 2**: `DeviceConfig::new()` rejects with error.
- **Device size not multiple of block size**: Auto-detect returns error.
- **io_uring submission queue full**: Synchronous operations return an error completion; the ring is not silently dropped.
- **Async operation timeout**: Timed-out operations produce `Completion::Timeout` and an `AsyncCancel` is submitted to io_uring.
- **Client callback channel disconnected**: Logged at warn level, completion silently dropped.
- **connect_client before initialize**: Returns `NotInitialized` error.
- **Telemetry without feature**: Returns `FeatureNotEnabled` error.

## Requirements

### Functional Requirements

- **FR-001**: Component MUST implement `IBlockDevice` and `IBlockDeviceAdmin` as defined in `components/interfaces/`.
- **FR-002**: Component MUST declare a receptacle for `ILogger` and log initialization at info level, operations at debug level, and channel disconnections at warn level.
- **FR-003**: Component MUST use `define_component!` macro from the component-framework with `provides: [IBlockDevice, IBlockDeviceAdmin]` and `receptacles: { logger: ILogger }`.
- **FR-004**: Component MUST open the backing block device with `O_DIRECT | O_DSYNC` flags to bypass the page cache and guarantee write durability without explicit fsync.
- **FR-005**: Component MUST use `io_uring` (via the `io-uring` crate v0.7) as the sole IO mechanism — no `pread`/`pwrite` fallback.
- **FR-006**: Component MUST use a raw Linux block device (`S_IFBLK`) as the backing store; regular files are rejected.
- **FR-007**: Component MUST support configurable block size (minimum 512, must be power of 2) and device capacity (number of blocks, or 0 for auto-detect via `BLKGETSIZE64`).
- **FR-008**: Component MUST support synchronous read/write (`ReadSync`, `WriteSync`) by submitting io_uring SQEs and calling `submit_and_wait(1)`.
- **FR-009**: Component MUST support asynchronous read/write (`ReadAsync`, `WriteAsync`) with configurable timeout in milliseconds and `OpHandle` tracking via an inflight operation map.
- **FR-010**: Component MUST support `WriteZeros` by allocating a 512-byte-aligned zero buffer via `posix_memalign`, writing via io_uring, and freeing the buffer after completion.
- **FR-011**: Component MUST support `BatchSubmit` by processing each operation in the batch sequentially via recursive `process_command` dispatch.
- **FR-012**: Component MUST support `AbortOp` by submitting an `AsyncCancel` SQE to io_uring and sending an `AbortAck` completion.
- **FR-013**: Component MUST support `NsProbe` returning a single `NamespaceInfo` with `ns_id=1` and configured geometry.
- **FR-014**: Component MUST return `NotSupported` error for `NsCreate`, `NsDelete`, `NsFormat`, and `ControllerReset` commands.
- **FR-015**: Component MUST use the actor model with a dedicated OS thread running an io_uring event loop. The actor implements `ActorHandler<ControlMessage>` with `handle()` for control messages and `on_idle()` for polling clients, harvesting completions, and checking timeouts.
- **FR-016**: Component MUST use per-client SPSC channels (capacity 64) for command ingress and completion callbacks, created via `SpscChannel` from `component_core`.
- **FR-017**: Component MUST validate LBA bounds before every IO operation: `lba + num_blocks <= device_num_blocks`, with overflow checking via `checked_add`.
- **FR-018**: Component MUST validate namespace ID (`ns_id == 1`) for `sector_size()`, `num_sectors()`, and all IO operations.
- **FR-019**: Component MUST drop stale page-cache pages on initialization via `posix_fadvise(fd, 0, 0, POSIX_FADV_DONTNEED)`.
- **FR-020**: Component MUST verify `O_DIRECT` is active on the opened fd via `fcntl(F_GETFL)` check.
- **FR-021**: Component MUST provide feature-gated telemetry (`--features telemetry`) that tracks total ops, min/max/mean latency, total bytes, and mean throughput. Without the feature, `telemetry()` returns `FeatureNotEnabled`.
- **FR-022**: Component MUST provide Criterion benchmarks for latency (command construction + sync 4KB IO) and throughput (write + read at 1/8/32/128 block transfer sizes).
- **FR-023**: `IBlockDeviceAdmin` methods `set_pci_address`, `set_actor_cpu`, `signal_stop`, and `detach_controller` MUST be no-ops (not applicable to kernel block devices).
- **FR-024**: Component MUST support graceful shutdown via `ControlMessage::Shutdown` which causes `on_idle()` to return `false`, terminating the actor loop.

### Non-Functional Requirements

- **NFR-001**: All IO buffers MUST be 512-byte aligned (enforced by `O_DIRECT` requirement on the fd).
- **NFR-002**: The default io_uring submission queue depth is 128 entries.
- **NFR-003**: The component MUST not panic on IO errors — all errors are propagated as `NvmeBlockError` variants via completions.
- **NFR-004**: Unsafe code blocks MUST include `// SAFETY:` justification comments.
- **NFR-005**: The component MUST be `Send`-safe (required for actor model).
- **NFR-006**: The actor `on_idle()` remains active (returns `true`) while clients are connected or inflight operations exist.
- **NFR-007**: Timeout checking uses `Instant::now()` comparison against per-operation deadlines calculated from `timeout_ms`.
- **NFR-008**: Platform requirement: Linux kernel >= 5.1 (io_uring support); tested on RHEL 9 (kernel 5.14).

## Key Entities

- **`BlockDeviceKernelComponent`**: The component struct, created via `define_component!`. Holds device path, block size, num_blocks, actor handle, client ID counter, and optional telemetry stats. Version "0.1.0".
- **`KernelHandler`**: The actor handler implementing `ActorHandler<ControlMessage>`. Owns the file descriptor (`OwnedFd`), `DeviceConfig`, `IoUring` ring, client sessions map, inflight operations map, and optional logger.
- **`DeviceConfig`**: Configuration struct holding device path, block size, num_blocks, and total_bytes. Validates block size (>= 512, power of 2) and optionally auto-detects device size.
- **`ClientSession`**: Per-client state held by the actor: client ID, ingress receiver, callback sender.
- **`ControlMessage`**: Enum with variants `ConnectClient`, `DisconnectClient`, `Shutdown`.
- **`InflightOp`**: Tracking state for in-flight async operations: handle, client_id, deadline, is_read flag, and optional telemetry fields.
- **`TelemetryStats`**: Feature-gated atomic counters for IO statistics (total_ops, min/max/total latency, total_bytes, start time).

## Dependencies

### Crate Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `component-core` | workspace | Actor, channel, IUnknown traits |
| `component-macros` | workspace | `define_component!` macro |
| `component-framework` | workspace | Facade re-export |
| `interfaces` | workspace (features=["spdk"]) | IBlockDevice, IBlockDeviceAdmin, Command, Completion, DmaBuffer |
| `io-uring` | 0.7 | io_uring submission/completion ring |
| `libc` | 0.2 | Raw syscalls: posix_memalign, stat, ioctl, fcntl, posix_fadvise |
| `crossbeam-channel` | 0.5 | (Available but primary channels use component-core SPSC) |

### Dev Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `criterion` | 0.5 | Benchmark framework |

### System Dependencies

- Linux block device (e.g., `/dev/nvme0n1`)
- Linux kernel >= 5.1 (io_uring)
- Appropriate permissions to open block device with O_DIRECT | O_DSYNC (typically root or disk group)

## Success Criteria

- **SC-001**: All `IBlockDevice` methods operate correctly with a raw block device — read-after-write returns identical data for all supported block sizes, with writes durable by `O_DSYNC`.
- **SC-002**: Component auto-detects device size via `BLKGETSIZE64` when `num_blocks=0`.
- **SC-003**: Component rejects non-block-device paths, invalid block sizes, and out-of-range LBAs with appropriate typed errors.
- **SC-004**: Multiple clients can connect and perform independent IO operations concurrently via the actor-serialized event loop.
- **SC-005**: Async operations with timeout produce `Completion::Timeout` when deadlines expire.
- **SC-006**: Feature-gated telemetry produces accurate `TelemetrySnapshot` values when enabled.
- **SC-007**: Criterion benchmarks produce stable measurements for sync IO latency and throughput at varying block counts.
- **SC-008**: Component can be used as a drop-in replacement for `block-device-spdk-nvme` in any code consuming the `IBlockDevice` interface.
- **SC-009**: All unit tests pass with `cargo test -p block-device-kernel` without requiring hardware (unit tests use mocked paths). Integration tests (marked `#[ignore]`) pass on systems with real block devices.

## Implementation Notes

- The component uses `io_uring` exclusively — unlike `block-device-filesys`, there is no `pread`/`pwrite` fallback.
- Write durability relies on `O_DSYNC` at the fd level rather than explicit per-write `fdatasync` calls.
- The `wait_for_cqe` method blocks in a loop for sync operations, processing any other async CQEs that arrive in the meantime.
- CQEs with `user_data & (1 << 63) != 0` are treated as internal fsync markers and silently ignored.
- The `on_idle()` actor method returns `false` only when shutdown is requested; it returns `true` while clients or inflight ops exist to keep the actor thread alive.
- `IBlockDeviceAdmin` is implemented as a passthrough shim — `set_pci_address`, `set_actor_cpu`, `signal_stop`, and `detach_controller` are intentional no-ops since these hardware concepts don't apply.
- The component does NOT support O_DIRECT fallback — if O_DIRECT cannot be set (e.g., on tmpfs), initialization fails.
