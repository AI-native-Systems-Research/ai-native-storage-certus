Validation passes. Here's a brief summary:

**Iteration 2 design complete — validated.**

The experiment tests whether adding NVMe queue depth (QD=32 via `Command::BatchSubmit`) to dispatcher v0's pipelined read path reduces 4 MiB SSD-tier lookup latency below iter-1's 9,659 us result:

- **h-main**: BatchSubmit of 32 ReadAsync + per-chunk cudaMemcpyAsync on completion. Predicted: 1,200-2,500 us (exploiting NVMe flash die parallelism).
- **h-ablation**: Same BatchSubmit QD=32 but with single synchronous cudaMemcpy after all reads complete. Tests whether async GPU copy overlap matters when NVMe parallelism already dominates.

Key insight: iter-1's remaining 9,600 us is 32 sequential NVMe reads at QD=1. BatchSubmit collapses these to approximately one read latency (~300-800 us) by submitting all simultaneously.