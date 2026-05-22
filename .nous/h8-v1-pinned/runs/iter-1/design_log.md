Validation passes. Here's a summary:

**Design complete for h8-v1-pinned iteration 1.** Three artifacts written:

1. **problem.md** — Frames the research question: does P2P with cached GPU staging beat pipelined bounce for sequential 4 MiB lookups? Includes baseline validation (12969 μs/obj from prior experiment), experimental conditions (A: bounce 1-iter, B: P2P 1-iter, C: P2P 20-iter), and success criteria.

2. **bundle.yaml** — Two arms:
   - **h-main**: P2P persistent will show lower first-hit latency than bounce by eliminating 32× cudaMemcpy + 32× host memcpy. Includes 7 code_changes spanning IpcHandle extension, DMA cache, P2P read function, routing logic, and service pass-through.
   - **h-robustness**: 20-iteration P2P stability (max/min < 1.3) confirms caching doesn't degrade.

3. **handoff_snapshot.md** (+ campaign handoff.md) — Complete executor/next-designer context including code map, payload format details, and the key insight that expected effect size may only be ~1% since sequential NVMe time dominates — making this a regime characterization experiment that either validates the P2P mechanism or points to P2P+BatchSubmit as the next step.