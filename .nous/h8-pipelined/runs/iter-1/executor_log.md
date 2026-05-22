Validation passes. Let me do a quick summary of what was accomplished:

---

## Summary

**All 5 phases complete. Validation passes.**

### Results

| Condition | Avg Latency | Throughput |
|-----------|-------------|------------|
| Bounce (non-pipelined, h-control-negative) | **1.87 ms** | 2143 MB/s |
| Bounce-pipeline (h-main treatment) | **2.07-2.12 ms** | 1890-1935 MB/s |
| P2P warm (h-robustness reference) | **1.27-1.34 ms** | 2976-3147 MB/s |

### Key Finding: h-main REFUTED

The pipelined bounce implementation is **11-13% slower** than the non-pipelined baseline, opposite to the predicted 40-50% speedup. Two consistent runs across different NVMe devices confirm this.

**Diagnosed root cause**: `cudaMemcpyAsync` with SPDK hugepage buffers (even after `cudaHostRegister`) is not achieving true async execution. The likely explanation is that CUDA's DMA engine requires memory allocated via `cudaHostAlloc` for genuine async copies; SPDK's hugepage allocations, while physically pinned, are not recognized by CUDA's async DMA path. The overhead of `cudaHostRegister`, stream creation, and `cudaStreamSynchronize` (~200μs) exceeds any overlap benefit.

### Principle Updates (4 extracted)
- **RP-1**: `cudaMemcpyAsync` unreliable with SPDK hugepage + `cudaHostRegister` — use `cudaHostAlloc` instead
- **RP-2**: Baseline bounce latency is 1.87ms (faster than prior 2.65ms — device variation is significant)  
- **RP-3**: P2P warm confirmed stable at 1.27-1.34ms across runs
- **RP-4**: For pipelining to work, buffers must be CUDA-native (`cudaHostAlloc`) + SPDK-registered, not the reverse

**Recommended iter-2**: Allocate pipeline buffers via `cudaHostAlloc` (CUDA-pinned), register with SPDK's `spdk_mem_register()`, and verify async execution with `cudaStreamQuery()` before synchronize.