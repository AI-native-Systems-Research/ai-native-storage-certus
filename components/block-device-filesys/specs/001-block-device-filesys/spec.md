# Feature Specification: Block Device Filesys Component

**Feature Branch**: `001-block-device-filesys`

**Created**: 2026-06-04

**Status**: Draft

**Input**: User description: "Create a new block-device component, block-device-filesys, that exports the IBlockDevice interface and has a receptacle for ILogger. The component is a substitute for the block-device-spdk-nvme that does not use SPDK NVMe driver, but instead use a file on a kernel-based filesystem to simulate a block device. Create unit tests for the component and benchmarks like those available for block-device-spdk-nvme."

## Clarifications

### Session 2026-06-04

- Q: Write durability semantics — buffered, fdatasync, or configurable? → A: fdatasync after each write (simulates durable block device semantics).
- Q: Backing file initialization — create/error on mismatch, resize, or require pre-existing? → A: Create if absent using fallocate to full size; error if existing file has wrong size.
- Q: Async IO concurrency model — sequential, thread pool, or io_uring? → A: Use io_uring for actual kernel-level async file IO.
- Q: DmaBuffer handling — direct slice access, copy to aligned buffer, or custom allocator? → A: Access DmaBuffer byte slice directly for pread/pwrite/io_uring ops.
- Q: io_uring Rust dependency — io-uring crate, raw syscalls, or high-level runtime? → A: Use the `io-uring` crate (tokio-rs/io-uring) as a thin safe wrapper.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Block IO via File-Backed Device (Priority: P1)

A developer needs a block device component that implements the same IBlockDevice interface as block-device-spdk-nvme but without requiring NVMe hardware or SPDK. They configure block-device-filesys with a path to a backing file, initialize the component, connect a client, and perform synchronous and asynchronous read/write operations at the block level.

**Why this priority**: This is the core functionality — without basic block IO the component has no value. It enables development and testing of higher-level components (e.g., extent-manager) without NVMe hardware.

**Independent Test**: Can be fully tested by creating a temporary backing file, initializing the component, performing read/write operations, and verifying data integrity.

**Acceptance Scenarios**:

1. **Given** a configured file path and block size, **When** the component is initialized, **Then** a backing file is created via fallocate at the specified path with the exact configured size, and the component transitions to an operational state. If the file already exists but has a different size, initialization MUST return an error.
2. **Given** an initialized component with a connected client, **When** the client sends a WriteSync command followed by a ReadSync at the same LBA, **Then** the data read back matches the data written, and the write is durable (fdatasync completed before WriteSync returns).
3. **Given** an initialized component, **When** the client sends ReadAsync and WriteAsync commands, **Then** the operations are submitted to io_uring and completions are delivered on the completion channel with correct OpHandles and success results.
4. **Given** an initialized component, **When** the client sends a WriteZeros command, **Then** reading the zeroed LBA range returns all-zero data.

---

### User Story 2 - Drop-In Replacement for SPDK Block Device (Priority: P2)

A developer working on higher-level storage components (e.g., extent-manager, applications) needs to switch between block-device-spdk-nvme and block-device-filesys without code changes. They use the same IBlockDevice interface methods (connect_client, sector_size, num_sectors, etc.) and receive identical Command/Completion channel semantics.

**Why this priority**: API compatibility ensures the component is truly a substitute; without it the component cannot serve its stated purpose.

**Independent Test**: Can be verified by writing integration tests that exercise all IBlockDevice methods and confirm the same behavior as block-device-spdk-nvme (minus NVMe-specific operations like NsCreate/NsDelete).

**Acceptance Scenarios**:

1. **Given** the block-device-filesys component, **When** queried via IBlockDevice methods, **Then** sector_size, num_sectors, block_size, max_queue_depth, and max_transfer_size return valid values consistent with the backing file configuration.
2. **Given** the block-device-filesys component, **When** the component is bound via the component-framework receptacle mechanism, **Then** IBlockDevice can be queried via IUnknown and used identically to block-device-spdk-nvme.

---

### User Story 3 - Performance Benchmarking (Priority: P3)

A developer needs to benchmark the file-backed block device to establish baseline performance characteristics and detect regressions. They run Criterion benchmarks measuring command construction latency, sync IO throughput, and batch operation throughput — analogous to the benchmarks in block-device-spdk-nvme.

**Why this priority**: Performance measurement is essential for a storage component, but the component must function correctly first.

**Independent Test**: Can be verified by running `cargo bench` and confirming that benchmark results are produced for latency and throughput groups.

**Acceptance Scenarios**:

1. **Given** the component crate, **When** `cargo bench` is run, **Then** Criterion benchmarks execute for command construction latency and batch construction throughput.
2. **Given** the component crate with benchmark feature enabled, **When** `cargo bench` is run, **Then** benchmarks measure actual file-backed IO latency and throughput at varying block counts and queue depths.

---

### Edge Cases

- What happens when the backing file path is invalid or the directory does not exist? Component initialization MUST return a typed error.
- What happens when a read/write targets an LBA beyond the configured device size? The component MUST return an LbaOutOfRange error.
- What happens when the backing file is deleted while the component is running? IO operations MUST return appropriate errors rather than panicking.
- What happens when multiple clients connect simultaneously? Each client MUST receive independent channel pairs and operations MUST be serialized by the actor.
- What happens when an existing backing file has a different size than configured? Initialization MUST return a size-mismatch error (never silently resize).
- What happens when io_uring submission queue is full? The actor MUST back-pressure by waiting for completions before submitting new operations.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Component MUST implement IBlockDevice as defined in `components/interfaces/src/iblock_device.rs`.
- **FR-002**: Component MUST declare a receptacle for ILogger and log operations at debug level and errors at error level.
- **FR-003**: Component MUST use `define_component!` and `define_interface!` macros from the component-framework.
- **FR-004**: Component MUST NOT expose any public functions outside its interface definitions.
- **FR-005**: Component MUST use a regular file on a Linux filesystem as the backing store for block data.
- **FR-006**: Component MUST support configurable block size (default 512 bytes) and device capacity (number of blocks).
- **FR-007**: Component MUST support synchronous read/write (ReadSync, WriteSync) via pread/pwrite syscalls, with fdatasync after each write to guarantee durability.
- **FR-008**: Component MUST support asynchronous read/write (ReadAsync, WriteAsync) using io_uring for kernel-level async IO, with timeout handling and OpHandle tracking.
- **FR-009**: Component MUST support WriteZeros by writing zero-filled blocks to the backing file with fdatasync.
- **FR-010**: Component MUST support BatchSubmit by executing operations sequentially within the batch.
- **FR-011**: Component MUST support AbortOp for in-flight async operations (cancel pending io_uring submissions where possible).
- **FR-012**: Component MUST support NsProbe returning a single namespace with the configured geometry.
- **FR-013**: Component MUST use the actor model (dedicated thread) with lock-free channel communication for IO processing. The actor runs an io_uring event loop for async operations.
- **FR-014**: Component MUST provide Criterion benchmarks for latency and throughput measurement.
- **FR-015**: All public API items MUST have documentation tests.
- **FR-016**: On initialization, component MUST create the backing file via fallocate if absent, or open it if it exists with the exact expected size. Size mismatch MUST produce an error.
- **FR-017**: Component MUST access DmaBuffer byte slices directly (via existing accessor methods) for all IO operations — no intermediate copies.
- **FR-018**: Component MUST depend on the `io-uring` crate (tokio-rs/io-uring) for async file IO. Minimum kernel version: 5.6.

### Key Entities

- **BlockDeviceFilesysComponent**: The component struct, created via `define_component!`. Owns the backing file handle and device configuration.
- **FilesysActor**: The actor thread that processes Command messages from client channels and performs file IO via an io_uring event loop.
- **DeviceConfig**: Configuration struct holding file path, block size, and number of blocks.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All IBlockDevice methods operate correctly with a file-backed store — read-after-write returns identical data for all supported block sizes, with writes guaranteed durable via fdatasync.
- **SC-002**: Component handles 100 concurrent operations per second from a single client without data corruption.
- **SC-003**: IO latency for 4KB blocks on local filesystem is under 1ms for synchronous operations (typical SSD-backed filesystem).
- **SC-004**: All unit tests pass with `cargo test` without requiring special hardware, root privileges, or external dependencies beyond a writable temporary directory.
- **SC-005**: Criterion benchmarks produce stable, reproducible measurements with coefficient of variation under 15%.
- **SC-006**: Component can be used as a drop-in replacement for block-device-spdk-nvme in any code that consumes only the IBlockDevice interface.

## Assumptions

- Backing file resides on a local Linux filesystem (ext4, XFS, or similar) — network filesystems are out of scope.
- The backing file is pre-allocated via fallocate to full configured size at initialization to avoid filesystem fragmentation.
- NVMe-specific admin operations (NsCreate, NsDelete, NsFormat, ControllerReset) will return NotSupported errors since they have no meaningful file-backed equivalent.
- The component operates in a single-namespace mode (ns_id=1) since there is no hardware namespace concept.
- DMA buffer byte slices are accessed directly for IO operations — no intermediate copy or custom allocator required.
- The `telemetry` feature gate behavior matches block-device-spdk-nvme (returns FeatureNotEnabled when not compiled in).
- Kernel version >= 5.6 is required for io_uring support (RHEL 9 ships kernel 5.14, satisfying this).
- The `io-uring` crate (tokio-rs/io-uring) is the sole external dependency for async IO.
