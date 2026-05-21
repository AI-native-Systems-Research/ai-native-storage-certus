# Drift Resolution Proposals

Generated: 2026-05-05
Based on: drift-report from 2026-05-05

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 1 |
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
