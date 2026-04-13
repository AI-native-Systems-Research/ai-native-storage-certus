# Data Model: CUDA GPU Memory Allocation Test

**Feature**: 002-cuda-pin-test
**Date**: 2026-04-13

## Entities

### CudaRuntime (helper)

Represents a dynamically loaded CUDA runtime library.

| Field | Type | Description |
|-------|------|-------------|
| lib | `libloading::Library` | Loaded `libcudart.so` handle |

Lifetime: Created once per test (or per test file via lazy init). Dropped
after all `CudaMemory` instances that reference it.

**Methods**:
- `load() -> Result<CudaRuntime, SkipReason>`: Attempt to dlopen libcudart.so
- `malloc(size: usize) -> Result<CudaMemory, CudaError>`: Call cudaMalloc
- `free(ptr: *mut c_void) -> Result<(), CudaError>`: Call cudaFree

### CudaMemory (helper, RAII)

Represents a GPU device memory allocation from `cudaMalloc`.

| Field | Type | Description |
|-------|------|-------------|
| devptr | `*mut c_void` | CUDA device pointer |
| size | `usize` | Allocation size in bytes |
| runtime | reference to `CudaRuntime` | Ensures library outlives allocation |

Lifetime: Allocated via `CudaRuntime::malloc()`, freed on Drop via `cudaFree`.

**Drop ordering constraint**: Any `PinnedMemory` holding a pin on this device
pointer MUST be dropped (unpinned) before `CudaMemory` is dropped.

### SkipReason (enum)

| Variant | Description |
|---------|-------------|
| NoCudaRuntime | `libcudart.so` could not be loaded |
| NoGpu | `cudaMalloc` failed (no GPU or driver) |
| NoKernelModule | `/dev/nvidia_p2p` could not be opened |

### State Transitions

```text
CudaMemory lifecycle:

  [not exists]
      │
      ├─ CudaRuntime::malloc(size) ──> [ALLOCATED]
      │                                     │
      │                                     ├─ pin_gpu_memory(devptr, size) ──> [PINNED]
      │                                     │       │
      │                                     │       └─ unpin / Drop PinnedMemory ──> [ALLOCATED]
      │                                     │
      │                                     └─ Drop CudaMemory ──> cudaFree() ──> [freed]
      │
      └─ CudaRuntime::malloc(size) fails ──> [not exists]
```

**CRITICAL**: The transition from [PINNED] back to [ALLOCATED] (unpin) MUST
happen before [ALLOCATED] to [freed] (cudaFree). Dropping CudaMemory while
PinnedMemory is still alive is undefined behavior.
