---
task_type: optimization
name: p2p-storage-gpu-transfer
---

# Optimize SSD-to-GPU Data Transfer Path

## Goal
Maximize cold-lookup throughput (GB/s) and minimize p99 latency (ms) for the
certus-server storage system's NVMe-SSD-to-GPU data path.

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
- NVMe Gen4 SSDs via SPDK userspace driver (5.4 GB/s per drive)
- NVIDIA A30 GPU, PCIe Gen4 x16
- Kernel modules: nvidia-peermem, gdrdrv
- 2048 hugepages, memlock unlimited, VFIO-bound NVMe
- Current baseline: ~2.4 GB/s cold lookup, score ~0.20

## Constraints
- Must compile with `--features p2p` (all p2p-gated code is type-checked)
- Data integrity must pass

## Context
The current path uses a ring of 8 CUDA-pinned + SPDK-registered DMA buffers with 2 CUDA streams.
Data flows: NVMe SSD → host DRAM (via SPDK async DMA) → GPU (via cudaMemcpy H2D).
The gpu-services crate has DMA buffer creation functions for various memory types.
Bottlenecks: ring depth limits parallelism, cudaMemcpy H2D adds latency and consumes PCIe bandwidth.
A single NVMe drive reads at 5.4 GB/s; cudaMemcpy H2D peaks at 16.8 GB/s for 4 MiB transfers.
The CPU is NOT in the hot path (3.2% on 64 cores) — SPDK does userspace async DMA.
