## Summary

**Experiment complete. Validation passed.**

### Key Findings:

**h-main (P2P NVMe→GPU BAR1 direct): REFUTED**
- Baseline (NVMe→DRAM→GPU): **2.41-2.43 GB/s**, avg latency 1733 us, p99 1820 us
- P2P treatment: **1.95-1.98 GB/s**, avg latency 2132 us, p99 2256 us
- P2P is **19% slower** than baseline — direction opposite to prediction

**h-ablation (P2P without DRAM backfill): PARTIALLY CONFIRMED**
- Without backfill: **2.01-2.04 GB/s** — ~3% faster than h-main (correct direction, small magnitude)

### Root Cause of Regression:

The hypothesis assumed P2P eliminates a redundant data copy. In reality, the baseline already reads NVMe directly into the final DRAM destination (zero-copy), so P2P *adds* a copy step (D2D from BAR1 staging to final GPU destination) rather than removing one. The batch-based pipeline (needed for correctness) also loses the sliding-window overlap that the baseline enjoys.

### Principles Extracted:

1. **RP-1**: P2P with staging buffers loses to zero-copy DRAM when DRAM is already the final destination
2. **RP-2**: Batch pipelines are ~20% slower than sliding-window pipelines due to lost overlap
3. **RP-3**: DRAM backfill overhead is minimal (~3%) when serialized on the same stream