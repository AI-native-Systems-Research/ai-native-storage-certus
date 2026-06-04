---
task_type: optimization
name: p2p-storage-gpu-transfer-hint
---

# Optimize SSD-to-GPU Data Transfer Path

## Goal
Maximize cold-lookup throughput (GB/s) and minimize p99 latency (ms) for the
certus-server storage system's NVMe-SSD-to-GPU data path.

## Optimization Target
Implement GPUDirect Storage (P2P): NVMe reads DMA directly into GPU BAR1 memory,
bypassing host DRAM entirely. This eliminates the host-bounce cudaMemcpy step.

## Evaluation
- Build: `cargo build -p certus-server --release --features p2p`
- Run server: `./target/release/certus-server --metadata-pci 0000:61:00.0 --data-pci 0000:62:00.0`
- Benchmark: `python3 apps/python/certus-api-bench.py --server localhost:50051 --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- Score: 0.60 * (throughput / 12.0) + 0.40 * (0.4 / p99_ms)
- Hard constraint: no ERRORS in benchmark output

## Files in scope
- components/dispatcher/src/pipeline.rs
- components/dispatcher/src/lib.rs
- components/gpu-services/src/dma.rs

## Hardware
- NVMe Gen4 SSDs via SPDK userspace driver (5.9 GB/s per drive at QD64)
- NVIDIA A30 GPU, PCIe Gen4 x16
- Kernel modules: nvidia-peermem, gdrdrv
- 2048 hugepages, memlock unlimited, VFIO-bound NVMe
- Current baseline: ~2.4 GB/s cold lookup, score ~0.20

## Implementation Notes
- Build uses --features p2p, so all #[cfg(feature = "p2p")] code is compiled and type-checked.
- The cuda_ffi module uses std::os::raw::c_int for all flags and error return types.
- The function create_spdk_dma_buffer_from_gpu_bar(gpu_ptr, size, container_fd) in dma.rs creates an SPDK-registered DMA buffer backed by GPU BAR1 memory.
- PipelineRing.streams is [GpuStream; 2] — callers pass &ring.streams as a fixed-size array.

## Constraints
- Must compile with `--features p2p`
- Data integrity must pass
- Do not modify interfaces or gRPC service
