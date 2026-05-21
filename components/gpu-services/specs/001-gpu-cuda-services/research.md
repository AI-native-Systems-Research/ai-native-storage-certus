# Research: GPU CUDA Services

**Date**: 2026-04-29
**Feature**: GPU CUDA Services (001)

## R1: CUDA FFI from Rust — Binding Strategy

**Decision**: Use hand-written `extern "C"` FFI bindings for the small
subset of CUDA Runtime API functions needed, rather than full bindgen
generation of the entire CUDA header set.

**Rationale**: The component requires only ~15 CUDA functions
(`cudaGetDeviceCount`, `cudaGetDeviceProperties`, `cudaIpcOpenMemHandle`,
`cudaIpcCloseMemHandle`, `cudaHostRegister`, `cudaHostUnregister`,
`cudaMemGetInfo`, `cudaSetDevice`, `cudaDeviceSynchronize`,
`cudaPointerGetAttributes`, `cudaFree`). Hand-written bindings are
easier to audit for safety, avoid pulling the entire CUDA toolkit
headers into the build, and match the existing `spdk-sys` pattern of
minimal FFI surfaces.

**Alternatives considered**:
- `cuda-sys` crate: Unmaintained, lags behind CUDA versions.
- Full bindgen on `cuda_runtime_api.h`: Generates thousands of
  bindings, most unused; increases compile time and audit burden.

## R2: IPC Handle Serialization Format

**Decision**: The Python client serializes a CUDA IPC handle as:
`base64(ipc_handle_bytes[64] || size_le_u64[8])` — a 72-byte payload
encoded to base64, yielding a 96-character string. The Rust side
decodes base64, splits at byte 64, and interprets the first 64 bytes
as `cudaIpcMemHandle_t` and the last 8 as little-endian u64 size.

**Rationale**: `cudaIpcMemHandle_t` is a fixed 64-byte opaque struct
in all CUDA versions. Concatenating the size as LE u64 keeps the wire
format simple with no framing overhead. Base64 is transport-safe for
Unix domain sockets and debugging.

**Alternatives considered**:
- JSON with hex-encoded handle: More overhead, no benefit for binary
  data.
- Protocol Buffers: Overkill for a fixed-size 72-byte message.
- Raw binary on socket: Works but harder to debug; base64 is human-
  readable in logs.

## R3: GPU Memory Contiguity Verification

**Decision**: Use `cudaPointerGetAttributes` to verify that an IPC-
opened pointer is device memory (not managed/host) and then rely on
CUDA's guarantee that `cudaMalloc`-allocated buffers are physically
contiguous within the GPU's virtual address space. Pin status is
verified by confirming the memory type is `cudaMemoryTypeDevice` (device
memory is always pinned on the GPU side).

**Rationale**: CUDA device memory allocated via `cudaMalloc` is always
contiguous and effectively pinned (not pageable). The verification step
confirms the memory type matches expectations, catching cases where
the Python side accidentally passes a managed memory handle or host
pointer.

**Alternatives considered**:
- `cuMemGetAddressRange`: Requires driver API, adds complexity for
  the same information.
- Skip verification: Unacceptable per constitution (correctness
  assurance).

## R4: DMA Buffer Creation from GPU IPC Handle

**Decision**: After opening the IPC handle via `cudaIpcOpenMemHandle`,
create a `DmaBuffer` using the existing `DmaBuffer::from_raw()` method
with a custom `free_fn` that calls `cudaIpcCloseMemHandle`. This
integrates GPU memory into the existing SPDK DMA pipeline without
modifying the `DmaBuffer` type.

**Rationale**: `DmaBuffer::from_raw` already supports external memory
with caller-supplied deallocation. The GPU pointer obtained from
`cudaIpcOpenMemHandle` is valid for DMA (GpuDirect RDMA / peer access)
and the deallocation function ensures proper cleanup.

**Alternatives considered**:
- New GpuDmaBuffer type: Duplicates DmaBuffer functionality; violates
  maintainability principle.
- Modify DmaBuffer struct: Unnecessary; `from_raw` already provides
  the exact extension point needed.

## R5: Python-Rust IPC Transport (Unix Domain Socket)

**Decision**: Use a stream-oriented Unix domain socket at a well-known
path (`/tmp/gpu-services-ipc.sock`). Protocol: the Python client
connects, sends a 4-byte little-endian length prefix followed by the
base64-encoded payload, then awaits a 1-byte ACK (0x01 = success,
0x00 = error). The Rust server listens, accepts one connection at a
time, and processes the handle.

**Rationale**: Simple, reliable, no external dependencies. Length-
prefixed framing avoids partial-read issues. Unix sockets provide
automatic access control via filesystem permissions. The test apps are
single-client demonstrations, so concurrent handling is not required.

**Alternatives considered**:
- TCP localhost: Unnecessary network stack overhead for same-machine
  IPC.
- Named pipe (FIFO): Unidirectional; would need two for request/
  response.
- Shared memory: More complex setup for a simple handle handoff.

## R6: Feature Gate Integration

**Decision**: Add a `gpu` feature to the `gpu-services` Cargo.toml
that gates all CUDA-dependent code. When `gpu` is not enabled, the
crate still builds but `IGpuServices` methods return an error
indicating GPU support was not compiled in. This ensures the crate
remains a workspace default-member without breaking builds on systems
without CUDA.

**Rationale**: Matches the existing `spdk` feature gate pattern in
the interfaces crate. The component skeleton (define_component! +
basic lifecycle) remains available unconditionally for interface
discovery and wiring, while actual GPU operations require the feature.

**Alternatives considered**:
- Exclude from default-members: Would break `cargo test --all` for
  non-GPU contributors.
- Runtime-only detection: Doesn't eliminate the CUDA link dependency
  at build time.

## R7: Minimum Compute Capability Enforcement

**Decision**: During `initialize()`, enumerate all GPUs via
`cudaGetDeviceProperties` and filter to those with
`major >= 7` (Volta+). Store only qualifying devices in the component
state. If no qualifying GPU is found, return an error.

**Rationale**: Compute capability 7.0+ guarantees the IPC and memory
management features needed for reliable cross-process DMA. Pre-Volta
GPUs have limited IPC handle support and different memory management
semantics.

**Alternatives considered**:
- Accept all GPUs and fail at operation time: Poor UX; errors would
  be cryptic and late.
- Compile-time CUDA version check: Doesn't account for actual hardware
  present.
