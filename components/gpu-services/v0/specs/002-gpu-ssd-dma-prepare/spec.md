# Feature Specification: GPU-to-SSD DMA Buffer Preparation

**Feature Branch**: `002-gpu-ssd-dma-prepare`  
**Created**: 2026-05-06  
**Status**: Draft  
**Input**: User description: "Add to the component interface IGpuServices a function, called `prepare_memory_for_spdk` that takes a GPU memory IPC handle and creates a DmaBuffer that can be used to perform peer-to-peer DMA from the SSD to the GPU. The code should use cudaIpcMemLazyEnablePeerAccess and expect the memory to come from PyTorch (passed over gRPC and deserialized). The GPU memory should be pinned if not already pinned (use cudaPointerGetAttributes). The free function on the DmaBuffer should unpin the memory if it was originally pinned; i.e. use different free functions depending on the pinned state from the IPC handle. Pinning actions should be logged to the logger."

## Clarifications

### Session 2026-05-06

- Q: Should `prepare_memory_for_spdk` accept a base64 string or an already-opened `GpuIpcHandle`? → A: Accept base64 string (`&str`) — full pipeline in one call.
- Q: Should the function return `GpuDmaBuffer` or SPDK `DmaBuffer`? → A: Return SPDK `DmaBuffer` directly — immediately usable by SPDK NVMe operations without conversion.
- Q: Should the function accept a device index for multi-GPU peer access or use current CUDA context? → A: Accept optional device index parameter — function sets CUDA context internally.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Prepare GPU Memory for SSD DMA in One Call (Priority: P1)

A developer receives a base64-encoded CUDA IPC memory handle from a PyTorch inference process (transmitted over gRPC). They need to prepare that GPU memory so SPDK can perform peer-to-peer DMA directly from an NVMe SSD into the GPU buffer — without an intermediate CPU bounce buffer. The `prepare_memory_for_spdk` function handles the full preparation pipeline: opening the IPC handle with lazy peer access, checking pin state, pinning if necessary, and returning a DmaBuffer with the correct cleanup semantics.

**Why this priority**: This is the core feature — a single high-level entry point that combines IPC handle opening, peer access enablement, pin-state detection, conditional pinning, and DmaBuffer creation into one atomic operation.

**Independent Test**: Can be tested by providing a serialized IPC handle payload and verifying that a valid DmaBuffer is returned with the correct size and pointer, and that cleanup (drop) executes without error.

**Acceptance Scenarios**:

1. **Given** a valid base64-encoded CUDA IPC handle from a PyTorch process where the memory is NOT already pinned, **When** `prepare_memory_for_spdk` is called, **Then** the memory is pinned (logged), peer access is enabled with lazy semantics, and a DmaBuffer is returned whose free function unpins the memory before closing the IPC handle.
2. **Given** a valid base64-encoded CUDA IPC handle from a PyTorch process where the memory IS already pinned, **When** `prepare_memory_for_spdk` is called, **Then** peer access is enabled, no additional pinning occurs (logged), and a DmaBuffer is returned whose free function closes the IPC handle without unpinning.
3. **Given** an invalid or malformed base64 payload, **When** `prepare_memory_for_spdk` is called, **Then** an error is returned without leaking resources.
4. **Given** the component is not initialized, **When** `prepare_memory_for_spdk` is called, **Then** an error is returned indicating the component must be initialized first.

---

### User Story 2 - DmaBuffer Cleanup Respects Original Pin State (Priority: P1)

When a DmaBuffer obtained from `prepare_memory_for_spdk` is dropped, it must only unpin memory that the function itself pinned. Memory that was already pinned by the originating process must not be unpinned, since the originating PyTorch process still expects that memory to remain pinned.

**Why this priority**: Incorrect cleanup can corrupt memory state in the originating PyTorch process, causing silent data corruption or crashes.

**Independent Test**: Create two DmaBuffers — one from pre-pinned memory and one from unpinned memory — drop both and verify that only the second triggers an unpin operation.

**Acceptance Scenarios**:

1. **Given** a DmaBuffer created from originally-unpinned memory, **When** the buffer is dropped, **Then** the memory is unpinned and the IPC handle is closed.
2. **Given** a DmaBuffer created from originally-pinned memory, **When** the buffer is dropped, **Then** the IPC handle is closed but the memory is NOT unpinned.

---

### User Story 3 - Pin State Changes Are Logged (Priority: P2)

All pinning and non-pinning decisions made during `prepare_memory_for_spdk` are logged via the component's logger receptacle. This allows operators to audit DMA preparation behavior and diagnose issues with GPU memory state.

**Why this priority**: Observability is essential for debugging production GPU-SSD data paths but is not blocking for core functionality.

**Independent Test**: Connect a logger receptacle, call `prepare_memory_for_spdk`, and verify that appropriate log messages are emitted for pin-state detection and any pinning action taken.

**Acceptance Scenarios**:

1. **Given** a logger receptacle is connected and memory requires pinning, **When** `prepare_memory_for_spdk` is called, **Then** a log message indicates that pinning was performed.
2. **Given** a logger receptacle is connected and memory is already pinned, **When** `prepare_memory_for_spdk` is called, **Then** a log message indicates that memory was already pinned and no action was taken.
3. **Given** no logger receptacle is connected, **When** `prepare_memory_for_spdk` is called, **Then** the operation succeeds silently without error.

---

### Edge Cases

- What happens when cudaIpcOpenMemHandle fails (e.g., originating process has exited)?
- What happens when cudaPointerGetAttributes reports memory type other than device (e.g., managed memory)?
- What happens when peer access cannot be enabled between devices?
- What happens when the GPU memory region is zero-sized?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST provide a `prepare_memory_for_spdk` function on the `IGpuServices` interface that accepts a base64-encoded CUDA IPC memory handle payload (`&str`) and an optional device index (`Option<u32>`), and returns an SPDK `DmaBuffer` directly usable for peer-to-peer SSD-to-GPU DMA operations.
- **FR-002**: The function MUST open the IPC memory handle using `cudaIpcMemLazyEnablePeerAccess` flag to enable lazy peer-to-peer access.
- **FR-003**: The function MUST query the pin state of the GPU memory using `cudaPointerGetAttributes` before attempting to pin.
- **FR-004**: The function MUST pin the GPU memory if it is not already pinned, and skip pinning if the memory is already pinned.
- **FR-005**: The function MUST log pinning actions (pin performed, or already pinned) to the logger receptacle when connected.
- **FR-006**: The returned DmaBuffer MUST use a free function that unpins the memory on drop ONLY if the function itself performed the pinning (i.e., memory was not originally pinned).
- **FR-007**: The returned DmaBuffer MUST use a free function that does NOT unpin memory on drop if the memory was already pinned when the function was called.
- **FR-008**: Both free function variants MUST close the CUDA IPC memory handle on drop.
- **FR-009**: The function MUST return an error if the component has not been initialized.
- **FR-010**: The function MUST return an error if the IPC handle cannot be opened (invalid payload, originating process gone, etc.).
- **FR-011**: The function MUST return an error if peer access cannot be enabled.
- **FR-012**: The function MUST not leak GPU resources on any error path (partial operations must be rolled back).
- **FR-013**: The function MUST be gated behind the `spdk` feature flag, consistent with other DMA-related methods on the interface.
- **FR-014**: When a device index is provided, the function MUST set the CUDA device context to the specified GPU before opening the IPC handle and enabling peer access. When no device index is provided, the function MUST use the current CUDA device context.
- **FR-015**: The function MUST return an SPDK `DmaBuffer` (not `GpuDmaBuffer`), directly usable by the SPDK NVMe driver for DMA operations without requiring type conversion by the caller.

### Key Entities

- **CUDA IPC Handle Payload**: A base64-encoded binary blob containing a 64-byte `cudaIpcMemHandle_t` plus an 8-byte little-endian size field (72 bytes total), originating from a PyTorch process and transmitted via gRPC.
- **DmaBuffer**: The SPDK `DmaBuffer` type (from `spdk_types`) that wraps a GPU device pointer with custom allocation/free semantics, enabling the SPDK NVMe driver to perform peer-to-peer DMA transfers directly to/from GPU memory without intermediate conversion.
- **Pin State**: Whether the GPU memory backing the IPC handle was already registered for DMA by the originating process, determined via `cudaPointerGetAttributes`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A single function call prepares GPU IPC memory for SSD DMA without requiring the caller to manage intermediate steps (open, check pin state, pin, create buffer).
- **SC-002**: Memory that was pinned by the calling function is correctly unpinned on buffer drop; memory that was pre-pinned is left intact.
- **SC-003**: All pinning decisions are observable via the logger when connected.
- **SC-004**: No GPU memory or IPC handle leaks occur on any code path (success or error).
- **SC-005**: The function integrates with the existing `IGpuServices` interface pattern and follows the same error-handling conventions as existing methods.

## Assumptions

- The IPC handle payload format (64-byte handle + 8-byte LE size = 72 bytes, base64-encoded) is consistent with the existing `deserialize_ipc_handle` method and the PyTorch serialization convention.
- `cudaIpcMemLazyEnablePeerAccess` is the appropriate flag for this use case — it defers full peer access setup until first access, reducing latency on the preparation path.
- The originating PyTorch process remains alive for the lifetime of the DmaBuffer (IPC handles become invalid when the allocating process exits).
- GPU memory from PyTorch allocations is always device-type memory (not managed or host), consistent with PyTorch's default CUDA allocator.
- The SPDK DmaBuffer type is the standard type used by the block device layer for NVMe DMA operations.
- The logger receptacle is optional — operations succeed silently without it, consistent with existing component patterns.
- Multi-GPU systems are supported; the optional device index parameter allows callers to specify the target GPU explicitly rather than relying on thread-local CUDA device state.
