# Research: CUDA GPU Memory Allocation Test for P2P Pinning

**Feature**: 002-cuda-pin-test
**Date**: 2026-04-13

## R1: CUDA Runtime Dynamic Loading Strategy

**Decision**: Use the `libloading` crate to `dlopen` `libcudart.so` at runtime.

**Rationale**: The `libloading` crate provides a safe Rust wrapper around
`dlopen`/`dlsym` with proper lifetime management for loaded libraries and
symbols. It avoids raw `libc::dlopen` unsafe blocks and is well-maintained.
The test binary builds without any CUDA dependency; at runtime, if
`libcudart.so` is not found, the test skips gracefully.

**Library Loading Order**:
1. Try `libcudart.so` (generic symlink, preferred)
2. Fall back to `libcudart.so.12` (CUDA 12.x)
3. Fall back to `libcudart.so.11.0` (CUDA 11.x)
4. If none found, skip test with message

**Required Symbols**:
```c
cudaError_t cudaMalloc(void **devPtr, size_t size);
cudaError_t cudaFree(void *devPtr);
```

Where `cudaError_t` is `int` (0 = `cudaSuccess`).

**Alternatives Considered**:
- Raw `libc::dlopen`/`dlsym`: More unsafe code, manual symbol lifetime
  management. Rejected in favor of `libloading` which wraps this safely.
- Compile-time linking (`-lcudart`): Build fails without CUDA SDK, violates
  FR-006 (graceful skip). Rejected per clarification decision.
- `cuda-sys` or `cudarc` crates: Heavy dependencies, compile-time CUDA SDK
  requirement. Rejected per FR-005 (minimal bindings).

## R2: CudaMemory RAII Wrapper Design

**Decision**: A `CudaMemory` struct that owns the device pointer and a
reference to the loaded library, calling `cudaFree` on Drop.

**Rationale**: RAII ensures GPU memory is freed even if a test assertion panics,
satisfying SC-005 (cleanup on failure). The struct holds:
- `devptr: *mut std::ffi::c_void` — the CUDA device pointer
- A reference or `Arc` to the loaded `CudaRuntime` to ensure the library
  outlives the allocation

**Drop ordering**: `PinnedMemory` (from `nvidia-p2p-pin` library) MUST be
dropped before `CudaMemory`. The test controls this by scoping or explicit
unpin before the `CudaMemory` goes out of scope. If `PinnedMemory` is still
alive when `cudaFree` is called, the behavior is undefined (GPU memory freed
while still pinned).

**Alternatives Considered**:
- Manual cleanup without RAII: Risk of resource leaks on panic. Rejected.
- `scopeguard` crate: Adds a dependency for something trivially implemented
  via Drop. Rejected.

## R3: Test Skip Mechanism

**Decision**: Use `println!` + `return` at the start of each test function
when prerequisites are missing. Not `#[ignore]`.

**Rationale**: `#[ignore]` requires explicit `--ignored` flag to run, which
makes the test invisible in normal `cargo test` output. Instead, each test
calls a shared `check_prerequisites()` helper that returns
`Result<CudaRuntime, SkipReason>`. If `Err(SkipReason)`, the test prints a
clear message and returns early. The test counts as "passed" (not skipped or
ignored) but the output clearly shows it was not exercised.

**Skip Conditions** (checked in order):
1. `dlopen("libcudart.so")` fails → "CUDA runtime not available, skipping"
2. `cudaMalloc(64KB)` fails → "No CUDA GPU available, skipping"
3. `open("/dev/nvidia_p2p")` fails → "nvidia_p2p_pin module not loaded, skipping"

**Alternatives Considered**:
- `#[ignore]` attribute: Hidden by default, requires manual opt-in. Rejected.
- `#[cfg(feature = "cuda-test")]`: Requires feature flag, adds build
  complexity. Rejected.
- Custom test harness: Over-engineered for ~5 tests. Rejected.

## R4: Test File Organization

**Decision**: Two files in `rust/tests/`: `cuda_helpers.rs` (module) and
`cuda_pin_test.rs` (test functions).

**Rationale**: Rust integration tests in `tests/` are each compiled as
separate crates. To share the `CudaRuntime` and `CudaMemory` helpers across
test functions, they are placed in a `cuda_helpers.rs` module that is imported
via `mod cuda_helpers;` from `cuda_pin_test.rs`. This keeps the helper code
separate from test logic.

**Alternatives Considered**:
- Single file with everything: Mixes helper code with test logic, harder to
  maintain. Rejected.
- Helper as a separate crate: Over-engineered for a test utility. Rejected.
- Inline in `integration.rs`: Mixes CUDA concerns with non-CUDA integration
  tests. Rejected.
