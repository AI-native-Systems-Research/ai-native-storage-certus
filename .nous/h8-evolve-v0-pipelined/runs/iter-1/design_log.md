Validation passed. All three artifacts have been written and validated:

**Summary:**

1. **problem.md** — Frames the research question: can pipelining dispatcher v0 (overlap NVMe reads with `cudaMemcpyAsync` + double-buffered `cudaHostAlloc` buffers) match P2P DMA for 4 MiB lookups through certus-server.

2. **bundle.yaml** — Three arms:
   - **h-main**: True pipelining via double-buffer + `cudaMemcpyAsync` (predicts latency reduction by hiding GPU copy behind NVMe reads)
   - **h-control-negative**: Single-chunk (4 KiB) case where pipelining has no overlap opportunity (validates mechanism specificity)
   - **h-robustness**: P2P direct DMA as comparison point (NVMe → GPU BAR1, no host bounce)

3. **handoff_snapshot.md** + **handoff.md** — Complete executor/next-iteration context including code map, code targets with exact line numbers, pipeline algorithm, warnings, and accumulated learnings from prior h8 campaigns (RP-4: cudaHostAlloc required for true async; channel reuse eliminates 544μs overhead).