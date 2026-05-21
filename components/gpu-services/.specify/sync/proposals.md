# Drift Resolution Proposals

Generated: 2026-05-21
Based on: drift-report from 2026-05-21

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 2 |
| Human Decision | 5 |

## Auto-Approved (Applied)

### Proposal 1: Naming - GpuServicesComponentV0 -> GpuServicesComponent

**Direction**: BACKFILL (Code -> Spec)

**Current State**:
- Spec quickstart files reference `GpuServicesComponentV0`
- Code uses `GpuServicesComponent` (no V0 suffix)

**Applied Resolution**: Renamed all occurrences of `GpuServicesComponentV0` to `GpuServicesComponent` in quickstart.md files for specs 001 and 002. Also updated constructor from `::new()` to `::new_default()` to match current API.

**Rationale**: The component was renamed during development. The V0 suffix is part of the directory path (`gpu-services/v0/`) not the type name. Specs must match the public API.

**Confidence**: HIGH
**Status**: applied

---

### Proposal 2: 001-gpu-cuda-services/FR-005 - Verification Caching

**Direction**: BACKFILL (Code -> Spec)

**Current State**:
- Spec says: "pin verifies device-residency and tracks state"
- Code does: `pin_memory` only calls `check_memory_attributes` if the pointer is NOT already in the verified set (optimization shortcut)

**Applied Resolution**: Added sentence to FR-005:

> As an optimization, `pin_memory` MAY skip re-verification (via `cudaPointerGetAttributes`) for pointers already present in the verified set, since verification is a prerequisite in the standard IPC workflow.

**Rationale**: This is a performance optimization, not a behavioral change. In the standard IPC workflow, `verify_memory` is always called before `pin_memory`, so re-verification is redundant. The spec now documents this as an intentional optimization.

**Confidence**: HIGH
**Status**: applied

---

## Human Decision Required (Not Applied)

The following 5 unspecced features require human decision on whether to write new specs:

1. **CUDA Stream API** (`create_stream`, `destroy_stream`, `stream_synchronize`)
   - Location: `src/lib.rs:539-605`, `interfaces/src/igpu_services.rs:486-537`
   - Suggested spec: `003-gpu-async-stream-ops` or `004-gpu-pipeline`

2. **Async DMA copy** (`dma_copy_to_device_async`)
   - Location: `src/lib.rs:608-660`
   - Suggested spec: `003-gpu-async-stream-ops` or `004-gpu-pipeline`

3. **Raw pointer async copy** (`memcpy_h2d_async`)
   - Location: `src/lib.rs:662-704`
   - Suggested spec: `003-gpu-async-stream-ops` or `004-gpu-pipeline`

4. **Pinned DMA buffer allocation** (`allocate_pinned_dma_buffer`)
   - Location: `src/lib.rs:706-744`
   - Suggested spec: `003-gpu-async-stream-ops` or `004-gpu-pipeline`

5. **GDRCopy P2P DMA path** (BAR1 mapping, VFIO IOMMU, cross-process physical address DMA)
   - Location: `src/dma.rs:353-720`, `src/bin/p2p_server.rs`, `tests/gpu_nvme_p2p.rs`
   - Suggested spec: `003-gpu-p2p-dma`
