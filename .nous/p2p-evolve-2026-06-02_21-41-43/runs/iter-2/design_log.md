Validation passes.

## Summary

Designed iteration 2 of the P2P GPUDirect Storage experiment. The key insight from iter-1's failure: the batch-based pipeline (submit all reads → wait all → copy all) lost 19% throughput vs baseline due to serialized phases (RP-2). 

**Iteration 2 fix:** Replace the batch approach with a true **sliding-window pipeline** that mirrors the baseline's `pipelined_ssd_to_gpu_zero_copy` structure:
- VecDeque tracks in-flight ring slot indices
- Each NVMe completion immediately triggers a `cudaMemcpyAsync(D2D)` and submits the next read
- D2D copies at GPU internal bandwidth (~1.5 TB/s) complete in <0.1µs vs ~22µs NVMe reads — ring slots are always available

Two arms:
- **h-main**: Pure P2P sliding-window, no DRAM allocation (tests max performance)
- **h-ablation**: Same P2P + DRAM backfill (tests RP-3 cache coherence overhead)

All three artifacts written and validated: `problem.md`, `bundle.yaml`, `handoff_snapshot.md`.