# Feature Specification: SPDK NVMe Block Device Component

**Feature Branch**: `001-spdk-nvme-block-device`
**Created**: 2026-04-14
**Status**: Draft
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
- **FR-004**: System MUST support asynchronous read and write operations with a timeout value; operations exceeding timeout MUST return an error. The component MUST assign a unique operation handle on submission and return it to the client. Completion callbacks MUST include the corresponding operation handle.
- **FR-005**: System MUST support aborting an in-flight asynchronous operation identified by its component-assigned operation handle.
- **FR-006**: System MUST support a write-zeros operation.
- **FR-007**: System MUST support batch submission of IO operations.
- **FR-008**: System MUST support probing, creating, formatting, and deleting NVMe namespaces.
- **FR-009**: System MUST support controller hardware reset with graceful handling of in-flight operations.
- **FR-010**: System MUST expose device information (capacity, max queue depth, IO queue count, max transfer size, block/sector size, NUMA id, NVMe version) via the IBlockDevice interface.
- **FR-011**: When compiled with the `telemetry` feature, system MUST collect and expose min/max/mean IO latencies, total operation count, and mean throughput. When compiled without the feature, the telemetry API MUST return an error.
- **FR-012**: Each component instance MUST be associated with a single NVMe controller device, configured via `IBlockDeviceAdmin::set_pci_address` and attached via `IBlockDeviceAdmin::initialize` (see FR-021).
- **FR-013**: The actor service thread MUST be pinned to a core in the same NUMA zone as the NVMe controller device.
- **FR-014**: The actor thread MUST poll all attached client channels.
- **FR-015**: The component MUST exploit different NVMe IO queues with varying queue depths to minimize latency for a given batch size. The queue pair pool MUST allocate queue pairs at standard depths [4, 16, 64, 256] (capped by the controller's maximum queue depth). The selection heuristic MUST choose the shallowest queue pair with sufficient available capacity for the batch, falling back to the deepest queue pair when all queues are at capacity. Each queue pair's `io_queue_requests` MUST be set to `depth * 2` to allow for SPDK request splitting.
- **FR-016**: The component MUST use a ILogger receptacle for debug logging; LoggerComponent MUST be usable for testing.
- **FR-017**: The component MUST use the spdk-env component for SPDK initialization.
- **FR-018**: Client-provided DmaBuffer structs MUST be accepted for read/write memory. Arc references MUST be usable in messages since clients are in-process.
- **FR-019**: When a client disconnects (drops its channel pair), the component MUST cancel all in-flight operations for that client and silently discard any pending completions. The actor MUST release all resources associated with the disconnected client.
- **FR-020**: All namespace management operations (probe, create, format, delete) MUST be serialized through the actor thread. No additional locking is required; the actor processes namespace commands in the order they are received from polled channels.
- **FR-021**: The component MUST provide an `IBlockDeviceAdmin` interface (defined via `define_interface!` in the interfaces crate) with methods: `set_pci_address(addr: PciAddress)` to configure the target NVMe controller, `set_actor_cpu(cpu: usize)` to pin the actor thread to a specific CPU core, `initialize() -> Result<(), NvmeBlockError>` to attach and start the actor, and `shutdown() -> Result<(), NvmeBlockError>` to stop the actor and join its thread. `set_pci_address` MUST be called before `initialize`.
- **FR-022**: The actor MUST use a hardware TSC (Time Stamp Counter) clock (`TscClock`) for async operation timeout checking, calibrated once at construction against `clock_gettime`. Timeout checks are throttled to approximately once per millisecond using TSC comparison.
- **FR-023**: The actor MUST use a `ContextPool` slab allocator for async IO context objects, eliminating per-IO heap allocation. Contexts are acquired at submission and returned to the pool in the SPDK completion callback.
- **FR-024**: The actor MUST use pre-allocated scratch buffers (`completion_scratch`, `timeout_scratch`) for draining completions and collecting timed-out handles, avoiding allocation in the hot path.
- **FR-025**: When an asynchronous NVMe command submission fails with ENOMEM (rc=-12, indicating the submission queue or internal tracker pool is full), the actor MUST retry the submission in a tight loop for up to 50ms. On each retry iteration the actor MUST poll all queue pairs for hardware completions to free submission slots. If the deadline expires without a successful submit, the operation MUST be failed with an error completion sent to the client.

### Key Entities

- **NVMe Controller**: The physical NVMe device bound to a component instance. Has properties: NUMA id, NVMe version, max transfer size, IO queue configuration.
- **NVMe Namespace**: A logical storage partition on a controller. Has properties: namespace id, capacity, block size.
- **Client Channel Pair**: A pair of shared-memory channels (ingress + callback) representing one connected client session.
- **DmaBuffer**: Client-allocated DMA-capable memory used for read/write data transfer. Defined in spdk_types.rs.
- **Operation Handle**: A unique identifier assigned by the component to each async operation at submission time. Used by the client for abort requests and by the component in completion callbacks.
- **IO Command**: A message on the ingress channel specifying operation type, namespace, LBA, buffer, and (for async) timeout.
- **Completion Callback**: A message on the callback channel indicating operation success, failure, or abort acknowledgement, tagged with the corresponding operation handle.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A client can complete a synchronous read/write round-trip (write then read-back verification) within the latency envelope expected for direct NVMe access (single-digit microsecond range for 4KB blocks).
- **SC-002**: Asynchronous operations that exceed their specified timeout are reported as errors within a bounded margin (no more than 10% beyond the timeout value).
- **SC-003**: Batch operations achieve higher aggregate throughput than the equivalent number of individual synchronous operations.
- **SC-004**: The component correctly handles all namespace management lifecycle operations (probe, create, format, delete) without data corruption or resource leaks.
- **SC-005**: Device information queries return values consistent with the physical hardware properties of the bound NVMe controller.
- **SC-006**: When telemetry is enabled, latency and throughput statistics are accurate to within 5% of independently measured values.
- **SC-007**: The actor thread runs on a core in the same NUMA zone as the controller, verified at instantiation.
- **SC-008**: All public interface methods have unit tests and documentation tests. Performance-sensitive paths (IO submission, batch processing, qpair selection) MUST have performance measurement coverage. This is satisfied by the dedicated IOPS benchmark application (`apps/iops-benchmark`), which exercises these paths under realistic workloads with configurable queue depths, block sizes, thread counts, and access patterns. Per-function Criterion benchmarks within this crate are not required.

## Assumptions

- The NVMe controller device is available and accessible via SPDK at instantiation time.
- SPDK environment initialization is handled by the spdk-env sibling component before this component is instantiated.
- All clients operate within the same process; inter-process communication is out of scope.
- The host system runs Linux with hugepages and VFIO/UIO configured for SPDK.
- A fast SPSC channel implementation (e.g., crossbeam bounded to 64 slots) is suitable for testing and benchmarking.
- The component-framework from `components/component-framework` provides the interface, receptacle, and actor infrastructure.
