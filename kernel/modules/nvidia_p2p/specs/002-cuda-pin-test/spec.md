# Feature Specification: CUDA GPU Memory Allocation Test for P2P Pinning

**Feature Branch**: `002-cuda-pin-test`
**Created**: 2026-04-13
**Status**: Draft
**Input**: User description: "Add a test in Rust, that uses the cudaMalloc API
to allocate memory that can be pinned by the driver."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Allocate and Pin GPU Memory End-to-End (Priority: P1)

A developer runs a Rust integration test that allocates GPU memory via the CUDA
runtime API (`cudaMalloc`), passes the resulting device pointer to the
`nvidia-p2p-pin` library's `pin_gpu_memory()` function, verifies that physical
addresses are returned, and then cleans up by unpinning and freeing the GPU
memory.

**Why this priority**: This is the primary end-to-end validation that the
kernel module and Rust library work correctly with real CUDA-allocated GPU
memory. Without this test, correctness of the full stack cannot be verified.

**Independent Test**: Can be run as a standalone `cargo test` invocation on
a system with the kernel module loaded and an NVIDIA GPU present.

**Acceptance Scenarios**:

1. **Given** a system with an NVIDIA GPU, loaded CUDA runtime, and the
   `nvidia_p2p_pin` kernel module loaded, **When** the test allocates 1 MB of
   GPU memory via `cudaMalloc` and calls `pin_gpu_memory()` with the returned
   device pointer, **Then** the pin call succeeds and returns physical
   addresses with the expected page count (16 pages at 64KB page size).
2. **Given** a successful pin of CUDA-allocated GPU memory, **When** the test
   unpins the memory and then frees it via `cudaFree`, **Then** both operations
   succeed without errors.
3. **Given** a successful pin, **When** the test inspects the returned physical
   addresses, **Then** every address is non-zero and the page count matches
   `ceil(allocation_size / page_size)`.

---

### User Story 2 - Validate Alignment Requirements (Priority: P2)

A developer runs a Rust test that verifies `cudaMalloc` returns a pointer
meeting the 64KB alignment requirement of the NVIDIA P2P API, confirming that
standard CUDA allocations are compatible with the pinning interface.

**Why this priority**: Validates the assumption that `cudaMalloc` returns
suitably aligned pointers, catching any environment-specific alignment issues
before they surface in production.

**Independent Test**: Run as part of the same integration test suite.

**Acceptance Scenarios**:

1. **Given** a CUDA-allocated device pointer, **When** the test checks
   alignment, **Then** the pointer is 64KB-aligned (address % 65536 == 0).
2. **Given** multiple CUDA allocations of varying sizes (64KB, 1 MB, 16 MB),
   **When** each is checked for alignment, **Then** all are 64KB-aligned.

---

### Edge Cases

- What happens when CUDA is not available (no GPU or driver not loaded)?
  The test MUST be skipped gracefully with a clear message indicating that
  the CUDA runtime is not available, rather than failing with a cryptic error.
- What happens when the kernel module is not loaded? The test MUST detect
  this condition (device open fails) and skip with a message indicating the
  module needs to be loaded.
- What happens with the minimum possible allocation size (64KB)? The test
  MUST verify that the smallest valid allocation can be pinned and returns
  exactly 1 page.

## Clarifications

### Session 2026-04-13

- Q: How should CUDA runtime (`libcudart.so`) be linked? → A: Runtime `dlopen` (builds without CUDA SDK, skips gracefully at runtime when CUDA is absent).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The test MUST use the CUDA runtime API (`cudaMalloc` /
  `cudaFree`) to allocate and free GPU device memory from Rust.
- **FR-002**: The test MUST call the `nvidia-p2p-pin` library's
  `pin_gpu_memory()` with the CUDA-allocated device pointer and verify the
  returned physical addresses.
- **FR-003**: The test MUST call `unpin_gpu_memory()` (or allow Drop) before
  calling `cudaFree()` to ensure correct resource cleanup ordering.
- **FR-004**: The test MUST be implemented as a Rust integration test using
  `#[test]` attributes, runnable via `cargo test`.
- **FR-005**: The test MUST load `libcudart.so` at runtime via `dlopen`
  (not compile-time linking) and resolve `cudaMalloc` and `cudaFree` symbols
  dynamically. This ensures the test builds on systems without the CUDA SDK.
  Bindings MUST be minimal and contained within the test or a small helper
  module. Avoid pulling in a large CUDA wrapper crate.
- **FR-006**: The test MUST skip gracefully (not fail) when prerequisites
  are missing: no NVIDIA GPU, CUDA runtime not installed, or kernel module
  not loaded.
- **FR-007**: The test MUST verify the full lifecycle: allocate -> pin ->
  validate physical addresses -> unpin -> free.
- **FR-008**: The test MUST validate that page count matches expected value
  based on allocation size and reported page size.

### Key Entities

- **CudaMemory**: A RAII wrapper around a CUDA device pointer obtained from
  `cudaMalloc`, which calls `cudaFree` on Drop.
- **PinnedMemory**: The existing handle from the `nvidia-p2p-pin` library
  representing pinned GPU memory.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The test passes on a system with NVIDIA GPU + CUDA + loaded
  kernel module, completing the full allocate/pin/validate/unpin/free cycle.
- **SC-002**: The test skips cleanly (not fails) on systems without GPU or
  CUDA, producing a human-readable skip message.
- **SC-003**: The test validates that all returned physical addresses are
  non-zero and page count matches expected value for the allocation size.
- **SC-004**: The test completes the full lifecycle in under 30 seconds.
- **SC-005**: The test cleans up all resources (GPU memory freed, pages
  unpinned) even if assertions fail mid-test.

## Assumptions

- The CUDA runtime library (`libcudart.so`) is available on the test system
  at runtime. It is loaded via `dlopen` — the CUDA SDK is NOT required at
  build time. If `libcudart.so` cannot be loaded, the test skips gracefully.
- `cudaMalloc` returns device pointers that are at least 64KB-aligned, which
  is the standard behavior for CUDA runtime allocations.
- The `nvidia_p2p_pin` kernel module (from feature 001-nvidia-p2p-gpu-pin)
  is built and loaded prior to running these tests.
- The `nvidia-p2p-pin` Rust library crate is available as a workspace
  dependency or path dependency.
- Tests are run as root or with `CAP_SYS_RAWIO` (required by the kernel
  module per feature 001 clarifications).
