## Summary

**Iteration 2 complete. Both hypotheses REFUTED.**

Key findings:
- **h-main (sliding-window P2P):** 0.01 GB/s vs 2.37 GB/s baseline — **140x slower**. The sliding-window pattern works correctly (integrity passes), but the fundamental bottleneck is the BAR1→GPU staging copy. Two issues discovered:
  1. **GPU L2 cache coherence (RP-4):** External PCIe DMA writes to BAR1 don't invalidate GPU L2, so D2D copies from dev_ptr read stale data (data corruption).
  2. **BAR1 VA is pageable memory:** The only correct path (H2D from BAR1 VA) uses CUDA's pageable memory path (~10ms per 128 KiB) because GDRCopy-mapped VAs aren't recognized as pinned memory.

- **h-ablation (P2P + backfill):** 47% slower than h-main due to CPU reads from BAR1 traversing PCIe with per-cacheline latency.

**Conclusion:** GPUDirect P2P with a staging ring buffer is a dead end for this architecture. The path forward would require either: (a) pinning the FINAL GPU destination directly for NVMe DMA (not possible with IPC handles), or (b) using NVIDIA's GPUDirect Storage (cuFile) API which handles the L2 cache coherence internally through a kernel driver.