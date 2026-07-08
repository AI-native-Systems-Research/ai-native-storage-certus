# Feature Specification: SPDK NVMe Block Device Component

**Feature Branch**: `001-block-device-spdk-nvme`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `block-device-spdk-nvme` component provides a high-performance NVMe block device driver using SPDK (Storage Performance Development Kit) for direct userspace NVMe controller access, bypassing the kernel storage stack entirely. It is built on the Certus actor-based component model, where each component instance owns a single NVMe controller identified by PCI address and runs a dedicated actor thread NUMA-pinned to the controller's memory domain.

Clients interact with the component through per-client SPSC (Single Producer Single Consumer) channel pairs: an ingress channel for submitting IO commands and a callback channel for receiving asynchronous completion notifications. The component supports synchronous and asynchronous read/write operations, write-zeros, batch IO submission, operation abort, NVMe namespace management (probe, create, format, delete), hardware controller reset, and optional compile-time telemetry for IO latency/throughput statistics. The queue pair pool uses a shallowest-fit depth selection heuristic to minimize latency for small IO while providing throughput capacity for large batches.

## User Scenarios & Testing

### User Story 1 - Synchronous Block IO (Priority: P1)

As a storage consumer, I want to perform synchronous read and write operations on NVMe namespaces so that I can store and retrieve data with guaranteed completion before the call returns.

**Acceptance Scenarios**:
- Given an initialized block device component with wired receptacles, when a client sends a `WriteSync` command with a DMA buffer to a valid LBA, then the actor writes the data to the NVMe device and sends a `WriteDone` completion with `Ok(())`.
- Given an initialized block device component, when a client sends a `ReadSync` command to an LBA previously written, then the actor reads the data into the provided buffer and sends a `ReadDone` completion, and the buffer contents match the written data.
- Given an initialized block device component, when a client sends a `WriteSync` to an LBA beyond the namespace boundary, then the actor sends a `WriteDone` completion with `Err(NvmeBlockError::LbaOutOfRange(...))`.

### User Story 2 - Asynchronous Block IO with Timeout (Priority: P1)

As a latency-sensitive consumer, I want to perform asynchronous reads and writes with configurable timeouts so that I can overlap IO with computation and bound worst-case waiting times.

**Acceptance Scenarios**:
- Given an initialized component, when a client sends a `ReadAsync` command with `timeout_ms=5000`, then the actor submits the read to the NVMe hardware and eventually sends a `ReadDone` completion with the assigned `OpHandle` and caller-supplied `tag`.
- Given a read that does not complete within `timeout_ms`, when the actor's timeout check fires, then the client receives a `Completion::Timeout { handle }` and the pending operation is removed.
- Given an `ENOMEM` return from SPDK on async submit (transient queue saturation), when the actor retries with backpressure polling (up to `min(timeout_ms, 1000ms)`), then the submission succeeds without returning a spurious error to the client.

### User Story 3 - Multi-Client Concurrent Access (Priority: P1)

As a system with multiple concurrent consumers, I want independent clients to perform IO simultaneously through separate channels so that one client's latency does not block another's.

**Acceptance Scenarios**:
- Given an initialized component, when multiple clients connect via `connect_client()`, then each receives independent SPSC channel pairs with 256-slot capacity.
- Given multiple connected clients submitting IO concurrently, when the actor polls all clients in round-robin order (rotating start index), then no single client starves others.
- Given a client that drops its channel sender, when the actor detects `ChannelError::Closed`, then the client is silently removed without affecting other clients.

### User Story 4 - Namespace Management (Priority: P2)

As a storage administrator, I want to probe, create, format, and delete NVMe namespaces so that I can partition and configure the NVMe device for multi-tenant workloads.

**Acceptance Scenarios**:
- Given an initialized component, when a client sends `NsProbe`, then the actor returns a `NsProbeResult` listing all active namespaces with their `ns_id`, `num_sectors`, and `sector_size`.
- Given unallocated capacity on the controller, when a client sends `NsCreate { size_sectors: N }`, then the actor creates and attaches a namespace of N sectors and returns `NsCreated { ns_id }`.
- Given `NsCreate { size_sectors: 0 }`, when there is remaining capacity, then the actor uses all unallocated NVM capacity for the new namespace.
- Given an existing namespace, when a client sends `NsFormat { ns_id, lbaf }`, then the actor formats the namespace (erasing all data), resets the controller, and returns `NsFormatted { ns_id }`.
- Given an existing namespace, when a client sends `NsDelete { ns_id }`, then the actor deletes the namespace and returns `NsDeleted { ns_id }`.

### User Story 5 - Controller Reset (Priority: P2)

As a recovery mechanism, I want to issue a hardware controller reset so that the device can recover from stuck state or firmware errors.

**Acceptance Scenarios**:
- Given an initialized component with multiple clients having pending ops, when one client sends `ControllerReset`, then all pending ops across ALL clients receive `Completion::Error` with `Aborted("cancelled due to controller reset")`, and the requesting client receives `Completion::ResetDone { result: Ok(()) }` after successful reset.
- Given a controller reset that fails (SPDK returns non-zero), then the requesting client receives `ResetDone { result: Err(...) }`.

### User Story 6 - Operation Abort (Priority: P2)

As a client, I want to abort an in-flight async operation so that I can cancel work that is no longer needed.

**Acceptance Scenarios**:
- Given a pending async operation with handle H, when the client sends `AbortOp { handle: H }`, then the actor removes the pending operation and sends `Completion::AbortAck { handle: H }`.
- Given an abort for a handle that is no longer pending (already completed or timed out), then the actor still sends `AbortAck` without error.

### User Story 7 - Batch Submission (Priority: P2)

As a throughput-oriented consumer, I want to submit multiple IO operations as a batch so that they are all routed to the same deep queue pair for optimal hardware utilization.

**Acceptance Scenarios**:
- Given a `BatchSubmit { ops: [WriteAsync, WriteAsync, ReadAsync] }`, when the actor dispatches the batch, then all sub-operations are submitted to the same queue pair (selected by batch size) and completions arrive individually on the callback channel.

### User Story 8 - Telemetry (Priority: P3)

As a performance engineer, I want to collect per-operation latency and throughput statistics so that I can monitor device performance without external instrumentation.

**Acceptance Scenarios**:
- Given the component compiled with `--features telemetry`, when IO operations complete, then `IBlockDevice::telemetry()` returns a `TelemetrySnapshot` with accurate `total_ops`, `min_latency_ns`, `max_latency_ns`, `mean_latency_ns`, `mean_throughput_mbps`, and `elapsed_secs`.
- Given the component compiled WITHOUT the `telemetry` feature, when `telemetry()` is called, then it returns `Err(NvmeBlockError::FeatureNotEnabled(...))`.
- Given N independently-timed operations, when compared to the telemetry snapshot's mean latency, then the values agree within 5%.

### User Story 9 - Device Introspection (Priority: P2)

As a consumer, I want to query device properties so that I can size buffers and validate IO parameters before submission.

**Acceptance Scenarios**:
- Given an initialized component, when `block_size()`, `max_queue_depth()`, `num_io_queues()`, `max_transfer_size()`, `numa_node()`, and `nvme_version()` are called, then they return values populated from the attached controller.
- Given a component that has NOT been initialized, when device info methods are called, then they return safe defaults (0, 512, -1, "unknown") instead of panicking.

### User Story 10 - Graceful Shutdown (Priority: P1)

As a system operator, I want a clean shutdown sequence so that no IO is lost and the NVMe controller is properly detached.

**Acceptance Scenarios**:
- Given an initialized component with clients, when `signal_stop()` is called, then the actor's command channel is closed and its poll loop exits.
- Given a stopped actor, when `shutdown()` is called, then the actor thread is joined, all in-flight SPDK operations are drained (up to 5s timeout), pending ops receive `Error(Aborted)` completions, and the controller is parked.
- Given a parked controller, when `detach_controller()` is called, then `spdk_nvme_detach` releases the controller back to the kernel and all queue pairs are freed first.

## Requirements

### Functional Requirements

- **FR-001**: The component SHALL implement the `IBlockDevice` interface providing `connect_client()`, `sector_size()`, `num_sectors()`, `max_queue_depth()`, `num_io_queues()`, `max_transfer_size()`, `block_size()`, `numa_node()`, `nvme_version()`, and `telemetry()` methods.
- **FR-002**: The component SHALL implement the `IBlockDeviceAdmin` interface providing `set_pci_address()`, `set_actor_cpu()`, `initialize()`, `signal_stop()`, `shutdown()`, and `detach_controller()` methods.
- **FR-003**: The component SHALL declare two receptacles: `spdk_env: ISPDKEnv` and `logger: ILogger`, both of which must be wired before `initialize()`.
- **FR-004**: `connect_client()` SHALL create two per-client SPSC channels of capacity 256 (ingress for commands, callback for completions) and register the client session with the actor.
- **FR-005**: The actor SHALL process `ReadSync` commands by submitting a synchronous SPDK read, polling the queue pair until the completion callback fires, and sending a `ReadDone` completion on the callback channel.
- **FR-006**: The actor SHALL process `WriteSync` commands by submitting a synchronous SPDK write, polling the queue pair until the completion callback fires, and sending a `WriteDone` completion.
- **FR-007**: The actor SHALL process `ReadAsync` commands by validating the namespace/LBA, submitting an asynchronous SPDK read with a callback context, tracking the operation in `pending_ops` with a TSC-based deadline, and sending a `ReadDone` completion when the SPDK callback fires.
- **FR-008**: The actor SHALL process `WriteAsync` commands following the same pattern as FR-007 but with write semantics.
- **FR-009**: The actor SHALL process `WriteZeros` commands via `spdk_nvme_ns_cmd_write_zeroes` and send a `WriteZerosDone` completion.
- **FR-010**: The actor SHALL validate namespace IDs against discovered active namespaces before any IO operation, returning `NvmeBlockError::InvalidNamespace` if not found.
- **FR-011**: The actor SHALL validate that `lba + num_blocks <= ns.num_sectors` before any IO operation, returning `NvmeBlockError::LbaOutOfRange` if out of bounds.
- **FR-012**: The actor SHALL support `BatchSubmit` by recursively dispatching all sub-commands using a single queue pair selected by total batch size.
- **FR-013**: The actor SHALL support `AbortOp` by removing the pending operation (if present) and sending an `AbortAck` completion regardless.
- **FR-014**: The actor SHALL support `NsProbe` by returning the current namespace list converted to `NamespaceInfo` structs.
- **FR-015**: The actor SHALL support `NsCreate` by calling `spdk_nvme_ctrlr_create_ns` and `spdk_nvme_ctrlr_attach_ns`, refreshing the namespace list on success, and returning the new `ns_id`.
- **FR-016**: The actor SHALL support `NsFormat` by calling `spdk_nvme_ctrlr_format`, issuing a controller reset to refresh identify data, and returning the formatted `ns_id`.
- **FR-017**: The actor SHALL support `NsDelete` by calling `spdk_nvme_ctrlr_delete_ns`, refreshing the namespace list, and returning the deleted `ns_id`.
- **FR-018**: The actor SHALL handle `ControllerReset` by cancelling ALL pending ops across ALL clients with `Aborted` errors, issuing `spdk_nvme_ctrlr_reset`, and sending `ResetDone` to the requesting client.
- **FR-019**: The actor SHALL silently remove disconnected clients (detected via `ChannelError::Closed` on the ingress channel) without affecting other clients.
- **FR-020**: The actor SHALL use a rotating `poll_start_idx` for fair round-robin polling of client channels to prevent head-of-line blocking.
- **FR-021**: The actor SHALL check for timed-out operations approximately every 1ms (throttled by TSC comparison) and send `Completion::Timeout` for any operation past its deadline.
- **FR-022**: On async SPDK submit returning `-ENOMEM`, the actor SHALL retry by polling completions to free qpair slots, up to `min(timeout_ms, 1000ms)`, before reporting failure.
- **FR-023**: `initialize()` SHALL return `NvmeBlockError::NotInitialized` if the `spdk_env` receptacle is not connected.
- **FR-024**: `connect_client()` SHALL return `NvmeBlockError::NotInitialized` if `initialize()` has not been called.
- **FR-025**: The component SHALL expose version "0.2.0" via `IUnknown::version()`.
- **FR-026**: The `on_stop` handler SHALL drain all in-flight SPDK operations (up to 5s), send `Error(Aborted)` to all remaining pending ops, clear client sessions, and park the controller for deferred detach.

### Non-Functional Requirements

- **NFR-001**: The actor thread SHALL be pinned to the NUMA node of the NVMe controller (or an explicit CPU if `set_actor_cpu()` was called) to minimize cross-NUMA memory latency.
- **NFR-002**: Per-client channels SHALL use lock-free SPSC channels with 256-slot capacity for zero-contention communication between client threads and the actor.
- **NFR-003**: DMA buffers SHALL be allocated from SPDK hugepages for zero-copy IO, avoiding kernel page-fault overhead.
- **NFR-004**: The async IO context pool (`ContextPool`) SHALL pre-allocate 340 entries and reuse them to eliminate per-IO heap allocation on the hot path.
- **NFR-005**: The TSC clock SHALL be calibrated once at actor construction (2ms calibration window) and provide sub-20-cycle timing via `rdtsc` for deadline checks and telemetry.
- **NFR-006**: The queue pair pool SHALL maintain 4 queue pairs at depths [4, 16, 64, 256] and select the shallowest queue with sufficient available capacity (low latency for small IO, high throughput for large batches).
- **NFR-007**: When all queue pairs are under pressure, the fallback selection SHALL pick the queue pair with the most available capacity (not unconditionally the deepest) to spread load and reduce ENOMEM probability.
- **NFR-008**: The actor SHALL spin (not park/sleep) whenever clients are connected, as NVMe completions only arrive via `spdk_nvme_qpair_process_completions()` invoked in `on_idle()`.
- **NFR-009**: Sync read/write round-trip p50 latency SHALL remain within the direct NVMe latency envelope (target < 100us for 4KB at QD=1).
- **NFR-010**: Telemetry overhead (when enabled) SHALL be bounded to atomic counter operations (fetch_add, compare_exchange_weak with Relaxed ordering) with no locking on the hot path.
- **NFR-011**: The component SHALL support the `telemetry` feature flag as an opt-in compile-time gate, adding zero overhead when disabled.
- **NFR-012**: Queue pair allocation SHALL set `io_queue_requests = depth * 4` to absorb transient bursts and request splitting without premature ENOMEM.
- **NFR-013**: Platform requirement: Linux only, x86_64 with invariant TSC, Rust stable edition 2021, MSRV 1.75.
- **NFR-014**: All unsafe code blocks SHALL carry `// SAFETY:` justification comments explaining why the invariants are upheld.

## Key Entities

| Entity | Description |
|--------|-------------|
| `BlockDeviceSpdkNvmeComponent` | The top-level component struct created by `define_component!`. Owns receptacles, actor handle, controller info snapshot, client ID counter, and telemetry stats. |
| `BlockDeviceHandler` | The `ActorHandler<ControlMessage>` implementation running on the dedicated actor thread. Owns the NVMe controller, client sessions, pending ops, context pool, and TSC clock. |
| `NvmeController` | Safe wrapper around `*mut spdk_nvme_ctrlr`. Manages probe/attach, namespace discovery, queue pair pool, and detach on drop. |
| `QueuePairPool` | Collection of `QueuePair` instances at standard depths [4, 16, 64, 256]. Provides shallowest-fit selection heuristic. |
| `QueuePair` | A single SPDK NVMe IO queue pair with depth tracking (`in_flight`, `available()`). |
| `ClientSession` | Per-client channel endpoints (ingress_rx, callback_tx) and session ID. Internal to the actor. |
| `ControlMessage` | Actor MPSC messages: `ConnectClient`, `DisconnectClient`. |
| `Command` | Client-to-actor IO commands: `ReadSync`, `WriteSync`, `ReadAsync`, `WriteAsync`, `WriteZeros`, `BatchSubmit`, `AbortOp`, `NsProbe`, `NsCreate`, `NsFormat`, `NsDelete`, `ControllerReset`. |
| `Completion` | Actor-to-client completion notifications: `ReadDone`, `WriteDone`, `WriteZerosDone`, `AbortAck`, `Timeout`, `NsProbeResult`, `NsCreated`, `NsFormatted`, `NsDeleted`, `ResetDone`, `Error`. |
| `ClientChannels` | Return type from `connect_client()`: contains `command_tx: Sender<Command>` and `completion_rx: Receiver<Completion>`. |
| `OpHandle` | Monotonically increasing `u64` assigned at submission time for tracking async operations. |
| `TelemetrySnapshot` | Point-in-time telemetry: `total_ops`, `min/max/mean_latency_ns`, `mean_throughput_mbps`, `elapsed_secs`. |
| `TelemetryStats` | Internal atomic counters for telemetry collection (feature-gated). |
| `TscClock` | Hardware TSC-based clock calibrated against `clock_gettime`. Provides `now()`, `ticks_to_ns()`, `deadline_from_ms()`. |
| `ContextPool` | Slab allocator for `AsyncIoContext` objects, avoiding per-IO heap allocation. |
| `AsyncIoContext` | Per-operation context passed to SPDK completion callbacks via raw pointer. Contains client_id, handle, is_read flag, and telemetry fields. |
| `NvmeBlockError` | Error enum: `FeatureNotEnabled`, `NotInitialized`, `Timeout`, `Aborted`, `InvalidNamespace`, `NotSupported`, `BlockDevice`, `SpdkEnv`, `LbaOutOfRange`, `ClientDisconnected`. |
| `PciAddress` | NVMe controller PCI address (domain, bus, dev, func). |
| `DmaBuffer` | SPDK hugepage-backed memory buffer for zero-copy IO. |

## Dependencies

| Dependency | Type | Purpose |
|------------|------|---------|
| `component-framework` | Workspace crate | `define_component!`, `define_interface!`, Actor, SpscChannel, NUMA topology |
| `component-core` | Workspace crate | `IUnknown`, `ActorHandler`, `ActorHandle`, channel types, binding |
| `component-macros` | Workspace crate | Procedural macros for component/interface definitions |
| `interfaces` | Workspace crate (features=["spdk"]) | `IBlockDevice`, `IBlockDeviceAdmin`, `ILogger`, `ISPDKEnv`, `Command`, `Completion`, `DmaBuffer`, `PciAddress`, `NvmeBlockError` |
| `spdk-sys` | Workspace crate | Raw FFI bindings to SPDK C libraries (NVMe probe/attach/IO/admin commands) |
| `spdk-env` | Workspace crate | Safe SPDK environment initialization, device discovery |
| `crossbeam-channel` | External (0.5) | Used internally for bounded channels |
| `logger` | Workspace crate | `ILogger` implementation for debug/info logging |
| `criterion` | Dev dependency (0.5) | Benchmarking framework for latency/throughput suites |

## Success Criteria

- **SC-001**: Sync read/write round-trip completes within direct NVMe latency envelope (p50 < 100us for 4KB at QD=1 on target hardware).
- **SC-002**: Async timeout completions arrive within 10% + 2ms of the specified timeout value (accounting for 1ms check_timeouts granularity).
- **SC-003**: Multiple clients (4+ threads, 16 ops each) can perform concurrent IO without data corruption or deadlock.
- **SC-004**: Namespace create/delete/format operations succeed on controllers supporting namespace management, with cross-namespace isolation verified.
- **SC-005**: Controller reset cancels all pending operations and delivers appropriate error completions to all connected clients.
- **SC-006**: Telemetry mean latency agrees within 5% of independently measured values when the feature is enabled.
- **SC-007**: Component passes `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --all`, and `cargo doc --no-deps` with zero warnings/failures.
- **SC-008**: Unit tests run without SPDK hardware; integration tests self-skip when hardware is unavailable.

## Implementation Notes

- The `define_component!` macro generates `IUnknown` implementation, receptacle wiring, and interface query support automatically.
- The component uses `Arc<Mutex<Option<NvmeController>>>` as a "parking slot" to ensure the controller outlives the actor thread during shutdown (required because `spdk_nvme_detach` must be called after all actor threads have exited).
- `atexit` is registered in integration tests to call `_exit(0)` before SPDK's own teardown runs, preventing SIGSEGV from SPDK/DPDK cleanup after Arc-leaked components.
- The `OnceLock<Option<&'static SpdkHardwareContext>>` pattern in tests ensures SPDK (a process singleton) is initialized exactly once and shared across all hardware tests.
- The `DmaBuffer` type is `Send` but not `Sync`; read buffers use `Arc<Mutex<DmaBuffer>>` while write buffers use `Arc<DmaBuffer>` (immutable after fill).
- On non-x86_64 architectures, `rdtsc()` falls back to `Instant::now()` for compilation compatibility (not performance-optimized).
- The `io_queue_requests = depth * 4` setting in queue pair opts is critical: it sizes SPDK's internal request tracker pool to absorb request splitting and burst load.
- The `SUBMIT_ENOMEM_MAX_BACKPRESSURE_MS` constant (1000ms) caps the retry loop to prevent indefinite actor spinning in pathological hardware stall scenarios.
