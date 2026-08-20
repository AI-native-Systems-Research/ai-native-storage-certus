# Feature Specification: Block Device Filesys Component

**Feature Branch**: `001-block-device-filesys`

**Created**: 2026-06-04

**Status**: Draft

**Last Synced**: 2026-08-20 (Phase B spec-sync) — BACKFILL of FR-015 (`create()`'s doc example is intentionally ` ```ignore `, not runnable; only `DeviceConfig::new` is runnable) and BACKFILL-UNSPECCED of FR-023 (reserved `pub(crate)` config mutators `set_file_path`/`set_block_size`/`set_num_blocks`). No ALIGN tasks; code unchanged. See `.specify/sync/apply-report.md`. Prior sync — 2026-08-07 (drift sweep on branch `sync/spec-drift-sweep-20260807`) — the FR-019 telemetry-latency defect (`record_op(0, …)` at every call site) was **fixed in code** on the branch (per-op start time captured before the blocking IO, elapsed ns recorded, including the sync-fallback paths and async completion), so latency accounting now works; softened FR-015 (`open_or_create_backing_file` doc-example claim), extended the US2 device-info scenario, added FR-021 (device-info methods `numa_node`/`nvme_version`/`num_io_queues`/`max_transfer_size`/`read_write_stats`) and FR-022 (`FlushSync` → real `fdatasync`). See `.specify/sync/apply-report.md`.

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

1. **Given** the block-device-filesys component, **When** queried via IBlockDevice methods, **Then** sector_size, num_sectors, block_size, max_queue_depth, and max_transfer_size return valid values consistent with the backing file configuration. `max_transfer_size` returns `block_size * 256`, `num_io_queues` returns 1, `numa_node` returns `-1`, and `nvme_version` returns `"N/A (file-backed)"` (see FR-021 for why these last three carry fixed placeholder values).
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
- What happens when one client's completion channel is full while other clients are also awaiting completions? *(Backfilled from implementation)* The full client's completions are buffered in a per-client FIFO backlog and retried each poll cycle; delivery to other, non-full clients MUST NOT be blocked or delayed by the backlogged client.
- What happens when an existing backing file has a different size than configured? Initialization MUST return a size-mismatch error (never silently resize).
- What happens when io_uring submission queue is full? The actor MUST back-pressure by waiting for completions before submitting new operations.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Component MUST implement IBlockDevice as defined in `components/interfaces/src/iblock_device.rs`.
- **FR-002**: Component MUST declare a receptacle for ILogger. Routine lifecycle events (client connect, client disconnect) are logged at debug level. Initialization events are logged at info level. Errors are returned as results rather than logged at error level in most IO paths. Unexpected environment/degradation conditions — the io_uring-ring-creation fallback to synchronous IO, and the fsync-SQE-push-failure path on the async write path — are logged at warn level. io_uring submission-queue-full conditions on the ReadAsync/WriteAsync hot paths are surfaced directly to the caller as an error `Completion` (`NotInitialized("io_uring submission queue full")`) rather than additionally logged, to avoid duplicate reporting of an error the client already receives.
- **FR-003**: Component MUST use `define_component!` and `define_interface!` macros from the component-framework.
- **FR-004**: The component's primary public API is through its interface definitions (`IBlockDevice`, `IBlockDeviceAdmin`). The `config` module (containing `DeviceConfig` and `open_or_create_backing_file`) is permitted as part of the public API. Additionally, `create()`, `initialize()`, and `shutdown()` are public convenience methods on the concrete component type for direct usage (they mirror the `IBlockDeviceAdmin` trait methods).
- **FR-005**: Component MUST use a regular file on a Linux filesystem as the backing store for block data.
- **FR-006**: Component MUST support configurable block size and device capacity (number of blocks), both supplied explicitly at construction via `create(file_path, block_size, num_blocks)` — there is no implicit default value applied by the component. `block_size` MUST be a power of 2 with an enforced minimum of 512 bytes (validated by `DeviceConfig::new`); values below the minimum are rejected with a configuration error.
- **FR-007**: Component MUST support synchronous read/write (ReadSync, WriteSync) via pread/pwrite syscalls with durable write semantics. The backing file is opened with O_DIRECT | O_SYNC to bypass the page cache and guarantee write-through durability. An explicit fdatasync is issued after each write as a belt-and-suspenders guarantee. On filesystems that do not support O_DIRECT (e.g., tmpfs, detected via EINVAL on first open), the component falls back to buffered IO (without O_SYNC) with fdatasync-only durability. This fallback warning is printed directly to stderr via `eprintln!` by `try_open_direct`/`open_or_create_backing_file` in the `config` module rather than routed through the `ILogger` receptacle, because this code executes during initialization before the actor thread (and the logger reference it holds) exists.
- **FR-008**: Component MUST support asynchronous read/write (ReadAsync, WriteAsync) using io_uring for kernel-level async IO, with timeout handling and OpHandle tracking. Async writes use IO_LINK to chain a write SQE with an fdatasync SQE for atomic durable completion. If io_uring is unavailable at runtime, the actor falls back to synchronous pread/pwrite with fdatasync.
- **FR-009**: Component MUST support WriteZeros by writing zero-filled blocks to the backing file with fdatasync.
- **FR-010**: Component MUST support BatchSubmit by executing operations sequentially within the batch.
- **FR-011**: Component MUST support AbortOp for in-flight async operations (cancel pending io_uring submissions where possible).
- **FR-012**: Component MUST support NsProbe returning a single namespace with the configured geometry.
- **FR-013**: Component MUST use the actor model (dedicated thread) with lock-free channel communication for IO processing. The actor runs an io_uring event loop for async operations.
- **FR-014**: Component MUST provide Criterion benchmarks for latency and throughput measurement.
- **FR-015**: Key public API items (constructors, configuration types) MUST have documentation examples. `DeviceConfig::new` has a runnable doc example (` ``` `) that exercises both valid and invalid configurations and is therefore compiled and executed by `cargo test`. The `create()` constructor has an illustrative doc example marked ` ```ignore ` — it documents the calling convention but is intentionally NOT compiled or run, because invoking `create()` in a doctest would allocate a real backing file on disk. *(Corrected 2026-08-20 — the prior text claimed `create()` also had a "runnable" doc example; the code at `src/lib.rs:77` is an ` ```ignore ` block, which is the intended reality, so the requirement is aligned to describe `create()`'s example as illustrative-only. Verified against `src/lib.rs:77-81` and `src/config.rs:42-57`.)* *(Corrected 2026-08-07 — an earlier revision also listed `open_or_create_backing_file`; that function has a prose doc comment but no runnable example code block, so it is carved out here alongside the interface/lifecycle methods.)* `open_or_create_backing_file`, the interface method implementations (`IBlockDevice` impl block), and the lifecycle methods (`initialize`, `shutdown`) do not currently have individual doc examples — these are covered by integration tests instead.
- **FR-016**: On initialization, component MUST create the backing file via fallocate if absent, or open it if it exists with the exact expected size. Size mismatch MUST produce an error.
- **FR-017**: Component MUST access DmaBuffer byte slices directly (via existing accessor methods) for all IO operations — no intermediate copies.
- **FR-018**: Component MUST depend on the `io-uring` crate (tokio-rs/io-uring) for async file IO. Minimum kernel version: 5.6.
- **FR-019**: *(Backfilled from implementation)* When compiled with the `telemetry` feature, the component MUST maintain a per-instance, lock-free `TelemetryStats` collector (atomics-based, no locking on the hot path) tracking: total operation count, minimum/maximum/mean per-op latency in nanoseconds, total bytes transferred, and mean throughput in MB/s (total bytes transferred divided by wall-clock seconds elapsed since the collector was created). `telemetry()` returns a `TelemetrySnapshot` populated from this collector, or a `FeatureNotEnabled` error when the feature is not compiled in. *(Updated 2026-08-07 — the previously-documented latency defect is now **fixed**: each call site captures a `start: Instant` immediately before the blocking IO (sync `pread`/`pwrite`, the io_uring sync-fallback paths, and `WriteZeros`) or on the `InflightOp` at async submit, and records `start.elapsed()` on completion. `min_latency_ns`/`max_latency_ns`/`mean_latency_ns` now reflect real per-op elapsed time. The fix is staged on branch `sync/spec-drift-sweep-20260807` — see `.specify/sync/apply-report.md`.)* Operation-count, byte-count, and throughput accounting were already implemented correctly.
- **FR-020**: *(Backfilled from implementation)* Completion delivery to clients MUST be non-blocking for the actor thread. Each `ClientSession` maintains a FIFO backlog (`pending: VecDeque<Completion>`); when a client's completion channel is full, the completion is buffered rather than blocking the actor, and buffered completions are retried in FIFO order (oldest first, stopping at the first that still cannot be sent) on each subsequent poll cycle via `ClientSession::flush_pending`. This prevents a slow or stalled client from head-of-line-blocking completion delivery to other connected clients.

- **FR-021**: *(Backfilled 2026-08-07 — documents interface-required device-info methods the original spec omitted.)* Because `IBlockDevice` is shared with `block-device-spdk-nvme`, the component MUST implement the full device-info surface, including fields that have no meaningful value for a file-backed device: `numa_node()` MUST return `-1` (no device-to-NUMA affinity is modeled), `nvme_version()` MUST return the fixed string `"N/A (file-backed)"` (there is no NVMe controller), `num_io_queues()` MUST return `1`, `max_transfer_size()` MUST return `block_size * 256`, and `read_write_stats()` MUST return a well-formed `ReadWriteStats` (currently zero-initialized — per-direction counters are not separately tracked; aggregate telemetry under FR-019 is the supported path). These are stable, intentional placeholder constants, not runtime-discovered values.

- **FR-022**: *(Backfilled 2026-08-07 — the shared `IBlockDevice` gained a `FlushSync` command after this spec was written; documents the implemented handler.)* The component MUST handle the `FlushSync { ns_id }` command by issuing a full `fdatasync(2)` on the backing file descriptor and replying with `Completion::FlushDone { handle, result }`. An invalid `ns_id` (anything other than 1) MUST return `InvalidNamespace` without touching the file; an `fdatasync` failure MUST be surfaced as a `WriteFailed` error. Although individual writes already `fdatasync` for durability (FR-007/FR-008), `FlushSync` provides an explicit, well-defined durability barrier for callers that need one (e.g. the extent-manager volatile-write-cache path).

- **FR-023**: *(Backfilled 2026-08-20 — documents unspecced internal config mutators found at `src/lib.rs:95-109`.)* The component MAY retain crate-private (`pub(crate)`) configuration mutators `set_file_path`, `set_block_size`, and `set_num_blocks` that overwrite the corresponding config fields after construction. These are reserved internal helpers, are NOT part of the component's public API (the public configuration path is `create(file_path, block_size, num_blocks)` per FR-004/FR-006), and are currently unused (`#[allow(dead_code)]`). Because they are not public and impose no observable external behavior, they carry no acceptance scenario and are outside the functional contract; they are documented here only to record that their presence is intentional (retained for anticipated internal reconfiguration) rather than accidental. They MUST NOT be promoted to public API without a corresponding spec change and validation (a value set via `set_block_size`/`set_num_blocks` is not re-validated against `DeviceConfig::new` invariants at set time, only at `initialize()`).

### Key Entities

- **BlockDeviceFilesysComponent**: The component struct, created via `define_component!`. Owns the backing file handle and device configuration.
- **FilesysActor**: The actor thread that processes Command messages from client channels and performs file IO via an io_uring event loop.
- **DeviceConfig**: Configuration struct holding file path, block size, and number of blocks.
- **ClientSession**: *(Backfilled from implementation)* Per-client channel state held by the actor; owns a FIFO backlog (`pending`) of completions that could not be delivered immediately, retried each poll cycle so one stalled client cannot block delivery to others.
- **TelemetryStats**: *(Backfilled from implementation)* Feature-gated (`telemetry`), atomics-based collector of op count, min/max/mean latency, total bytes, and mean throughput, sampled into a `TelemetrySnapshot` on demand.

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
