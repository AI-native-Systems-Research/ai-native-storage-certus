Validation passed. Here's a summary of the experiment design:

**Design complete.** The hypothesis bundle tests whether parallelizing the 32 sequential NVMe reads in dispatcher v1's `pipelined_ssd_to_gpu` can dramatically reduce SSD-tier lookup latency for 4 MiB objects.

**Key insight from exploration:** The current pipeline issues `ReadSync` per 128 KiB chunk (~600us each × 32 = ~19,200us serial floor). The reference `gpu-bb-vs-p2p` achieves ~1,300us for the same data volume by priming an async ring — a ~15x improvement from NVMe internal parallelism.

**Two arms designed:**
- **h-main**: Full optimization (parallel ReadAsync + cudaMemcpyAsync on alternating CUDA streams)
- **h-ablation**: NVMe parallelism only (parallel ReadAsync + synchronous GPU copy) — tests whether NVMe read dominance (RP-16) means GPU async adds negligible benefit

All artifacts written to `.nous/h8-v1-optimized-pipeline/runs/iter-1/` and validation passes.