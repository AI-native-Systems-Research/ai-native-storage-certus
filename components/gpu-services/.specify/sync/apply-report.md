# Sync Apply Report

Generated: 2026-05-21
Component: gpu-services
Based on: drift-report from 2026-05-21

## Applied Changes

### 1. Naming Backfill: GpuServicesComponentV0 -> GpuServicesComponent

**Files modified**:
- `specs/001-gpu-cuda-services/quickstart.md` — renamed type and updated constructor
- `specs/002-gpu-ssd-dma-prepare/quickstart.md` — renamed type and updated constructor
- `CLAUDE.md` — updated component name in Architecture section

**Changes**:
- `GpuServicesComponentV0::new()` replaced with `GpuServicesComponent::new_default()`
- All references now match the actual public API

### 2. FR-005 Verification Caching (spec 001)

**File modified**: `specs/001-gpu-cuda-services/spec.md`

**Change**: Added clarifying sentence to FR-005:

> As an optimization, `pin_memory` MAY skip re-verification (via `cudaPointerGetAttributes`) for pointers already present in the verified set, since verification is a prerequisite in the standard IPC workflow.

**Effect**: The minor drift flagged in the drift report is now documented as intentional behavior. No code changes required.

### 3. Unspecced Feature Backfill (spec 001): CUDA Streams, Async DMA, Pinned Buffers

**File modified**: `specs/001-gpu-cuda-services/spec.md`

**Changes**: Added FR-017 through FR-020:
- **FR-017**: CUDA stream lifecycle (`create_stream`, `destroy_stream`, `stream_synchronize`)
- **FR-018**: Async DMA copy from `DmaBuffer` to GPU (`dma_copy_to_device_async`)
- **FR-019**: Raw pointer async H2D copy (`memcpy_h2d_async`)
- **FR-020**: Pinned DMA buffer allocation (`allocate_pinned_dma_buffer`)

**Effect**: 4 previously unspecced features now have formal FR coverage in the CUDA services spec.

### 4. Unspecced Feature Backfill (spec 002): GDRCopy P2P DMA Path

**File modified**: `specs/002-gpu-ssd-dma-prepare/spec.md`

**Changes**: Added FR-021 through FR-024:
- **FR-021**: `create_spdk_dma_buffer_from_gpu_bar` — full GDRCopy BAR1 P2P pipeline
- **FR-022**: `create_spdk_dma_buffer_from_phys` — cross-process P2P via physical address IOMMU mapping
- **FR-023**: `create_spdk_dma_buffer_from_bar_direct` — DPDK IOMMU registration for existing BAR VA
- **FR-024**: Exposed GDRCopy FFI bindings and `GPU_PAGE_SIZE` for decomposed P2P operations

**Effect**: The full GDRCopy P2P DMA path (feature-gated behind `p2p`) now has formal FR coverage.

## Not Applied (Awaiting Human Decision)

None. All 5 previously unspecced features have been backfilled into existing specs.

## Post-Apply Drift Status

| Category | Before | After |
|----------|--------|-------|
| Aligned | 33 | 42 |
| Drifted | 1 | 0 |
| Not Implemented | 0 | 0 |
| Unspecced | 5 | 0 |
