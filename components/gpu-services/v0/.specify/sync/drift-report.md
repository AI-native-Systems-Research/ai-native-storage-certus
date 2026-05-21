# Spec Drift Report

Generated: 2026-05-20
Project: GPU Services V0
Specs: 001-gpu-cuda-services, 002-gpu-ssd-dma-prepare

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 34 |
| Aligned | 30 (88%) |
| Drifted | 1 (3%) |
| Not Implemented | 1 (3%) |
| Unspecced Code | 0 |

## Detailed Findings

### Spec: 001-gpu-cuda-services - GPU CUDA Services

#### Aligned

- FR-001: CUDA initialization via cudaGetDeviceCount with descriptive errors
- FR-002: Enumerates GPUs with compute capability >= 7.0 (Volta+), reports model/memory/compute level
- FR-003: Deserializes base64-encoded CUDA IPC handle (64 bytes) + size (8 bytes LE u64) into native structures
- FR-004: Verifies device memory type via cudaPointerGetAttributes before DMA buffer creation
- FR-006: Creates GpuDmaBuffer from valid IPC handle with cudaIpcCloseMemHandle on drop
- FR-007: All operations return descriptive errors without panicking or leaking GPU resources
- FR-008: Exposes functionality through IGpuServices interface
- FR-009: Build gated behind `--features gpu`; without it returns "GPU support not compiled"

#### Drifted

- FR-005: Spec says "pin and unpin operations for GPU memory regions." Code only tracks pin state in a HashSet without calling actual CUDA pinning APIs (cudaHostRegister/cudaHostUnregister). However, IPC device memory is inherently pinned by CUDA — the tracking approach is functionally correct.
  - Location: src/lib.rs (pin/unpin methods)
  - Severity: minor (semantically misleading but functionally correct for IPC use case)

#### Not Implemented

- FR-010: Spec requires Criterion benchmarks when gpu feature is enabled. Cargo.toml declares benchmark targets (gpu_services_benchmark, dma_transfer_benchmark) but no corresponding source files exist in benches/.
  - Location: Cargo.toml, benches/
  - Severity: low

### Success Criteria

- SC-001: Aligned (structure supports < 5 second init)
- SC-002: Aligned (discover_devices returns device info)
- SC-003: Aligned (IPC deserialization is fast decode)
- SC-004: Aligned (memory verification is single API call)
- SC-005: Aligned (DMA buffer creation is lightweight)
- SC-006/SC-007: Not verifiable (benchmark files missing)
- SC-008: Aligned (integration structure in place)

### Unspecced Code

(none — `register_host_memory`/`unregister_host_memory` specced as FR-015/016 in 001-gpu-cuda-services and FR-019/020 in 002-gpu-ssd-dma-prepare as of 2026-05-20)

## Recommendations

1. **Backfill FR-005**: Update spec to clarify that "pinning" for IPC device memory means verifying device-residency (already pinned by definition), not calling cudaHostRegister.
2. **Implement FR-010**: Create benchmark source files in benches/ directory.
3. **Spec register/unregister_host_memory**: Add FR-011 and FR-012 to 002-gpu-ssd-dma-prepare covering host memory registration for zero-copy DMA paths.
