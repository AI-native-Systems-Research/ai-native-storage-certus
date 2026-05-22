Validation passes. Here's a summary of what was produced:

**Iteration 1 design for h8-pipelined is complete.** Three artifacts written to `.nous/h8-pipelined/runs/iter-1/`:

1. **problem.md** — Frames the pipelining experiment grounded in prior iter-2 data showing NVMe read (790μs) and H2D copy (819μs) are nearly equal, making them ideal for overlap.

2. **bundle.yaml** — 3-arm hypothesis bundle:
   - **h-main**: Pipelined bounce via double-buffered `cudaMemcpyAsync` + individual `ReadAsync`. Predicts ~40-50% latency reduction over non-pipelined bounce.
   - **h-control-negative**: Existing non-pipelined bounce as the "no overlap" baseline.
   - **h-robustness**: P2P warm as the competitive target (pipelined should come within 30%).

3. **handoff_snapshot.md** — Complete executor briefing with code map, targets, and the critical warning about `cudaHostRegister` being needed on SPDK buffers for true async behavior.

Key code changes needed: add 4 CUDA stream FFI functions to `cuda_ffi.rs`, add `BouncePipeline` mode to `p2p_server.rs` with a ~60-line double-buffered pipeline loop.