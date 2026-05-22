Validation passes. All five phases are complete.

**Summary:**

The experiment ran 6 conditions (3 modes × 2 seeds each) with 50 iterations of 4 MiB transfers.

**h-main — REFUTED:** The bounce hypothesis was wrong in direction. P2P warm (NVMe→GPU-BAR1→D2D) is ~2x *faster* than bounce (NVMe→host-DMA→H2D): 3031 MB/s vs 1511 MB/s avg throughput, 1.32ms vs 2.65ms avg latency. The D2D copy (intra-GPU, no PCIe traversal) is dramatically cheaper than H2D, and that dominates over any NVMe DMA target overhead.

**h-control-negative — CONFIRMED:** P2P cold is 5.6x slower than warm (7.49ms vs 1.32ms avg), confirming ~6ms GDRCopy per-request kernel overhead for 32 chunks. Pre-allocated pool amortization is essential for P2P performance.

**Principle updates:** RP-1 and RP-2 were updated to reflect the actual observed behavior — P2P warm is the faster path, not bounce.