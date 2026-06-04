Validation passes. Here's a summary of the iteration 3 design:

**Key pivot from iter-1/2:** Abandoned the GDRCopy BAR1 staging approach (proven fundamentally broken) in favor of **nvidia-peermem direct registration** — a completely different mechanism that was already validated in the codebase (`prepare_memory_for_spdk`) but never applied to the cold lookup path.

**The approach:**
1. Register the GPU IPC destination pointer with SPDK via `spdk_mem_register` (nvidia-peermem handles the IOMMU mapping)
2. Read NVMe directly into chunks of the GPU memory using the same sliding-window pipeline as baseline
3. No staging ring, no BAR1 mapping, no copies — data lands in the final location

**Expected outcome:** 3.0-5.5 GB/s cold lookup (vs 2.4 GB/s baseline) by eliminating one full PCIe traversal (the H2D copy).

**Three arms:** h-main (direct NVMe→GPU, no DRAM), h-ablation (direct + DRAM backfill for cache), h-control-negative (baseline).