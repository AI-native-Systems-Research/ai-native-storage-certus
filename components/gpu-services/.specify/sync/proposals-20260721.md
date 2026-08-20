# Drift Resolution Proposals

Generated: 2026-07-21
Based on: drift-report from 2026-07-21

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 2 |
| Human Decision | 0 |

Both proposals add new functional requirements to spec
`001-gpu-cuda-services`. They document capabilities already present in the code
(`set_device`, `device_of_ptr`) and do not contradict any existing requirement.

## Proposals (Awaiting Approval)

### Proposal 1: 001-gpu-cuda-services / FR-021 - set_device

**Direction**: BACKFILL (Code -> Spec)

**Current State**:
- Spec 001 has no requirement for selecting the calling thread's CUDA device.
- Code provides `set_device(device)` (src/lib.rs:566-592;
  interfaces/src/igpu_services.rs:555), wrapping `cudaSetDevice`.

**Proposed Resolution**: Add FR-021 to spec 001-gpu-cuda-services:

> **FR-021**: Component MUST provide a `set_device(device)` method that binds
> the calling thread's current CUDA device context to the specified GPU ordinal
> via `cudaSetDevice`, so that subsequently-created streams and issued transfers
> target that GPU. This is required for multi-GPU / tensor-parallel operation,
> where each device must be selected before a stream is created on it or a
> `cudaMemcpyAsync` is issued to a pointer resident on it (a stream is bound to
> the device that was current at creation, and `cudaMemcpyAsync` rejects a
> destination pointer on a different device). CUDA tracks the current device per
> OS thread. MUST return an error if GPU support is not compiled, the component
> is not initialized, or the device ordinal is invalid.

**Rationale**: New multi-GPU capability. `cudaSetDevice` is a per-thread CUDA
requirement; without an explicit selection method the interface cannot direct
stream creation or transfers to a chosen device. No existing FR covered device
selection.

**Confidence**: HIGH

**Approval**: [x] Approved (applied 2026-07-21)

---

### Proposal 2: 001-gpu-cuda-services / FR-022 - device_of_ptr

**Direction**: BACKFILL (Code -> Spec)

**Current State**:
- Spec 001 has no requirement for querying the owning device of a pointer.
- Code provides `device_of_ptr(ptr)` (src/lib.rs:594-633;
  interfaces/src/igpu_services.rs:577), wrapping `cudaPointerGetAttributes`.

**Proposed Resolution**: Add FR-022 to spec 001-gpu-cuda-services:

> **FR-022**: Component MUST provide a `device_of_ptr(ptr)` method that returns
> the CUDA device ordinal owning a given device pointer via
> `cudaPointerGetAttributes`. It MUST return `-1` for a pointer with no device
> association (e.g. plain host or unregistered memory). This is used to route a
> transfer to a stream on the pointer's own device and to reject cross-device
> pointers. MUST return an error if GPU support is not compiled, the component
> is not initialized, or the attribute query fails.

**Rationale**: New multi-GPU routing/safety capability. Complements FR-021: to
issue a transfer correctly, the caller must know which device owns the target
pointer and select that device. No existing FR exposed device ownership of a
pointer (FR-004 only verifies device-type residency for DMA gating).

**Confidence**: HIGH

**Approval**: [x] Approved (applied 2026-07-21)
</content>
