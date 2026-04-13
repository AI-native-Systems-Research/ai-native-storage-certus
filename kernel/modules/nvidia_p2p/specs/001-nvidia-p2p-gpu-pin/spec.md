# Feature Specification: NVIDIA P2P GPU Memory Pinning

**Feature Branch**: `001-nvidia-p2p-gpu-pin`
**Created**: 2026-04-13
**Status**: Draft
**Input**: User description: "Write a kernel module that allows a Rust user-space
library to access the NVIDIA driver kernel functions nvidia_p2p_get_pages() so
that GPU memory can be pinned. The user API should take a virtual address and
length, and get back a physical address for the pinned memory. An API to unpin
previously pinned memory should also be included. The purpose of pinning GPU
memory is to DMA transfer data directly from SSD into the GPU, and vice-versa."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Pin GPU Memory for DMA (Priority: P1)

A user-space application allocates GPU memory via CUDA (e.g., `cudaMalloc`),
then calls the Rust library to pin that GPU memory region. The library returns
the physical address(es) corresponding to the pinned GPU memory so the caller
can program an NVMe SSD controller to DMA data directly to/from the GPU.

**Why this priority**: This is the core functionality. Without pinning, no
GPUDirect Storage transfers are possible. Everything else depends on this.

**Independent Test**: Can be tested by allocating GPU memory via CUDA,
calling pin, verifying that physical addresses are returned, and confirming
the page table entry count matches the expected number of 64KB pages.

**Acceptance Scenarios**:

1. **Given** a valid CUDA-allocated GPU virtual address (64KB-aligned) and a
   length (multiple of 64KB), **When** the user calls `pin_gpu_memory(va, len)`,
   **Then** the call succeeds and returns a list of physical addresses, one per
   page, along with the page size used by the NVIDIA driver.
2. **Given** a GPU virtual address that is not 64KB-aligned, **When** the user
   calls `pin_gpu_memory(va, len)`, **Then** the call returns an
   `InvalidAlignment` error.
3. **Given** a length that is not a multiple of 64KB, **When** the user calls
   `pin_gpu_memory(va, len)`, **Then** the call returns an `InvalidLength` error.
4. **Given** the kernel module is not loaded, **When** the user calls
   `pin_gpu_memory(va, len)`, **Then** the call returns a `DeviceNotFound` error.

---

### User Story 2 - Unpin Previously Pinned GPU Memory (Priority: P1)

After a DMA transfer completes, the user-space application calls the Rust
library to unpin the GPU memory, releasing the P2P mapping and allowing the
NVIDIA driver to reclaim or migrate the memory.

**Why this priority**: Pinning without unpinning causes resource leaks. Pin and
unpin are co-equal in importance; both are required for a functional MVP.

**Independent Test**: Can be tested by pinning GPU memory, then unpinning it,
and verifying the call succeeds. A second unpin of the same region MUST return
an error (double-free detection).

**Acceptance Scenarios**:

1. **Given** a previously pinned GPU memory region, **When** the user calls
   `unpin_gpu_memory(handle)`, **Then** the call succeeds and the kernel module
   releases the P2P page mapping.
2. **Given** a handle that was already unpinned or never pinned, **When** the
   user calls `unpin_gpu_memory(handle)`, **Then** the call returns an
   `InvalidHandle` error.
3. **Given** the kernel module is unloaded while memory is pinned, **Then** all
   outstanding pinned regions MUST be automatically released during module
   cleanup.

---

### User Story 3 - Query Pinned Region Metadata (Priority: P2)

A user-space application queries a pinned memory handle to retrieve metadata
about the pinned region: the number of pages, page size, physical addresses,
and the GPU UUID.

**Why this priority**: Useful for diagnostics and for callers that need to
inspect the mapping details (e.g., to set up scatter-gather lists), but not
strictly required for basic pin/unpin functionality.

**Independent Test**: Can be tested by pinning memory, querying the handle,
and verifying returned metadata matches expected values.

**Acceptance Scenarios**:

1. **Given** a valid pinned memory handle, **When** the user calls
   `query_pinned_region(handle)`, **Then** the call returns the page count,
   page size enum, physical address list, and GPU UUID.
2. **Given** an invalid handle, **When** the user calls
   `query_pinned_region(handle)`, **Then** the call returns `InvalidHandle`.

---

### Edge Cases

- What happens when the NVIDIA driver unloads or the GPU is reset while pages
  are pinned? Since the persistent API is used, there is no free callback.
  The kernel module MUST call `nvidia_p2p_put_pages_persistent()` for all
  active regions during module cleanup. Subsequent user-space operations on
  invalidated handles MUST return an error.
- What happens when the user-space process crashes with pages still pinned?
  The kernel module MUST release all pinned pages associated with that file
  descriptor on `release` (file close).
- What happens when the same GPU virtual address range is pinned twice? The
  kernel module MUST reject the second pin with an `AlreadyPinned` error.
  No reference counting is used; each VA range has at most one active pin.
- What happens with extremely large pin requests (e.g., multiple GB)? The
  kernel module MUST respect the NVIDIA driver's limits and propagate any
  `-ENOMEM` errors.
- What happens on systems without an NVIDIA GPU or driver? The kernel module
  MUST fail to load gracefully with a clear error message in `dmesg`.

## Clarifications

### Session 2026-04-13

- Q: What permissions are required to use `/dev/nvidia_p2p`? → A: Root or `CAP_SYS_RAWIO` required.
- Q: Which NVIDIA P2P API variant should be used? → A: Persistent API only (`nvidia_p2p_get_pages_persistent` / `nvidia_p2p_put_pages_persistent`).
- Q: What happens when the same GPU VA range is pinned twice? → A: Reject with `AlreadyPinned` error (no reference counting).
- Q: Should there be a max concurrent pinned regions limit? → A: No artificial limit; rely on NVIDIA driver's own `-ENOMEM` for resource exhaustion.
- Q: What level of kernel logging should the module provide? → A: Standard: `pr_info` at load/unload, `pr_err` for errors, `pr_debug` for per-operation tracing (pin/unpin/query).

## Requirements *(mandatory)*

### Functional Requirements

#### Kernel Module (C)

- **FR-001**: The kernel module MUST expose a character device (e.g.,
  `/dev/nvidia_p2p`) with an ioctl interface for pin, unpin, and query
  operations.
- **FR-002**: The kernel module MUST call `nvidia_p2p_get_pages_persistent()`
  to pin GPU memory and obtain physical page addresses from the NVIDIA driver.
  The non-persistent `nvidia_p2p_get_pages()` API is not used.
- **FR-003**: The kernel module MUST call `nvidia_p2p_put_pages_persistent()`
  to unpin GPU memory. The non-persistent `nvidia_p2p_put_pages()` API is
  not used.
- **FR-004**: The kernel module MUST track all active pinned regions per
  open file descriptor, and release all of them on file close (`release`).
- **FR-006**: The kernel module MUST validate all ioctl input parameters
  (alignment, length, NULL pointers) before calling NVIDIA driver functions.
- **FR-007**: The kernel module MUST support kernel versions 5.14 and beyond,
  targeting RHEL 9 and RHEL 10.
- **FR-008**: The build system MUST auto-discover the `nv-p2p.h` header
  location at build time. The known reference path is
  `/usr/src/nvidia-580.126.20/nvidia/nv-p2p.h` (also available under the
  `nvidia-peermem` subdirectory). The build MUST NOT hard-code a specific
  driver version; it MUST glob `/usr/src/nvidia-*/nvidia/nv-p2p.h` (or
  `/usr/src/nvidia-*/nvidia-peermem/nv-p2p.h`) and select the newest
  matching installation. If no header is found, the build MUST fail with a
  clear error message indicating the NVIDIA driver development files are
  required.
- **FR-009**: The kernel module MUST use a mutex or similar synchronization to
  protect the pinned-regions data structure from concurrent ioctl access.
- **FR-011**: The kernel module MUST create the character device with mode 0600
  (owner root) and MUST check for `CAP_SYS_RAWIO` capability in the `open`
  file operation. Processes without root or `CAP_SYS_RAWIO` MUST receive
  `-EPERM`.
- **FR-012**: The kernel module MUST log at the following levels: `pr_info`
  for module load/unload events, `pr_err` for all error paths (failed pin,
  invalid ioctl parameters, NVIDIA driver errors), and `pr_debug` for
  per-operation tracing of pin/unpin/query calls (enabled via dynamic debug).
- **FR-010**: The kernel module MUST assign an opaque handle to each pinned
  region and return it to user space, so that unpin and query operations can
  identify the target without re-specifying the GPU virtual address.

#### User-Space Library (Rust)

- **FR-101**: The Rust library MUST provide a safe, ergonomic API:
  `pin_gpu_memory(va: u64, len: u64) -> Result<PinnedMemory, Error>` and
  `unpin` via `Drop` on `PinnedMemory` (with an explicit `unpin()` method
  also available).
- **FR-102**: The Rust library MUST open the character device and issue ioctls
  to the kernel module; all ioctl interaction MUST be encapsulated and not
  exposed to the caller.
- **FR-103**: The Rust library MUST minimize `unsafe` code. The only `unsafe`
  blocks permitted are for the raw ioctl syscall interface. All other logic
  MUST be safe Rust.
- **FR-104**: The Rust library MUST provide typed, descriptive error variants
  (e.g., `DeviceNotFound`, `InvalidAlignment`, `InvalidLength`,
  `InvalidHandle`, `AlreadyPinned`, `PermissionDenied`, `DriverError(i32)`).
- **FR-105**: The `PinnedMemory` struct MUST expose accessors for: list of
  physical addresses, page size, and page count.
- **FR-106**: The Rust library MUST implement `Drop` for `PinnedMemory` to
  automatically unpin memory when the handle goes out of scope, preventing
  resource leaks.
- **FR-107**: The Rust library MUST support RHEL 9 and RHEL 10 (glibc-based
  Linux targets).
- **FR-108**: The Rust library MUST provide a `PinnedMemory::physical_address()`
  convenience method that returns the base physical address when the pinned
  region is contiguous (single page), or the full list for multi-page regions.

### Key Entities

- **PinnedRegion** (kernel): Represents an active P2P mapping. Contains the
  NVIDIA page table pointer, GPU virtual address, length, handle ID, and
  associated file descriptor.
- **PinnedMemory** (Rust): User-facing handle wrapping the kernel handle. Holds
  the file descriptor, handle ID, physical addresses, page size, and page count.
- **NvP2pDevice** (Rust): Represents the open file descriptor to
  `/dev/nvidia_p2p`. Manages device lifetime and ioctl dispatch.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A CUDA application can pin a 1 MB GPU memory region and receive
  16 physical addresses (at 64KB page size) in under 10 ms.
- **SC-002**: Unpinning a previously pinned region completes in under 5 ms.
- **SC-003**: The kernel module passes all unit and integration tests with zero
  memory leaks (verified via `kmemleak` or equivalent).
- **SC-004**: The Rust library contains zero `unsafe` blocks beyond the ioctl
  interface layer.
- **SC-005**: The Rust library's Criterion benchmarks demonstrate pin/unpin
  round-trip latency for characterization of performance-sensitive paths.
- **SC-006**: The kernel module correctly handles process termination with
  outstanding pinned regions (no leaked P2P mappings).
- **SC-007**: The build system compiles on RHEL 9 (kernel 5.14) and RHEL 10
  without manual header path configuration.

## Assumptions

- The NVIDIA proprietary driver (with P2P support) is installed and loaded on
  the target system. The `nvidia_p2p_get_pages` symbol is exported by the
  NVIDIA kernel module.
- GPU memory to be pinned has been allocated via CUDA (`cudaMalloc` or
  equivalent) and resides in GPU-local memory.
- The `nv-p2p.h` header is available under `/usr/src/nvidia-*/` as installed
  by the NVIDIA driver packages (e.g.,
  `/usr/src/nvidia-580.126.20/nvidia/nv-p2p.h`). The exact version suffix
  varies by driver version and MUST be auto-discovered by the build system.
- The persistent P2P API (`nvidia_p2p_get_pages_persistent` /
  `nvidia_p2p_put_pages_persistent`) is used exclusively. This avoids
  p2p_token/va_space management and async callback complexity. The
  non-persistent API is not supported.
- The user-space caller is responsible for obtaining the GPU virtual address
  from CUDA (e.g., via `cuMemGetAddressRange` or `cudaPointerGetAttributes`)
  and ensuring it is 64KB-aligned with a 64KB-multiple length.
- MIG (Multi-Instance GPU) mode is not a target; the persistent API does not
  support MIG-enabled devices per NVIDIA documentation.
- This module does NOT itself perform DMA transfers. It only pins GPU memory
  and returns physical addresses. The actual DMA programming (e.g., NVMe
  submission queue entries) is handled by a separate SSD/storage driver or
  library.
