Validation passes. Here's a summary:

**Design complete for h8-v0-vs-p2p iteration 1.** 

The hypothesis bundle tests whether P2P direct NVMe→GPU DMA (via `prepare_memory_for_spdk`) beats the current bounce path (NVMe→host DRAM→cudaMemcpy) in dispatcher v0 for 4 MiB sequential lookups, all measured through certus-server.

Three arms:
- **h-main**: P2P sequential will have lower latency than bounce sequential for 4 MiB objects
- **h-control-negative**: Staging-tier lookups show identical latency (mechanism is SSD-path-specific)
- **h-robustness**: Tests 1 MiB objects to probe where setup overhead dominates

Key code changes needed: extend `IpcHandle` with raw CUDA handle bytes, modify certus-server to pass them through, implement `read_from_block_device_p2p` in dispatcher v0 using GPU-backed DmaBuffer sub-views.