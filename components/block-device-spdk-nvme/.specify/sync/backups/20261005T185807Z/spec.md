# Feature Specification: SPDK NVMe Block Device Component

**Feature Branch**: `001-spdk-nvme-block-device`
**Created**: 2026-04-14
**Status**: Draft
**Last Synced**: 2026-08-27 — backfilled FR-032 (debug-build-only DMA issue-size logging via the `log_dma_issue!` macro; compiled out in release, independent of the `telemetry` feature). Prior sync 2026-08-20 (Spec-Sync Phase B) — re-synced FR-005 (abort contract now fully implemented in mainline, no longer "drafted"), FR-010/SC-005 (`max_transfer_size` is MDTS-derived, not a fixed constant; only `nvme_version` and `numa_id` remain fixed); backfilled unspecced FR-031 (`FlushSync` durability barrier) and a note on the dead `namespace::probe()` helper. Prior sync 2026-08-07 (branch `sync/spec-drift-sweep-20260807`) backfilled FR-004 (tag correlation), FR-005 (buffer-safe abort contract), FR-010/SC-005 (device-info fields), FR-013/SC-007 (NUMA node-0), FR-028 (drain-then-park order), and added FR-030 (`read_write_stats`). Remaining hardware-discovery follow-up (`nvme_version`/`numa_id`) is in `.specify/sync/align-tasks.md` Task BD-2.
**Input**: User description: "SPDK NVMe block device component with actor model, IBlockDevice interface, async IO, namespace management, and telemetry"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Basic Block IO Operations (Priority: P1)

A storage client connects to the block device component and performs
synchronous read and write operations against an NVMe namespace. The
client allocates DMA buffers, submits read/write commands specifying
namespace, LBA offset, and buffer, and receives confirmation of
completion.

**Why this priority**: Read/write is the fundamental operation. Without
it no other functionality is meaningful.

**Independent Test**: Can be verified by connecting a client, writing a
known pattern to a range of LBAs, reading them back, and confirming
data integrity.

**Acceptance Scenarios**:

1. **Given** a connected client with an allocated DMA buffer, **When** the
   client issues a synchronous write followed by a synchronous read at
   the same LBA, **Then** the read returns the exact data that was written.
2. **Given** a connected client, **When** the client issues a read to an
   LBA beyond the namespace capacity, **Then** an error is returned
   without crashing the component.
3. **Given** a connected client, **When** the client issues a write-zeros
   command to a range of LBAs, **Then** a subsequent read of those LBAs
   returns all zeros.
4. **Given** a connected client that has issued writes to a namespace,
   **When** the client issues a synchronous flush (`FlushSync`) for that
   namespace, **Then** a `FlushDone` completion is returned once the
   controller has flushed its volatile write cache, and an invalid
   namespace id yields an error result rather than a crash. (FR-031)

---

### User Story 2 - Asynchronous IO with Timeout and Abort (Priority: P1)

A storage client submits asynchronous read/write operations with a
specified timeout. The component processes these in the background and
signals completion via the callback channel. If a timeout is exceeded,
an error is reported. The client can also abort an in-flight
asynchronous operation.

**Why this priority**: Async IO is essential for achieving high
throughput with deep queue depths in production workloads.

**Independent Test**: Can be verified by submitting async writes,
confirming callback completions, testing with an artificially short
timeout to trigger timeout errors, and issuing abort requests.

**Acceptance Scenarios**:

1. **Given** a connected client, **When** the client submits an
   asynchronous write with a valid timeout, **Then** a completion
   callback is received on the callback channel within the timeout.
2. **Given** a connected client, **When** an async operation does not
   complete before the timeout, **Then** an error is delivered via the
   callback channel.
3. **Given** a connected client with an in-flight async operation,
   **When** the client issues an abort, **Then** the operation is
   cancelled and an abort-acknowledged callback is received.

---

### User Story 3 - Batch Operations (Priority: P2)

A storage client submits a batch of IO operations in a single request.
The component processes the batch, exploiting multiple NVMe IO queues
to minimize latency for the given batch size. Completions for the
entire batch are reported.

**Why this priority**: Batch submission is critical for throughput
optimization and efficient use of NVMe IO queue depth.

**Independent Test**: Can be verified by submitting a batch of writes,
confirming all completions, and measuring that throughput exceeds the
sum of individual synchronous operations.

**Acceptance Scenarios**:

1. **Given** a connected client, **When** the client submits a batch of
   N write operations, **Then** N completion callbacks are received.
2. **Given** a batch of operations with mixed valid and out-of-range
   LBAs, **When** submitted, **Then** valid operations succeed and
   invalid operations return individual errors.

---

### User Story 4 - NVMe Namespace Management (Priority: P2)

An administrator probes the controller to discover existing namespaces,
creates new namespaces, formats namespaces, and deletes namespaces.

**Why this priority**: Namespace management is required for device
provisioning and must be available before production deployment.

**Independent Test**: Can be verified by probing for existing
namespaces, creating a new namespace, formatting it, verifying it
appears in subsequent probes, and then deleting it.

**Acceptance Scenarios**:

1. **Given** an initialized component, **When** the client issues a
   namespace probe, **Then** a list of existing namespaces with their
   properties is returned.
2. **Given** an initialized component, **When** the client creates a new
   namespace, **Then** it appears in subsequent probe results.
3. **Given** a namespace, **When** the client formats it, **Then** all
   data in that namespace is erased.
4. **Given** a namespace, **When** the client deletes it, **Then** it no
   longer appears in probe results.

---

### User Story 5 - Device Information and Telemetry (Priority: P3)

A monitoring client queries the device for its capabilities (capacity,
max queue depth, IO queue count, max transfer size, block size, NUMA
id, NVMe version) via the IBlockDevice interface. When compiled with
the `telemetry` feature, the client also retrieves IO latency
statistics (min, max, mean), total operation count, and mean
throughput.

**Why this priority**: Observability and capacity planning depend on
device introspection, but core IO must work first.

**Independent Test**: Can be verified by querying device info and
confirming values match known hardware properties. Telemetry can be
tested by running IO, then verifying statistics are populated (with
feature) or return an error (without feature).

**Acceptance Scenarios**:

1. **Given** an initialized component, **When** the client queries
   device information, **Then** accurate values for capacity, max queue
   depth, IO queue count, max transfer size, block size, NUMA id, and
   NVMe version are returned.
2. **Given** a component compiled with the `telemetry` feature, **When**
   the client runs IO and then queries telemetry, **Then** min/max/mean
   latency, total operation count, and mean throughput are returned.
3. **Given** a component compiled without the `telemetry` feature,
   **When** the client queries telemetry, **Then** an error is returned.

---

### User Story 6 - Controller Hardware Reset (Priority: P3)

An administrator issues a hardware reset command to the NVMe
controller. The component resets the controller and reinitializes it
for continued operation.

**Why this priority**: Hardware reset is a recovery mechanism, not part
of normal operation.

**Independent Test**: Can be verified by issuing a reset, confirming
the controller comes back online, and performing a read/write to
confirm functionality is restored.

**Acceptance Scenarios**:

1. **Given** an initialized component, **When** the client issues a
   controller reset, **Then** the controller is reset and subsequently
   available for IO operations.
2. **Given** an in-flight async operation, **When** a controller reset
   is issued, **Then** pending operations are cancelled with errors and
   the controller resets cleanly.

---

### Edge Cases

- When a client disconnects while async operations are in-flight, the component cancels all in-flight operations for that client and silently discards completions.
- What happens when DMA buffer memory is too small for the requested IO size?
- Concurrent namespace management operations from multiple clients are serialized through the actor thread; the actor processes them in the order they are received from the polled channels.
- What happens when the SPDK environment fails to initialize?
- What happens when the NVMe controller is not present or not responding?

## Clarifications

### Session 2026-04-14

- Q: How are async operations identified for abort and completion correlation? → A: Component assigns a unique operation handle on submission; client uses it for abort and completion correlation.
- Q: What happens when a client disconnects while async operations are in-flight? → A: Cancel all in-flight operations for the disconnected client; discard completions silently.
- Q: How are concurrent namespace management operations from multiple clients handled? → A: All namespace operations serialize through the actor thread; natural ordering, no extra locking.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide an IBlockDevice interface for creating and connecting client channels.
- **FR-002**: Each connected client MUST have two shared-memory channels: one for ingress command messages, one for asynchronous completion callbacks.
- **FR-003**: System MUST support synchronous read and write operations with parameters for NVMe namespace id, DmaBuffer, and LBA offset (no timeout — sync ops block until completion).
- **FR-004**: System MUST support asynchronous read and write operations with a timeout value; operations exceeding timeout MUST return an error. The component assigns a unique operation handle to each async op, which it echoes back in the completion. Submission is fire-and-forget over the ingress channel: the handle is NOT returned synchronously at submit time. Clients therefore correlate a completion to its request via the caller-supplied `tag` field (carried on the command and echoed in `Completion::ReadDone`/`WriteDone`), not via the component handle. *(Backfilled 2026-08-07: the original text required returning the handle synchronously at submission; the shipped fire-and-forget channel design makes `tag` the client-side correlation mechanism. The component-assigned handle is used internally for pending-op tracking, abort matching, and timeout delivery — see FR-005, FR-028.)*
- **FR-005**: System MUST support aborting an in-flight asynchronous operation identified by its component-assigned operation handle. Aborting MUST be memory-safe with respect to the pinned DMA buffer: the component MUST NOT release the buffer (drop the `PendingOp`) while the controller may still DMA into it. On an abort request for a still-outstanding op, the actor issues a real NVMe abort for that command (matched by its `cmd_cb_arg` via `spdk_nvme_ctrlr_cmd_abort_ext`), marks the op `aborting`, and keeps the `PendingOp` (and its buffer) alive. The `Completion::AbortAck` is deferred until the original command's completion arrives (aborted-by-request or otherwise), at which point the buffer is released. An abort for an unknown/already-completed handle is acked immediately (idempotent). *(Backfilled 2026-08-07 to document the buffer-lifetime contract. Re-synced 2026-08-20: the defer-until-completion abort contract is now fully implemented in the shipped code — `Command::AbortOp` marks the op `aborting`, retains the `PendingOp` and its buffer, issues a real `spdk_nvme_ctrlr_cmd_abort_ext` matched by `cmd_cb_arg`, and defers `Completion::AbortAck` until the original command's completion arrives, at which point the buffer is released; an unknown handle is acked immediately. See `src/actor.rs:999-1047` (abort dispatch) and `src/actor.rs:543-576` (deferred ack on real completion). The earlier "drafted on branch, requires hardware validation (Task BD-1)" status is superseded — the fix is in the mainline component. *(Anchors re-synced 2026-09-03: code shifted since the 2026-08-27 sync — abort dispatch moved from `972-1020` to `999-1047`, deferred ack from `528-537` to `543-576`; behavior unchanged.)*)*
- **FR-006**: System MUST support a write-zeros operation.
- **FR-007**: System MUST support batch submission of IO operations.
- **FR-008**: System MUST support probing, creating, formatting, and deleting NVMe namespaces.
- **FR-009**: System MUST support controller hardware reset with graceful handling of in-flight operations.
- **FR-010**: System MUST expose device information (capacity, max queue depth, IO queue count, max transfer size, block/sector size, NUMA id, NVMe version) via the IBlockDevice interface. *(Backfilled 2026-08-07; re-synced 2026-08-20 — current implementation status: capacity, sector/block size, max queue depth, IO-queue count, AND `max_transfer_size` are read from the live controller/namespace. `max_transfer_size` is auto-detected from the controller's MDTS via `spdk_nvme_ctrlr_get_max_xfer_size` (`src/controller.rs:169-177`), so it reflects the device's real transfer limit (e.g. 1 MiB); 131072 (128 KiB) is used only as a fallback when MDTS reports 0 ("no limit"). Two fields remain fixed rather than hardware-derived: `nvme_version` returns 1.0.0 (`src/controller.rs:156-161`, not available from the minimal bindings) and `numa_id` returns 0 (`src/lib.rs:333-334`) because controller NUMA discovery is not yet implemented (see FR-013). Populating those two from the real SPDK identify/opts APIs is tracked in align-tasks.md Task BD-2.)*
- **FR-011**: When compiled with the `telemetry` feature, system MUST collect and expose min/max/mean IO latencies, total operation count, and mean throughput. When compiled without the feature, the telemetry API MUST return an error.
- **FR-012**: Each component instance MUST be associated with a single NVMe controller device, configured via `IBlockDeviceAdmin::set_pci_address` and attached via `IBlockDeviceAdmin::initialize` (see FR-021).
- **FR-013**: The actor service thread MUST be pinned to a core in the same NUMA zone as the NVMe controller device. *(Backfilled 2026-08-07 — current implementation pins the actor to a NUMA-local core, but the controller's NUMA node is hardcoded to 0 at probe rather than discovered from the device, so pinning always targets node 0. On a single-node host or a node-0 device this is correct; on a multi-node host with a non-node-0 device the guarantee degenerates to "node 0." Real NUMA discovery is tracked in align-tasks.md Task BD-2, which resolves this and SC-007 together.)*
- **FR-014**: The actor thread MUST poll all attached client channels.
- **FR-015**: The component MUST exploit different NVMe IO queues with varying queue depths to minimize latency for a given batch size. The queue pair pool MUST allocate queue pairs at standard depths [4, 16, 64, 256] (capped by the controller's maximum queue depth). The selection heuristic MUST choose the shallowest queue pair with sufficient available capacity for the batch. When all queue pairs are at capacity, the fallback selects the queue pair with the most available capacity (rather than unconditionally the deepest) to prevent cascading saturation. Each queue pair's `io_queue_requests` MUST be set to `depth * 4` to allow for SPDK request splitting. The larger multiplier absorbs SPDK request splitting and transient bursts.
- **FR-016**: The component MUST use a ILogger receptacle for debug logging; LoggerComponent MUST be usable for testing.
- **FR-017**: The component MUST use the spdk-env component for SPDK initialization.
- **FR-018**: Client-provided DmaBuffer structs MUST be accepted for read/write memory. Arc references MUST be usable in messages since clients are in-process.
- **FR-019**: When a client disconnects (drops its channel pair), the component MUST cancel all in-flight operations for that client and silently discard any pending completions. The actor MUST release all resources associated with the disconnected client.
- **FR-020**: All namespace management operations (probe, create, format, delete) MUST be serialized through the actor thread. No additional locking is required; the actor processes namespace commands in the order they are received from polled channels.
- **FR-021**: The component MUST provide an `IBlockDeviceAdmin` interface (defined via `define_interface!` in the interfaces crate) with methods: `set_pci_address(addr: PciAddress)` to configure the target NVMe controller, `set_actor_cpu(cpu: usize)` to pin the actor thread to a specific CPU core, `initialize() -> Result<(), NvmeBlockError>` to attach and start the actor, and `shutdown() -> Result<(), NvmeBlockError>` to stop the actor and join its thread. `set_pci_address` MUST be called before `initialize`.
- **FR-022**: The actor MUST use a hardware TSC (Time Stamp Counter) clock (`TscClock`) for async operation timeout checking, calibrated once at construction against `clock_gettime`. Timeout checks are throttled to approximately once per millisecond using TSC comparison.
- **FR-023**: The actor MUST use a `ContextPool` slab allocator for async IO context objects, eliminating per-IO heap allocation. Contexts are acquired at submission and returned to the pool in the SPDK completion callback. The pool allocates on first use and reaches steady-state after warmup, eliminating allocation in the hot path. Full pre-allocation at construction is not required.
- **FR-024**: The actor MUST use pre-allocated scratch buffers (`completion_scratch`, `timeout_scratch`) for draining completions and collecting timed-out handles, avoiding allocation in the hot path.
- **FR-025**: When an asynchronous NVMe command submission fails with ENOMEM (rc=-12, indicating the submission queue or internal tracker pool is full), the actor MUST retry the submission in a tight loop for up to `min(op.timeout_ms, 1000ms)` as a dynamic cap that adapts per-operation, avoiding premature timeouts under heavy load. On each retry iteration the actor MUST poll all queue pairs for hardware completions to free submission slots. If the deadline expires without a successful submit, the operation MUST be failed with an error completion sent to the client.
- **FR-026**: The actor MUST deliver completions to a client's callback channel without blocking. Because a single actor thread serves all clients on a controller, it MUST NOT block delivering a completion to one client, as that would head-of-line-block completion delivery to every other client on the same controller. Completions that cannot be delivered immediately (callback ring full) MUST be buffered per-client in FIFO order and retried on subsequent poll cycles. Per-client backlog is bounded by that client's outstanding operations. (Implemented by `ClientSession::deliver`/`flush_pending`; the `Completion` type derives `Clone` so a completion can be `try_send`-cloned without being consumed on a full ring.)
- **FR-027**: The `IBlockDeviceAdmin` interface MUST provide a `signal_stop()` method that requests actor shutdown by closing the command channel without blocking to join the actor thread, enabling a coordinating process to signal multiple actors to stop concurrently before joining them. It MUST also provide a `detach_controller()` method that performs an explicit `spdk_nvme_detach` on the underlying controller after shutdown; this is required because the component's controller handle participates in an `Arc` reference cycle that prevents automatic detachment on drop, so callers that need deterministic controller release MUST call `detach_controller()` explicitly after `shutdown()`/`signal_stop()`.
- **FR-028**: On actor shutdown, the actor MUST perform a graceful drain of in-flight operations, waiting up to a 5-second deadline for outstanding NVMe commands to complete. Any operations still outstanding when the deadline expires MUST receive an `Completion::Error{Aborted}` delivered to their client. After the drain deadline and the `Aborted` deliveries, the controller MUST be "parked" (moved into the shared park slot for later detachment via `detach_controller()`, FR-027). *(Backfilled 2026-08-07 — the shipped `on_stop` order is drain → deliver `Error{Aborted}` → park, i.e. the controller is parked after the drain, not before. This is race-free because the actor loop has already stopped accepting/submitting new commands before `on_stop` runs, so no new IO can be submitted during the drain regardless of park timing. The spec previously required park-before-drain; corrected here to match the implemented, safe ordering.)*
- **FR-030**: *(Backfilled 2026-08-07)* The IBlockDevice interface MUST expose a `read_write_stats()` method returning a `ReadWriteStats` value with per-direction (read vs write) byte, operation, and latency counters, complementing the aggregate telemetry of FR-011. Unlike FR-011's min/max/mean/throughput aggregate (gated behind the `telemetry` feature), `read_write_stats` provides directionally-split counters. The `ReadWriteStats` value additionally carries per-transfer-size IO histograms — `read_size_buckets`/`write_size_buckets`, each `[u64; IO_SIZE_BUCKETS]` with `IO_SIZE_BUCKETS = 25` log2-spaced buckets indexed by `ReadWriteStats::size_bucket(bytes)` (with `bucket_lower_bound(idx)` reporting each bucket's lower edge) — plus a `merge_from(&other)` helper that sums two `ReadWriteStats` (counters and both histograms) for dispatcher-wide aggregation across multiple devices. (Implemented at `iblock_device.rs:589` (trait decl), `lib.rs:525` (impl), `telemetry.rs:150` (backing accumulation); histogram surface at `interfaces/src/iblock_device.rs:139,155-161,177,193,218`. *(Anchors re-synced and histogram/`merge_from` surface backfilled 2026-09-03.)*)
- **FR-029**: The actor MUST poll attached client channels using a fair round-robin rotation (tracked via a rotating start index) rather than always starting from the same client, so that no single client is starved when many clients are attached. Within a single poll of a given client, the actor MUST cap the number of commands drained from that client's channel to a fixed per-client-per-poll limit (`MAX_COMMANDS_PER_CLIENT_PER_POLL`), currently 64, to bound worst-case per-poll latency and preserve fairness across clients. Changes to this limit MUST be reflected in this requirement.
- **FR-032**: *(Backfilled 2026-08-27)* In debug builds only (`#[cfg(debug_assertions)]`), the actor MUST emit a diagnostic log line to stderr for each successfully submitted read/write NVMe command, reporting the size of the DMA data transfer issued to the controller. The line MUST include the operation kind (`read-sync`, `write-sync`, `read-async`, `write-async`), the starting LBA, the block count, and the transfer size in bytes (blocks × sector size). The reported size is the size of the logical request as submitted by the driver, which may be split into multiple on-wire NVMe commands by SPDK when it exceeds the controller's MDTS (see FR-010). The diagnostic MUST be gated so that it compiles out entirely in release builds — no argument evaluation and zero runtime cost — and MUST be independent of the `telemetry` feature (FR-011), since it aids field diagnosis of transfer-size behavior (e.g. MDTS-related command errors) in unoptimized builds without requiring a telemetry-enabled binary. Logging occurs only on the submission success path, after the SPDK submit call returns success, so a failed submission produces no line. (Implemented via the `log_dma_issue!` macro at `src/actor.rs:50`, invoked at `src/actor.rs:825` (async read), `:945` (async write), `:1189` (sync read), and `:1240` (sync write).)
- **FR-031**: *(Backfilled 2026-08-20)* The component MUST support a synchronous flush operation (`Command::FlushSync { ns_id }`) that flushes the target namespace's volatile write cache to non-volatile media via `spdk_nvme_ns_cmd_flush`, blocking until the controller signals completion and then delivering a `Completion::FlushDone { handle, result }`. This provides a durability barrier for clients that must guarantee prior writes are persisted (e.g. the extent-manager `volatile_write_cache` durability path). The namespace id MUST be validated before the flush is issued; an invalid namespace or a non-zero SPDK submit return code MUST surface as an error in the completion result rather than crashing the actor. (Implemented at `src/actor.rs:968-978` dispatch and `src/actor.rs:1249-1288` `do_sync_flush`. *(Anchors re-synced 2026-09-03: dispatch moved from `941-951` to `968-978`, `do_sync_flush` from `1214-1260` to `1249-1288`; behavior unchanged.)*)

### Key Entities

- **NVMe Controller**: The physical NVMe device bound to a component instance. Has properties: NUMA id, NVMe version, max transfer size, IO queue configuration.
- **NVMe Namespace**: A logical storage partition on a controller. Has properties: namespace id, capacity, block size.
- **Client Channel Pair**: A pair of shared-memory channels (ingress + callback) representing one connected client session.
- **DmaBuffer**: Client-allocated DMA-capable memory used for read/write data transfer. Defined in spdk_types.rs.
- **Operation Handle**: A unique identifier assigned by the component to each async operation at submission time. Used by the client for abort requests and by the component in completion callbacks.
- **IO Command**: A message on the ingress channel specifying operation type, namespace, LBA, buffer, and (for async) timeout.
- **Completion Callback**: A message on the callback channel indicating operation success, failure, or abort acknowledgement, tagged with the corresponding operation handle. Derives `Clone` so the actor can `try_send` a clone onto a full callback ring without consuming the original (see FR-026).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A client can complete a synchronous read/write round-trip (write then read-back verification) within the latency envelope expected for direct NVMe access (single-digit microsecond range for 4KB blocks).
- **SC-002**: Asynchronous operations that exceed their specified timeout are reported as errors within a bounded margin (no more than 10% beyond the timeout value).
- **SC-003**: Batch operations achieve higher aggregate throughput than the equivalent number of individual synchronous operations.
- **SC-004**: The component correctly handles all namespace management lifecycle operations (probe, create, format, delete) without data corruption or resource leaks.
- **SC-005**: Device information queries return values consistent with the physical hardware properties of the bound NVMe controller — for the hardware-derived fields (capacity, sector/block size, max queue depth, IO-queue count, and `max_transfer_size`, which is MDTS-derived and therefore hardware-consistent; see FR-010). Per FR-010, `nvme_version` and `numa_id` remain fixed constants (1.0.0 and 0 respectively) pending the align-tasks.md Task BD-2 hardware-discovery work; a monitoring client sees fixed values for those two fields regardless of the bound controller. *(Re-synced 2026-08-20 — `max_transfer_size` was previously listed as fixed but is auto-detected from the controller's MDTS, so it is no longer a fixed constant.)*
- **SC-006**: When telemetry is enabled, latency and throughput statistics are accurate to within 5% of independently measured values.
- **SC-007**: The actor thread runs on a core in the same NUMA zone as the controller, verified at instantiation. *(Currently satisfied only for node-0 devices — see FR-013: the controller NUMA node is hardcoded to 0, so the "same NUMA zone" guarantee is real only when the device is on node 0 or the host is single-node. Full satisfaction is tied to align-tasks.md Task BD-2.)*
- **SC-008**: All public interface methods have unit tests and documentation tests. Performance-sensitive paths (IO submission, batch processing, qpair selection) MUST have performance measurement coverage. This is satisfied by the dedicated IOPS benchmark application (`apps/iops-benchmark`), which exercises these paths under realistic workloads with configurable queue depths, block sizes, thread counts, and access patterns. Per-function Criterion benchmarks within this crate are not required.

## Assumptions

- The NVMe controller device is available and accessible via SPDK at instantiation time.
- SPDK environment initialization is handled by the spdk-env sibling component before this component is instantiated.
- All clients operate within the same process; inter-process communication is out of scope.
- The host system runs Linux with hugepages and VFIO/UIO configured for SPDK.
- Namespace discovery at attach time is performed by the controller's internal `discover_namespaces` path (results exposed via `NsProbe`/`to_namespace_info_list`). The standalone `namespace::probe()` free function (`src/namespace.rs:20-47`) is a superseded legacy helper, retained only under `#[allow(dead_code)]`; it is not on any live code path and is a candidate for removal. It is documented here (rather than specified) so the drift sweep does not repeatedly re-flag it as unspecced behavior. *(Noted 2026-08-20.)*
- A fast SPSC channel implementation is required for client ingress/callback channels; the component uses `component_core::channel::spsc::SpscChannel` bounded to `CLIENT_CHANNEL_CAPACITY` (256) slots per channel for production use. (Earlier drafts of this spec referenced a `crossbeam`-bounded 64-slot channel for testing/benchmarking; the `crossbeam-channel` crate is no longer used by the production channel path — see align-tasks for cleanup of the now-unused dependency and matching benchmark doc comments.)
- The component-framework from `components/component-framework` provides the interface, receptacle, and actor infrastructure.
