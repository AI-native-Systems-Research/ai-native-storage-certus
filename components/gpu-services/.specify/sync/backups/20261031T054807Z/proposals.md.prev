# Drift Resolution Proposals

Generated: 2026-09-09
Based on: drift-report 2026-09-09

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 3 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 0 |

All three drift items are the same intentional, tested change (`ecf2dbe3`,
2026-09-02, Gate #1 for multi-GPU data-parallel serving) plus pre-existing
idempotent hardening on the same method. Code is authoritative; resolve by
backfilling the spec text.

## Proposals

### Proposal 1: 001-gpu-cuda-services/FR-015

**Direction**: BACKFILL (Code → Spec)

**Current State**:
- Spec says: "Component MUST provide a `register_host_memory(ptr, size)` method
  (gated behind `spdk` feature) that page-locks the specified host memory
  region via `cudaHostRegister` (enabling async GPU DMA) and registers it with
  SPDK via `spdk_mem_register` (enabling NVMe DMA). If `cudaHostRegister`
  succeeds but `spdk_mem_register` fails, the method MUST roll back by calling
  `cudaHostUnregister` before returning the error."
- Code does (`src/lib.rs:960-1010`): registers with the **portable** flag
  `CUDA_HOST_REGISTER_PORTABLE` so the pinning holds across all GPU contexts;
  treats `cudaHostRegister` → `CUDA_ERROR_HOST_MEMORY_ALREADY_REGISTERED` (712)
  and `spdk_mem_register` → `rc == -16` (EBUSY) as success; rolls back
  `cudaHostUnregister` only when this call actually performed the CUDA
  registration (`we_registered_cuda`).

**Proposed Resolution** (append to FR-015):

> Registration uses the **portable** flag (`CUDA_HOST_REGISTER_PORTABLE`) so
> memory pinned under one device's context remains page-locked for transfers
> to/from every GPU — required for multi-GPU / data-parallel serving where one
> shared host pool feeds several devices. The method is idempotent: a
> `cudaHostRegister` result of `CUDA_ERROR_HOST_MEMORY_ALREADY_REGISTERED`, and
> an `spdk_mem_register` result of `EBUSY` (memory already registered, e.g. via
> `spdk_zmalloc`), are both treated as success. The `cudaHostUnregister`
> rollback on SPDK failure is performed only when this call actually performed
> the CUDA registration (i.e. not when the memory was already registered by a
> prior call or another owner).

**Rationale**: Introduced deliberately as Gate #1 for the 2-way DP arm
(`ecf2dbe3`), validated on 2×A100 (both GPUs served from one portable-pinned
pool, 0 PIN failures). The idempotent paths are long-standing and prevent
double-registration errors. Code is correct and load-bearing.

**Confidence**: HIGH

**Action**: [x] Approve

---

### Proposal 2: 001-gpu-cuda-services/FR-020

**Direction**: BACKFILL (Code → Spec)

**Current State**:
- Spec says: "Component MUST provide `allocate_pinned_dma_buffer(size)` ... that
  allocates page-locked host memory via `cudaHostAlloc` ..."
- Code does (`src/lib.rs:937-939`): allocates with `CUDA_HOST_ALLOC_PORTABLE`
  (changed from `CUDA_HOST_ALLOC_DEFAULT` in `ecf2dbe3`).

**Proposed Resolution** (append to FR-020):

> The allocation uses the **portable** flag (`CUDA_HOST_ALLOC_PORTABLE`) so the
> returned buffer stays page-locked across all GPU contexts and is usable as a
> source for async H2D copies to any device, not only the device current at
> allocation time (required for multi-GPU / data-parallel operation).

**Rationale**: Same Gate #1 change (`ecf2dbe3`); keeps a single shared pinned
pool zero-copy for every replica's GPU.

**Confidence**: HIGH

**Action**: [x] Approve

---

### Proposal 3: 002-gpu-ssd-dma-prepare/FR-019

**Direction**: BACKFILL (Code → Spec)

**Current State**:
- Spec says: "The interface MUST provide a `register_host_memory(ptr, size)`
  method ... that page-locks an existing host allocation via `cudaHostRegister`
  ... On partial failure (CUDA succeeds, SPDK fails), MUST roll back
  `cudaHostRegister` before returning error."
- Code does: identical method to spec-001 FR-015 — portable flag + idempotent
  `ALREADY_REGISTERED`/`EBUSY` handling + conditional rollback.

**Proposed Resolution** (append to FR-019, consistent with Proposal 1):

> Registration uses the portable flag (`CUDA_HOST_REGISTER_PORTABLE`) so the
> pinning is valid across all GPU contexts. The method is idempotent
> (`cudaHostRegister` → `HOST_MEMORY_ALREADY_REGISTERED` and `spdk_mem_register`
> → `EBUSY` are treated as success), and the `cudaHostUnregister` rollback is
> performed only when this call performed the CUDA registration.

**Rationale**: Same underlying method as spec-001 FR-015; keep both specs
consistent so the shared behavior is documented identically.

**Confidence**: HIGH

**Action**: [x] Approve
