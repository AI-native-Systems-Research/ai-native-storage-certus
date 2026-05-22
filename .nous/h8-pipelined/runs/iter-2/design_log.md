Validation passes. Here's a summary of what was designed:

**Iter-2 hypothesis bundle** targets the root cause of iter-1's failure. Iter-1's pipelined bounce was 11-13% slower because `cudaMemcpyAsync` on `cudaHostRegister`-ed SPDK hugepages doesn't achieve true async, plus per-request registration overhead (~200μs) dominated.

**The fix:** Use `cudaHostAlloc` (CUDA-native pinned memory) for pipeline buffers, wrapped via the existing `create_spdk_dma_buffer_from_cuda_host_alloc` (dma.rs:253) so NVMe can DMA into them. Pre-allocate at startup — zero per-request overhead. This guarantees truly asynchronous `cudaMemcpyAsync` execution.

**Three arms:**
- **h-main:** Pipelined bounce v2 with CUDA-native pinned buffers — predicts 20-40% faster than non-pipelined bounce
- **h-control-negative:** Non-pipelined bounce stable at ~1.87ms
- **h-robustness:** P2P warm stable at ~1.3ms as target floor