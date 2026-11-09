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
2. **Given** an IPC handle referencing memory that is not CUDA device
   memory (e.g. host or unregistered memory), **When** the check is
   performed, **Then** the component returns a failure. *(Backfilled
   2026-08-07: the implemented check `check_memory_attributes`
   (`src/memory.rs:26`) is a device-type check via
   `cudaPointerGetAttributes` per FR-004; it does not separately diagnose
   contiguity vs pin status. IPC-opened device memory is inherently
   contiguous and pinned by the CUDA runtime, so a separate contiguity/pin
   distinction is not required.)*

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
  data structures. **Precondition**: the caller is responsible for
  setting the correct CUDA device context (via `cudaSetDevice`) before
  calling `deserialize_ipc_handle`. `open_ipc_handle` does not call
  `cudaSetDevice` internally. Callers using the high-level
  `prepare_memory_for_spdk` (FR-013) are exempt from this requirement
  since that function manages device context internally.
- **FR-004**: Component MUST verify that GPU memory referenced by an
  IPC handle is CUDA device memory via `cudaPointerGetAttributes`
  before allowing DMA buffer creation. Verified pointers are tracked
  in an internal set.
- **FR-005**: Component MUST provide pin and unpin operations for GPU
  memory regions. For IPC-opened device memory (which is inherently
  pinned by the CUDA runtime), pin verifies device-residency and
  tracks state in an internal `HashSet<usize>`; unpin releases
  tracking. `pin_memory` is idempotent (no-op if already pinned), and
  `unpin_memory` returns an error if the pointer is not in the pinned
  set. For IPC-imported memory, `unpin_memory` removes internal
  tracking only — CUDA unregistration is not needed since the memory
  was registered in the originating process. `unpin_memory` performs
  tracking-removal only in all cases and never calls `cudaHostUnregister`;
  there is no locally-pinned host-registration path in
  `pin_memory`/`unpin_memory`. Full CUDA un/registration of *host* memory is
  handled exclusively by `register_host_memory`/`unregister_host_memory`
  (FR-015/FR-016). *(Backfilled 2026-08-07: prior wording claimed
  `unpin_memory` performed full CUDA unregistration for locally-pinned
  memory; the code has no such path — `src/lib.rs:249-267`.)* As an
  optimization, `pin_memory` MAY skip re-verification (via
  `cudaPointerGetAttributes`) for pointers already present in the
  verified set, since verification is a prerequisite in the standard
  IPC workflow.
- **FR-006**: Component MUST create a `GpuDmaBuffer` (as defined in
  `interfaces`) from a valid, verified, and pinned IPC handle. The
  `GpuDmaBuffer` wraps the GPU device pointer with custom free
  semantics. A separate `prepare_memory_for_spdk` path returns an
  SPDK `DmaBuffer` instead (see spec 002).
- **FR-007**: All operations MUST return descriptive errors on failure
  without panicking or leaking GPU/system resources.
- **FR-008**: Component MUST expose its interface-level operations
  through the `IGpuServices` interface defined in
  `components/interfaces`. *Exception (clarified 2026-08-07):* the
  `p2p`/GDRCopy DMA-buffer builders in the `dma` module
  (`create_spdk_dma_buffer_from_gpu`/`_from_cuda_malloc`/
  `_from_cuda_host_alloc`/`_from_gpu_bar`/`_from_phys`/`_from_bar_direct`,
  and `get_phys_addr`) are intentionally `pub` module functions consumed
  directly by the `gpu-p2p-server` binary (specs 002 FR-021..024 / 003),
  not `IGpuServices` methods. The "exclusively through the interface"
  scope applies to the CUDA-services operations of spec 001; the p2p
  module helpers are a documented, deliberate exception. This resolves
  the FR-008-vs-002/003 conflict noted in the 2026-08-07 drift report.
- **FR-009**: Component build MUST be gated behind `--features gpu`
  feature flag.
- **FR-010**: Component MUST include unit tests and Criterion benchmarks
  available when the `gpu` feature is enabled.
- **FR-011**: Component MUST provide `dma_copy_to_host` to copy from a
  GPU device pointer to an SPDK `DmaBuffer` using `cudaMemcpy`
  device-to-host. Gated behind `spdk` feature.
- **FR-012**: Component MUST provide `dma_copy_to_device` to copy from
  an SPDK `DmaBuffer` to a GPU device pointer using `cudaMemcpy`
  host-to-device. Gated behind `spdk` feature.
- **FR-013**: Component MUST provide `prepare_memory_for_spdk` that
  accepts a base64 IPC payload and optional device index, performs
  the full open/verify/pin/DmaBuffer-creation pipeline, and returns
  an SPDK `DmaBuffer` with pin-state-aware cleanup. Gated behind
  `spdk` feature. (See spec 002 for full requirements.)
- **FR-014**: When the `gpu` feature is disabled, all interface methods
  MUST return an error indicating GPU support is not compiled, without
  panicking. Exception: `shutdown()` returns `Ok(())` as a no-op when
  the GPU feature is disabled, since there are no resources to release.
- **FR-015**: Component MUST provide a `register_host_memory(ptr, size)`
  method (gated behind `spdk` feature) that page-locks the specified
  host memory region via `cudaHostRegister` (enabling async GPU DMA)
  and registers it with SPDK via `spdk_mem_register` (enabling NVMe
  DMA). If `cudaHostRegister` succeeds but `spdk_mem_register` fails,
  the method MUST roll back by calling `cudaHostUnregister` before
  returning the error.
- **FR-016**: Component MUST provide an `unregister_host_memory(ptr,
  size)` method (gated behind `spdk` feature) that unregisters memory
  from SPDK via `spdk_mem_unregister` then removes page-locking via
  `cudaHostUnregister`. MUST be called before freeing the underlying
  allocation.
- **FR-017**: Component MUST provide CUDA stream lifecycle management
  via `create_stream`, `destroy_stream`, and `stream_synchronize`
  methods. `create_stream` returns an opaque `GpuStream` handle
  wrapping a `cudaStream_t`; `destroy_stream` releases the stream;
  `stream_synchronize` blocks until all operations on the stream
  complete. All three require prior initialization and return errors
  when GPU support is not compiled.
- **FR-018**: Component MUST provide `dma_copy_to_device_async` (gated
  behind `spdk` feature) that issues a `cudaMemcpyAsync`
  host-to-device copy from an SPDK `DmaBuffer` to a GPU device pointer
  on a specified `GpuStream`, enabling concurrent GPU DMA overlapped
  with CPU/NVMe work. The caller must synchronize the stream before
  reusing the source buffer.
- **FR-019**: Component MUST provide `memcpy_h2d_async` (gated behind
  `spdk` feature) that issues a `cudaMemcpyAsync` host-to-device copy
  from a raw CUDA-pinned host pointer to a GPU device pointer on a
  specified `GpuStream`. This avoids requiring callers to wrap
  pre-existing pinned memory in a `DmaBuffer`.
- **FR-020**: Component MUST provide `allocate_pinned_dma_buffer(size)`
  (gated behind `spdk` feature) that allocates page-locked host memory
  via `cudaHostAlloc` and registers it with SPDK via
  `spdk_mem_register`, returning an SPDK `DmaBuffer`. The returned
  buffer is suitable as both an NVMe DMA target and a source for async
  GPU H2D copies. On drop, the buffer unregisters from SPDK and frees
  via `cudaFreeHost`.
- **FR-021**: Component MUST provide a `set_device(device)` method that
  binds the calling thread's current CUDA device context to the
  specified GPU ordinal via `cudaSetDevice`, so that subsequently-created
  streams and issued transfers target that GPU. This is required for
  multi-GPU / tensor-parallel operation, where each device must be
  selected before a stream is created on it or a `cudaMemcpyAsync` is
  issued to a pointer resident on it (a stream is bound to the device
  that was current at creation, and `cudaMemcpyAsync` rejects a
  destination pointer on a different device). CUDA tracks the current
  device per OS thread. MUST return an error if GPU support is not
  compiled, the component is not initialized, or the device ordinal is
  invalid.
- **FR-022**: Component MUST provide a `device_of_ptr(ptr)` method that
  returns the CUDA device ordinal owning a given device pointer via
  `cudaPointerGetAttributes`. It MUST return `-1` for a pointer with no
  device association (e.g. plain host or unregistered memory). This is
  used to route a transfer to a stream on the pointer's own device and to
  reject cross-device pointers. MUST return an error if GPU support is
  not compiled, the component is not initialized, or the attribute query
  fails.
- **FR-023**: Component MUST provide a `stream_query(stream)` method that
  performs a non-blocking check of whether all operations queued on a
  `GpuStream` have completed, via `cudaStreamQuery`. It MUST return
  `Ok(true)` when all queued work is complete and `Ok(false)` when work
  is still in-flight, distinct from the blocking behavior of
  `stream_synchronize` (FR-017). MUST return an error if GPU support is
  not compiled or the stream handle is invalid.
  *(Backfilled 2026-07-22 from `src/lib.rs:656-678`,
  `interfaces/src/igpu_services.rs:596-619`.)*
- **FR-024**: Component MUST provide `dma_copy_to_host_async` (gated
  behind `spdk` feature) that issues a `cudaMemcpyAsync`
  device-to-host copy from a GPU device pointer to an SPDK `DmaBuffer`
  on a specified `GpuStream`, mirroring `dma_copy_to_device_async`
  (FR-018) for the opposite direction. The caller must synchronize the
  stream before reading the destination buffer. MUST return an error if
  `size` exceeds the destination buffer length, GPU support is not
  compiled, the component is not initialized, or the CUDA async memcpy
  operation fails.
  *(Backfilled 2026-07-22 from `src/lib.rs:799-851`,
  `interfaces/src/igpu_services.rs:694-714`.)*
- **FR-025**: Component MUST provide `memcpy_d2h_async` (gated behind
  `spdk` feature) that issues a `cudaMemcpyAsync` device-to-host copy
  from a GPU device pointer to a raw CUDA-pinned host pointer on a
  specified `GpuStream`, mirroring `memcpy_h2d_async` (FR-019) for the
  opposite direction and avoiding the need to wrap pre-existing pinned
  memory in a `DmaBuffer`. MUST return an error if GPU support is not
  compiled, the component is not initialized, or the CUDA async memcpy
  operation fails.
  *(Backfilled 2026-07-22 from `src/lib.rs:853-895`,
  `interfaces/src/igpu_services.rs:716-735`.)*

### Key Entities

- **GpuDeviceInfo**: Represents a discovered GPU — model name, memory
  capacity (bytes), compute architecture level, device index, and
  `pci_bus_id` string.
- **GpuIpcHandle**: Deserialized CUDA IPC memory handle enabling
  cross-process GPU memory sharing. Contains a raw pointer and size.
  *(Note, 2026-08-07: the shared `interfaces::GpuIpcHandle` struct also
  defines `verified`/`pinned` boolean fields with `set_*`/`is_*`
  accessors (`components/interfaces/src/igpu_services.rs:63-118`). These
  are **reserved for future use** and are not currently read or written
  by this component, which tracks verified/pinned state in its own
  `HashSet<usize>` sets (see GpuState). They are retained deliberately as
  reserved API surface rather than removed.)*
- **GpuDmaBuffer**: Buffer object (defined in `interfaces`) wrapping a
  GPU device pointer with custom free semantics, usable for
  GPU-memory-backed DMA transfers.
- **GpuState**: Internal state holding initialization flag, discovered
  devices, and `HashSet<usize>` sets for verified/pinned pointer
  tracking.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: CUDA initialization completes in under 5 seconds on a
  system with installed NVIDIA drivers and at least one GPU.
- **SC-002**: GPU hardware scan returns complete device information for
  all installed GPUs within 1 second of initialization.
- **SC-003**: IPC handle deserialization from base64 completes in under
  1 millisecond per handle.
- **SC-004**: Memory type verification via `cudaPointerGetAttributes`
  completes in under 10 milliseconds per handle.
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
- The `GpuDmaBuffer` type is defined in `interfaces` and wraps GPU
  device pointers. The SPDK `DmaBuffer` type (from `interfaces` with
  `spdk` feature) wraps host/device pointers for SPDK NVMe DMA.
- The `block-device-spdk-nvme` component is available for integration
  testing of SSD-to-GPU DMA paths.
- Two feature flags control compilation: `--features gpu` gates all
  CUDA-dependent code; `--features spdk` additionally gates
  `dma_copy_to_host`, `dma_copy_to_device`, and
  `prepare_memory_for_spdk` methods.
- IPC handle serialization format from Python uses standard base64
  encoding of the raw CUDA IPC handle bytes concatenated with the
  buffer size as a little-endian 64-bit integer.
- The target system runs Linux with IOMMU and hugepages configured
  for SPDK operations.
- All target GPUs have compute capability 7.0 or higher (Volta
  architecture and newer). Pre-Volta GPUs are not supported.
- `open_ipc_handle` (and the `deserialize_ipc_handle` method that calls
  it) does **not** call `cudaSetDevice`. It is a low-level function with
  an implicit precondition: the caller must have already set the CUDA
  device context to the target GPU via `cudaSetDevice` before calling
  this function. In the certus-server integration, `service.rs` is
  responsible for calling `cudaSetDevice` before invoking
  `deserialize_ipc_handle`. The high-level `prepare_memory_for_spdk`
  (FR-013) handles device context setup internally when an optional
  device index is provided — callers of that path are not required to
  manage device context themselves.
