Validation passes. Here's the summary:

**Design complete for hypothesis 8, iteration 1 — pipelined bounce buffer transfer.**

The bundle has 3 arms:
- **h-main**: Implement true double-buffered pipelining (overlap NVMe reads with cudaMemcpyAsync H2D copies). Predicts ~50% latency reduction from sequential bounce (~1610μs → ~820μs), approaching P2P warm's ~824μs.
- **h-control-negative**: Sequential bounce baseline (existing code + timing instrumentation). Validates no overlap occurs.
- **h-robustness**: 2-stream pipelined variant — tests whether a single CUDA stream is a bottleneck.

Key code changes: add `cudaMemcpyAsync`/stream FFI bindings to `cuda_ffi.rs`, implement `handle_bounce_pipelined` with per-chunk ReadAsync + async H2D overlap, add timing instrumentation to all modes. Critical risk: SPDK DMA buffers may need explicit `cudaHostRegister` for truly async copies.