# Spec Drift Report
Generated: 2026-06-18
Project: gpu-services

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 44 |
| Aligned | 43 (98%) |
| Drifted | 1 (2%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 2 |

## Detailed Findings
### Spec: 001-gpu-cuda-services - GPU CUDA Services
#### Aligned
- FR-001: CUDA initialization with success/failure reporting → src/lib.rs:80-106
- FR-002: Enumerate GPUs with CC 7.0+ reporting model, memory, compute arch → src/device.rs:11-75
- FR-003: Deserialize base64 IPC handle into native Rust structures → src/lib.rs:144-168, src/ipc.rs:11-71
- FR-004: Verify GPU memory is device type via cudaPointerGetAttributes, track in verified set → src/lib.rs:170-190, src/memory.rs:8-34
- FR-005: Pin/unpin operations with idempotent pin, error on unpin of non-pinned → src/lib.rs:193-245
- FR-006: Create GpuDmaBuffer from verified+pinned handle with custom free → src/lib.rs:247-275, src/dma.rs:727-742
- FR-007: All operations return descriptive errors without panicking → all methods use Result<_, String>, no unwrap/panic in public API
- FR-008: Expose functionality through IGpuServices interface → src/lib.rs:79
- FR-009: Build gated behind --features gpu → Cargo.toml features; cfg(feature = "gpu") gates throughout
- FR-010: Unit tests and Criterion benchmarks with gpu feature → src/lib.rs:834-1084, benches/gpu_services_benchmark.rs, benches/dma_transfer_benchmark.rs
- FR-011: dma_copy_to_host using cudaMemcpy D2H, gated behind spdk → src/lib.rs:277-327
- FR-012: dma_copy_to_device using cudaMemcpy H2D, gated behind spdk → src/lib.rs:487-537
- FR-013: prepare_memory_for_spdk full pipeline, gated behind spdk → src/lib.rs:330-484
- FR-014: Return error when gpu feature disabled → cfg(not(feature = "gpu")) blocks return error messages
- FR-015: register_host_memory with cudaHostRegister + spdk_mem_register, rollback on SPDK failure → src/lib.rs:747-792
- FR-016: unregister_host_memory with spdk_mem_unregister + cudaHostUnregister → src/lib.rs:794-831
- FR-017: CUDA stream lifecycle (create_stream, destroy_stream, stream_synchronize) → src/lib.rs:539-605
- FR-018: dma_copy_to_device_async using cudaMemcpyAsync H2D on a stream → src/lib.rs:608-660
- FR-019: memcpy_h2d_async from raw pinned host pointer → src/lib.rs:662-703
- FR-020: allocate_pinned_dma_buffer via cudaHostAlloc + SPDK register → src/lib.rs:706-744

#### Drifted
- FR-005: Spec says pin_memory MAY skip re-verification for pointers in the verified set. Code implements this optimization (skips cudaPointerGetAttributes if verified). However, the unpin path calls only state.pinned.remove() without calling cudaHostUnregister. For IPC-opened device memory this is correct (inherently pinned by CUDA runtime; pin/unpin is tracking-only). The spec language "releases tracking" is slightly ambiguous about whether an underlying unpin system call should occur, but for device memory the behavior is semantically correct.
  - Location: src/lib.rs:227-245
  - Severity: minor

#### Not Implemented
(none)

---

### Spec: 002-gpu-ssd-dma-prepare - GPU-to-SSD DMA Buffer Preparation
#### Aligned
- FR-001: prepare_memory_for_spdk accepts base64 + optional device index, returns SPDK DmaBuffer → src/lib.rs:330-484
- FR-002: Opens IPC handle with cudaIpcMemLazyEnablePeerAccess → src/ipc.rs:55
- FR-003: Checks pin state via internal HashSet (not cudaPointerGetAttributes) → src/lib.rs:405-416
- FR-004: Pins GPU memory if not already pinned, skips if already pinned → src/lib.rs:419-448
- FR-005: Logs pinning actions to logger receptacle → src/lib.rs:443-448
- FR-006: DmaBuffer free function unpins on drop only if function pinned it → src/dma.rs:88-102
- FR-007: DmaBuffer free does NOT unpin if already pinned → src/dma.rs:70-82
- FR-008: Both free functions close IPC handle → both call cudaIpcCloseMemHandle(ptr)
- FR-009: Returns error if not initialized → src/lib.rs:336-340
- FR-010: Returns error if IPC handle cannot be opened → src/lib.rs:393-398
- FR-011: Peer access via cudaIpcMemLazyEnablePeerAccess flag → src/ipc.rs:55
- FR-012: No GPU resource leaks on error → src/lib.rs:385-468 (rollback on all error paths)
- FR-013: Gated behind spdk feature → src/lib.rs:329
- FR-014: Sets device context when index provided, uses current when not → src/lib.rs:350-372
- FR-015: Returns SPDK DmaBuffer (not GpuDmaBuffer) → src/lib.rs:334 return type
- FR-016: Calls spdk_mem_register on GPU pointer → src/dma.rs:122-128
- FR-017: Rolls back spdk_mem_register on subsequent error → src/dma.rs:151-159
- FR-018: Restores original CUDA device context on success and error → src/lib.rs:375-382, 388, 397, 468, 474
- FR-019: register_host_memory with cudaHostRegister + spdk_mem_register, rollback → src/lib.rs:747-792
- FR-020: unregister_host_memory with reverse order → src/lib.rs:794-831
- FR-021: create_spdk_dma_buffer_from_gpu_bar using GDRCopy (gdr_open, pin, map, spdk_mem_register), full cleanup on drop → src/dma.rs:352-466 (gated behind p2p feature)
- FR-022: create_spdk_dma_buffer_from_phys for cross-process P2P (mmap + rte_extmem_register + VFIO DMA map) → src/dma.rs:547-617 (gated behind p2p feature)
- FR-023: create_spdk_dma_buffer_from_bar_direct for existing BAR mapping (DPDK IOMMU, no munmap on drop) → src/dma.rs:635-702 (gated behind p2p feature)
- FR-024: P2P feature exposes GDRCopy FFI bindings and GPU_PAGE_SIZE constant → src/gdrcopy_ffi.rs (pub module, exports gdr_open/close/pin_buffer/unpin_buffer/map/unmap, GPU_PAGE_SIZE = 65536)

#### Drifted
(none)

#### Not Implemented
(none)

---

## Unspecced Code
- **create_spdk_dma_buffer_from_cuda_malloc**: Public function in src/dma.rs:189-226 that creates an SPDK DmaBuffer from cudaMalloc-allocated GPU memory (not IPC-opened). Used internally by the p2p_server for staging buffers. Not specified in either spec.
- **p2p_server binary**: Full NVMe-to-GPU P2P DMA server application at src/bin/p2p_server.rs (679 lines) supporting bounce, p2p, and p2p-cold transfer modes over Unix domain sockets. While it exercises spec'd capabilities end-to-end, the binary itself and its multi-mode architecture are not covered by either spec document.
