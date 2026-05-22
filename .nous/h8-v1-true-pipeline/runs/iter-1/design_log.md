Validation passes. Here's a summary of what was produced:

**Experiment design complete for h8-v1-true-pipeline iteration 1.**

Three files written to `.nous/h8-v1-true-pipeline/runs/iter-1/`:

- **problem.md** — Frames the research question with grounded code references. The existing `pipelined_ssd_to_gpu` (pipeline.rs:60-119) is sequential despite claiming pipelining. Baseline SSD-tier latency is 7,029 us/obj from prior experiments.

- **bundle.yaml** — Two arms:
  - **h-main**: Rewrite pipeline.rs to use double-buffered ReadAsync + cudaMemcpyAsync for true overlap. Requires adding CUDA stream FFI bindings to cuda_ffi.rs. Predicts >15% latency reduction from overlapping NVMe reads with GPU copies.
  - **h-control-negative**: Apply P2P direct DMA patch (bypasses memory-tier entirely). Expected to achieve ~3,451 us/obj as the comparison ceiling.

- **handoff_snapshot.md** — Complete executor/next-iteration context including code map, code targets, warnings (SPDK symlinks, sudo, RUSTFLAGS), and the key uncertainty: whether `cudaMemcpyAsync` from `cudaHostRegistered` mmap'd memory actually runs asynchronously.