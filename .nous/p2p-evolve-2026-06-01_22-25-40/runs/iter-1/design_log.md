Validation passes. Here's a summary of what was designed:

**Iteration 1: Pipeline Parallelism Experiment**

The hypothesis bundle tests whether the cold lookup throughput bottleneck (~2.4 GB/s vs 5.4 GB/s NVMe ceiling) is caused by insufficient pipeline parallelism in the dispatcher:

1. **h-main**: Increase NVMe queue depth 16→64 and CUDA streams 2→4. The current QD16 with 128 KiB chunks only keeps 2 MB in-flight — far too little to saturate a Gen4 NVMe drive.

2. **h-ablation**: Increase chunk size 128 KiB→512 KiB (queue depth unchanged). Isolates whether fewer, larger NVMe commands reduce per-command overhead enough to matter independently.

3. **h-super-additivity**: Combine QD64 + 4 streams + 512 KiB chunks. Tests whether eliminating bottlenecks at multiple pipeline stages produces multiplicative gains (32 MB in-flight vs 2 MB baseline = 16x more NVMe parallelism).

All artifacts written to `.nous/p2p-evolve-2026-06-01_22-25-40/runs/iter-1/` and validation passed.