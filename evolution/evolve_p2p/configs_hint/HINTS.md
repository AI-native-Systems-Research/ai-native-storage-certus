# P2P Implementation Hints

## Direction

Implement GPUDirect Storage (P2P): NVMe reads DMA directly into GPU BAR1 memory,
bypassing host DRAM entirely. This eliminates the host-bounce cudaMemcpy step and
achieves near line-rate throughput.

## Compile Barriers Removed

1. Build with `--features p2p` — all `#[cfg(feature = "p2p")]` code is compiled and type-checked.
2. The cuda_ffi module uses `std::os::raw::c_int` for all flags and error return types.
3. From pipeline.rs, call: `gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar(dev_ptr, size)` — returns `Result<interfaces::DmaBuffer, String>`.
4. `PipelineRing.streams` is `[GpuStream; 2]` — callers pass `&ring.streams` as a fixed-size array.
