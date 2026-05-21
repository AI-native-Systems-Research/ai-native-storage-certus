# Drift Resolution Proposals

Generated: 2026-05-20
Based on: drift-report from 2026-05-20

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 2 |
| Align (Spec -> Code) | 1 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: 001-gpu-cuda-services/FR-005

**Direction**: BACKFILL (Code -> Spec)

**Current State**:
- Spec says: "Component MUST provide pin and unpin operations for GPU memory regions"
- Code does: "Tracks pin state in HashSet without calling cudaHostRegister/cudaHostUnregister"

**Proposed Resolution**: Update FR-005 to:

> FR-005: Component MUST provide pin and unpin operations for GPU memory. For IPC-opened device memory (which is inherently pinned by the CUDA runtime), pin verifies device-residency and tracks state; unpin releases tracking. Pin is idempotent (returns Ok if already pinned).

**Rationale**: cudaHostRegister is for host memory, not device memory. IPC device memory obtained via cudaIpcOpenMemHandle is always pinned by definition. The code's approach of verifying + tracking is the correct behavior for this use case.

**Confidence**: HIGH

---

### Proposal 2: 001-gpu-cuda-services/FR-010

**Direction**: ALIGN (Spec -> Code)

**Current State**:
- Spec says: "Component MUST include Criterion benchmarks when gpu feature enabled"
- Code does: "Cargo.toml declares benchmark targets but no source files exist"

**Proposed Resolution**: Create benchmark source files:
- `benches/gpu_services_benchmark.rs` — initialization, device discovery, IPC deserialization latency
- `benches/dma_transfer_benchmark.rs` — DMA buffer creation throughput

Both should skip gracefully if no GPU hardware is present.

**Rationale**: The spec requirement is valid — benchmarks document performance characteristics and detect regressions. The Cargo.toml already expects these files.

**Confidence**: HIGH

---

### Proposal 3: Unspecced - register_host_memory / unregister_host_memory

**Direction**: BACKFILL (Code -> Spec)

**Feature**: Two new `IGpuServices` methods that call `cudaHostRegister`/`cudaHostUnregister` + `spdk_mem_register`/`spdk_mem_unregister` on existing host allocations to enable zero-copy NVMe and GPU DMA.
**Location**: `src/lib.rs` (register_host_memory, unregister_host_memory implementations), `interfaces/src/igpu_services.rs:589-618`

**Proposed Addition to Spec 002-gpu-ssd-dma-prepare** (FR-011, FR-012):

- **FR-011**: Component MUST provide a `register_host_memory(ptr, size)` method (gated behind `#[cfg(feature = "spdk")]`) that page-locks the specified host memory region via `cudaHostRegister` (enabling async GPU DMA from it) and registers it with SPDK via `spdk_mem_register` (enabling NVMe DMA to/from it). If `cudaHostRegister` succeeds but `spdk_mem_register` fails, the method MUST roll back by calling `cudaHostUnregister` before returning the error.

- **FR-012**: Component MUST provide an `unregister_host_memory(ptr, size)` method (gated behind `#[cfg(feature = "spdk")]`) that unregisters the memory from SPDK via `spdk_mem_unregister` then removes page-locking via `cudaHostUnregister`. MUST be called before freeing the underlying allocation.

**Rationale**: These methods are the enabler for the dispatcher's zero-copy pipeline — the memory-tier pool is registered once at init, allowing NVMe to read directly into it and GPU to async-DMA from it. Without them, intermediate buffer copies are required on every cold-path promotion.

**Confidence**: HIGH
