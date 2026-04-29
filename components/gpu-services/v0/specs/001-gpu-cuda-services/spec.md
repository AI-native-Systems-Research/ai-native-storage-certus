# Feature Specification: GPU CUDA Services

**Feature Branch**: `001-gpu-cuda-services`
**Created**: 2026-04-29
**Status**: Draft
**Input**: User description: "GPU Services component providing CUDA library initialization, GPU hardware scanning, IPC handle deserialization, memory pinning, and DMA buffer creation for SSD-to-GPU data transfer"

## Clarifications

### Session 2026-04-29

- Q: What IPC transport mechanism is used for Python-to-Rust handle handoff? → A: Unix domain socket
- Q: How is the target GPU selected when multiple GPUs are present? → A: Implicit from IPC handle (handle carries device context)
- Q: What is the minimum CUDA compute capability required? → A: 7.0+ (Volta and newer)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Initialize CUDA and Discover GPU Hardware (Priority: P1)

A system operator starts the GPU services component to prepare for
GPU-accelerated storage operations. The component initializes CUDA
libraries and scans the system for available GPU hardware, reporting
model names, memory capacity, and supported compute architecture
levels.

**Why this priority**: Initialization and hardware discovery are
prerequisites for all other GPU operations. No other functionality
can proceed without a successful CUDA environment.

**Independent Test**: Can be fully tested by calling `initialize()`
and verifying that GPU hardware information is returned with correct
model, memory, and architecture fields populated.

**Acceptance Scenarios**:

1. **Given** a system with one or more NVIDIA GPUs and CUDA drivers
   installed, **When** `initialize()` is called, **Then** CUDA
   libraries are loaded and the component reports success.
2. **Given** a successfully initialized component, **When** GPU
   hardware is scanned, **Then** for each GPU the model name, total
   memory capacity in bytes, and compute architecture level are
   returned.
3. **Given** a system with no NVIDIA GPU or missing CUDA drivers,
   **When** `initialize()` is called, **Then** a descriptive error is
   returned indicating the specific failure reason.

---

### User Story 2 - Deserialize Python IPC Handle (Priority: P2)

A Python process has allocated GPU memory and serialized the CUDA IPC
handle and buffer size as base64-encoded data. The Rust component
receives this serialized data and deserializes it into a usable CUDA
IPC handle and size, enabling cross-process GPU memory sharing.

**Why this priority**: IPC handle deserialization is the bridge between
the Python GPU allocation side and the Rust DMA engine. It must work
correctly before DMA buffers can be created.

**Independent Test**: Can be tested by providing known base64-encoded
IPC handle data and verifying the deserialized handle and size match
expected values.

**Acceptance Scenarios**:

1. **Given** a valid base64-encoded CUDA IPC handle and size from a
   Python process, **When** deserialization is requested, **Then** the
   component returns a valid CUDA IPC handle struct and the correct
   buffer size.
2. **Given** malformed or invalid base64 data, **When** deserialization
   is attempted, **Then** the component returns a clear error indicating
   the data is invalid without panicking or leaking resources.
3. **Given** a valid IPC handle from a terminated process, **When**
   deserialization succeeds but the handle is stale, **Then** subsequent
   operations on the handle return an appropriate error.

---

### User Story 3 - Verify GPU Memory Contiguity and Pin Status (Priority: P3)

Before performing DMA operations, the system must verify that GPU
memory associated with an IPC handle is physically contiguous and
pinned (page-locked). This prevents DMA failures due to memory that
could be paged out or scattered across non-contiguous physical pages.

**Why this priority**: Memory verification is a safety gate before DMA
operations. Skipping this check could lead to data corruption or
hardware errors.

**Independent Test**: Can be tested by providing an IPC handle to known
pinned/contiguous memory and verifying the check passes, and by
providing unpinned memory and verifying the check fails.

**Acceptance Scenarios**:

1. **Given** an IPC handle referencing contiguous, pinned GPU memory,
   **When** the contiguity/pin check is performed, **Then** the check
   returns success.
2. **Given** an IPC handle referencing non-contiguous or unpinned GPU
   memory, **When** the check is performed, **Then** the component
   returns a failure indicating which condition was not met.

---

### User Story 4 - Pin and Unpin GPU Memory (Priority: P4)

The system needs to pin GPU memory to prevent it from being paged out
during DMA transfers, and to unpin it when DMA operations are complete
to return resources to the system.

**Why this priority**: Memory pinning is required for reliable DMA but
is a resource management operation that depends on prior stories being
functional.

**Independent Test**: Can be tested by allocating GPU memory, pinning
it, verifying pin status, then unpinning and verifying it is released.

**Acceptance Scenarios**:

1. **Given** a valid GPU memory region, **When** pin is requested,
   **Then** the memory is page-locked and the operation returns success.
2. **Given** pinned GPU memory, **When** unpin is requested, **Then**
   the memory is released from page-lock and returns to normal status.
3. **Given** already-pinned memory, **When** pin is requested again,
   **Then** the operation is idempotent (succeeds without error).
4. **Given** memory that cannot be pinned (insufficient resources),
   **When** pin is requested, **Then** a descriptive error is returned.

---

### User Story 5 - Create DMA Buffer from IPC Handle (Priority: P5)

A Rust process receives a CUDA IPC handle from a Python process and
creates a DmaBuffer object that can be used to perform DMA transfers
from an SSD (via block-device-spdk-nvme) or from CPU-memory allocated
DmaBuffers. This enables direct GPU-to-storage data paths.

**Why this priority**: DMA buffer creation is the culmination of all
prior stories — it requires initialization, handle deserialization,
memory verification, and pinning to be functional.

**Independent Test**: Can be tested by deserializing a known IPC handle,
creating a DmaBuffer, and verifying its properties match the source
GPU allocation. Full DMA transfer testing requires integration with
block-device-spdk-nvme.

**Acceptance Scenarios**:

1. **Given** a valid deserialized IPC handle referencing pinned,
   contiguous GPU memory, **When** DMA buffer creation is requested,
   **Then** a DmaBuffer is returned with correct size and GPU memory
   backing.
2. **Given** a DmaBuffer created from a GPU IPC handle, **When** it is
   used as a target for SSD DMA via block-device-spdk-nvme, **Then**
   data is transferred correctly to GPU memory.
3. **Given** a DmaBuffer created from a GPU IPC handle, **When** it is
   used as a target for CPU-to-GPU memory copy from an SPDK-allocated
   DmaBuffer, **Then** data is transferred correctly.
4. **Given** an invalid or expired IPC handle, **When** DMA buffer
   creation is attempted, **Then** the operation fails with a
   descriptive error and no resources are leaked.

---

### User Story 6 - Python-to-Rust IPC Handle Handoff Demo (Priority: P6)

A test application demonstrates the end-to-end flow: a Python client
allocates GPU memory, serializes the IPC handle, sends it to a Rust
server process using this component, which then deserializes the handle
and performs DMA operations from SPDK-allocated CPU memory to the GPU
buffer.

**Why this priority**: This is a demonstration/integration test that
validates the full pipeline works end-to-end. It depends on all prior
stories.

**Independent Test**: Can be tested by running the Python client and
Rust server together — the Python client allocates GPU memory, hands
off the IPC handle, and the Rust server performs a DMA write followed
by a verification read-back.

**Acceptance Scenarios**:

1. **Given** the Python client (`apps/gpu-handle-test-client`) and Rust
   server (`apps/gpu-handle-test-server`) are connected via a Unix
   domain socket, **When** the Python client allocates GPU memory and
   sends the serialized IPC handle over the socket, **Then** the Rust
   server successfully deserializes it.
2. **Given** the Rust server has a valid IPC handle, **When** it creates
   a DmaBuffer and performs a DMA write from SPDK CPU memory, **Then**
   the data appears correctly in GPU memory (verified by read-back).

---

### Edge Cases

- What happens when CUDA driver version is incompatible with runtime?
- How does the component handle GPU memory exhaustion during pinning?
- What happens if the Python process terminates while the Rust process
  holds an open IPC handle?
- How does the component behave when multiple GPUs are present but only
  some meet the minimum compute capability 7.0 requirement?
- What happens if DMA buffer creation is attempted on a GPU that has
  been reset or removed (hot-unplug)?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Component MUST initialize CUDA libraries and report
  success or a descriptive failure reason.
- **FR-002**: Component MUST enumerate all NVIDIA GPUs in the system
  with compute capability 7.0+ (Volta and newer) and report model
  name, memory capacity (bytes), and compute architecture level for
  each. GPUs below compute capability 7.0 MUST be excluded from
  enumeration.
- **FR-003**: Component MUST deserialize a base64-encoded CUDA IPC
  handle and size originating from a Python process into native Rust
  data structures.
- **FR-004**: Component MUST verify that GPU memory referenced by an
  IPC handle is physically contiguous and pinned before allowing DMA
  buffer creation. The target GPU device MUST be determined implicitly
  from the IPC handle's device context.
- **FR-005**: Component MUST provide pin and unpin operations for GPU
  memory regions.
- **FR-006**: Component MUST create a DmaBuffer (as defined in
  `spdk_types.rs`) from a valid IPC handle, suitable for DMA from SSD
  via block-device-spdk-nvme or from CPU-memory DmaBuffers.
- **FR-007**: All operations MUST return descriptive errors on failure
  without panicking or leaking GPU/system resources.
- **FR-008**: Component MUST expose all functionality exclusively
  through the `IGpuServices` interface defined in
  `components/interfaces`.
- **FR-009**: Component build MUST be gated behind `--features gpu`
  feature flag.
- **FR-010**: Component MUST include unit tests and Criterion benchmarks
  available when the `gpu` feature is enabled.

### Key Entities

- **GpuDevice**: Represents a discovered GPU — model name, memory
  capacity, compute architecture level, device index.
- **CudaIpcHandle**: Deserialized CUDA IPC memory handle enabling
  cross-process GPU memory sharing.
- **DmaBuffer**: Buffer object (defined in `spdk_types.rs`) backed by
  GPU memory, usable for SSD or CPU-memory DMA transfers.
- **PinnedRegion**: Represents a page-locked GPU memory region with
  lifecycle management (pin/unpin).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: CUDA initialization completes in under 5 seconds on a
  system with installed NVIDIA drivers and at least one GPU.
- **SC-002**: GPU hardware scan returns complete device information for
  all installed GPUs within 1 second of initialization.
- **SC-003**: IPC handle deserialization from base64 completes in under
  1 millisecond per handle.
- **SC-004**: Memory contiguity and pin verification completes in under
  10 milliseconds per handle.
- **SC-005**: DMA buffer creation from a valid IPC handle completes in
  under 50 milliseconds.
- **SC-006**: All unit tests pass with `cargo test -p gpu-services
  --features gpu`.
- **SC-007**: All Criterion benchmarks run without error via
  `cargo bench -p gpu-services --features gpu`.
- **SC-008**: The Python-to-Rust demo application successfully
  completes end-to-end IPC handle handoff and DMA transfer.

## Assumptions

- NVIDIA CUDA drivers and runtime are pre-installed on the target
  system; the component does not install or manage driver versions.
- The Python client uses standard `cuda` or `cupy` libraries to
  allocate GPU memory and serialize IPC handles using Python's
  `base64` module. The Python-to-Rust handoff uses a Unix domain
  socket for IPC transport.
- The `DmaBuffer` type is defined in `spdk_types.rs` within the
  workspace and provides the necessary interface for SPDK DMA
  operations.
- The `block-device-spdk-nvme` component is available for integration
  testing of SSD-to-GPU DMA paths.
- The `--features gpu` gate ensures the component does not introduce
  build dependencies on systems without CUDA toolkits.
- IPC handle serialization format from Python uses standard base64
  encoding of the raw CUDA IPC handle bytes concatenated with the
  buffer size as a little-endian 64-bit integer.
- The target system runs Linux with IOMMU and hugepages configured
  for SPDK operations.
- All target GPUs have compute capability 7.0 or higher (Volta
  architecture and newer). Pre-Volta GPUs are not supported.
- GPU device selection for IPC operations is implicit — the IPC handle
  carries the originating device context and the component follows it
  automatically.
