# Functional Design: dispatcher-p2p (GPUDirect Storage Cold Path)

## Overview

The dispatcher-p2p component is a variant of the dispatcher component that adds a GPUDirect Storage cold-read path. When cache entries have been evicted from the DRAM memory-tier to NVMe SSDs, the P2P path reads data back using NVMe DMA directly into pre-allocated GPU BAR1 staging buffers, then issues device-to-device copies to the client's final GPU destination. This eliminates the host DRAM bounce present in the standard zero-copy path.

## Architecture

### Data Paths

**Hot path (unchanged)**: Client lookup → dispatch-map says MemoryTier → DMA from DRAM to client GPU. Same as standard dispatcher.

**Cold path (P2P)**: Client lookup → dispatch-map says BlockDevice → NVMe read into GPU BAR1 staging ring slot → cudaMemcpyAsync D2D from staging to client GPU destination → promote entry back to memory-tier.

**Cold path (DRAM fallback)**: If P2P ring not available → NVMe read into memory-tier DRAM slot → cudaMemcpyAsync H2D from DRAM to client GPU. Same as standard dispatcher.

### P2P Staging Ring

A pre-allocated ring of 64 GPU-resident staging buffers:
- Each slot: `cudaMalloc` (GPU device memory) → GDRCopy `gdr_pin_buffer` + `gdr_map` (BAR1 mapping) → `spdk_mem_register` (SPDK DMA target)
- The ring is allocated once at initialization and shared across all cold lookup threads
- Ring partitioning: 64 slots split into non-overlapping halves for concurrent thread access
- Effective queue depth per thread: 16 (prevents NVMe qpair saturation under 4+ client concurrency)

### Why Pre-allocated Ring (Not Per-Call BAR1)

The client's GPU memory arrives via `cudaIpcOpenMemHandle` (IPC). GDRCopy's `gdr_pin_buffer` returns EINVAL on IPC-opened memory — it only works with locally `cudaMalloc`'d pointers. Therefore:
- We cannot create BAR1 DMA buffers from the client's GPU destination pointer
- We must use pre-allocated staging buffers (locally malloc'd) and D2D copy to the IPC destination
- The D2D copy runs at GPU internal bandwidth (~600 GB/s) so the overhead is negligible

### Pipeline Algorithm (pipelined_ssd_to_gpu_p2p)

1. **Prime**: Submit up to `effective_qd` async NVMe reads into ring slots (ring_offset for thread partitioning)
2. **On each NVMe completion**:
   - Issue `cudaMemcpyAsync` D2D: ring slot dev_ptr → gpu_dst (on alternating CUDA stream)
   - Submit next NVMe read into a recycled slot
3. **Periodically sync**: Every `ring_size/2` completions, synchronize both CUDA streams before recycling ring slots
4. **Finalize**: Sync both streams after all chunks complete

### Path Selection

Determined at initialization:
- Attempt `P2pRing::new()` — if succeeds, cold lookups use P2P path
- If fails (GDRCopy unavailable, insufficient GPU memory), fall back to DRAM zero-copy path
- Decision stored in `OnceLock` global, immutable for component lifetime



## Interface

Implements `IDispatcher` — same interface as the standard dispatcher. Drop-in replacement. Selected at build time via YAML profile (`full-p2p.yaml` → `crate: dispatcher-p2p`).

## Performance Measurement

End-to-end performance is measured using `certus-api-bench_v2.py` (`apps/python/`), the pipelined benchmark that exercises hot/cold lookup mixes under multi-client concurrency.

## Dependencies

- `gpu-services` with `p2p` feature (provides `dma::create_spdk_dma_buffer_from_gpu_bar`, `cuda_ffi::cudaMalloc/cudaFree/cudaMemcpyAsync`)
- `gdrdrv` kernel module (GDRCopy)
- `nvidia-peermem` kernel module (PCIe P2P DMA)
- All standard dispatcher dependencies (SPDK, block-device, extent-manager, memory-tier, etc.)
